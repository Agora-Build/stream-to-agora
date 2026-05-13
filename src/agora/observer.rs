//! The `rtc_conn_observer` the SDK calls back on (its own thread), the
//! event channel those callbacks push into, and the pure logic that maps
//! a received event to a decision the main thread acts on.

/// Events surfaced from the SDK observer callbacks (or, for `Shutdown`,
/// from the SIGINT handler / `--duration` timer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnEvent {
    /// `on_connected` fired. `conn_id` is the connection id from `rtc_conn_info`.
    Connected { conn_id: u32 },
    /// `on_disconnected` fired. `reason` is the SDK's reason code.
    Disconnected { reason: i32 },
    /// `on_connection_lost` fired.
    ConnectionLost,
    /// `on_error` fired (or connect() returned an error). `code`/`msg` from the SDK.
    Failed { code: i32, msg: String },
    /// SIGINT received or `--duration` elapsed — caller asked us to stop.
    Shutdown,
    /// `on_token_privilege_will_expire` fired — the SDK is ~30 s away
    /// from rejecting the current token. The renew task uses this to
    /// run `--token-renew-cmd`. Emitted ONLY to the optional renew
    /// sender (see `set_renew_sender`), never to the main `EVENT_TX`.
    TokenWillExpire { current: String },
}

/// What the main thread should do given a received `ConnEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// We're connected — print `ready`, then idle.
    Ready { conn_id: u32 },
    /// Stop cleanly with exit code 0.
    Stop,
    /// Abort with a non-zero exit and this message.
    Fatal { message: String },
    /// The event is benign noise; the main loop should keep waiting.
    Continue,
}

/// Pure decision: given an event we just received, what happens next?
pub fn outcome_for(event: &ConnEvent) -> Outcome {
    match event {
        ConnEvent::Connected { conn_id } => Outcome::Ready { conn_id: *conn_id },
        ConnEvent::Shutdown => Outcome::Stop,
        ConnEvent::ConnectionLost => Outcome::Fatal { message: "connection lost".into() },
        ConnEvent::Disconnected { reason } =>
            Outcome::Fatal { message: format!("disconnected (reason {reason})") },
        ConnEvent::Failed { code, msg } => {
            let named = super::error::error_name(*code);
            let message = match (named, msg.is_empty()) {
                (Some(n), false) => format!("connection error (code {code}: {n}): {msg}"),
                (Some(n), true)  => format!("connection error (code {code}: {n})"),
                (None, false)    => format!("connection error (code {code}): {msg}"),
                (None, true)     => format!("connection error (code {code})"),
            };
            Outcome::Fatal { message }
        }
        // TokenWillExpire never reaches outcome_for in practice (the trampoline
        // emits only to RENEW_TX, not EVENT_TX). This arm is defense-in-depth.
        ConnEvent::TokenWillExpire { .. } => Outcome::Continue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_maps_to_ready() {
        assert_eq!(outcome_for(&ConnEvent::Connected { conn_id: 7 }), Outcome::Ready { conn_id: 7 });
    }
    #[test]
    fn shutdown_maps_to_stop() {
        assert_eq!(outcome_for(&ConnEvent::Shutdown), Outcome::Stop);
    }
    #[test]
    fn failures_map_to_fatal_with_message() {
        assert!(matches!(outcome_for(&ConnEvent::ConnectionLost), Outcome::Fatal { .. }));
        let o = outcome_for(&ConnEvent::Failed { code: 110, msg: "bad token".into() });
        match o { Outcome::Fatal { message } => {
            assert!(message.contains("ERR_INVALID_TOKEN"));
            assert!(message.contains("bad token"));
        }, _ => panic!("expected Fatal") }
        let o = outcome_for(&ConnEvent::Disconnected { reason: 4 });
        match o { Outcome::Fatal { message } => assert!(message.contains("reason 4")), _ => panic!() }
    }
}

// ─── FFI glue: event channel + extern "C" trampolines ────────────────────────

use std::os::raw::{c_char, c_int, c_void};
use std::panic::catch_unwind;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use super::sys;

/// Phase 1 opens exactly one RTC connection per process, and the C
/// `rtc_conn_observer` struct has no user-data slot, so the trampolines
/// reach the event channel through this process-global. `Session::connect`
/// installs the sender before registering the observer and clears it on Drop.
///
/// TODO(phase2): when a second simultaneous connection is needed, replace
/// this global with a connection-keyed map (e.g. `DashMap<conn_id, Sender>`).
static EVENT_TX: Mutex<Option<Sender<ConnEvent>>> = Mutex::new(None);

/// Second sender for the renew task. Only fed by the
/// `on_token_privilege_will_expire` trampoline; not used by any other
/// callback. None when `--token-renew-cmd` is not set.
static RENEW_TX: Mutex<Option<Sender<ConnEvent>>> = Mutex::new(None);

/// Install the channel sender the trampolines will push events into.
/// Must be called before the observer is registered with the SDK.
pub(super) fn set_event_sender(tx: Sender<ConnEvent>) {
    *EVENT_TX.lock().unwrap() = Some(tx);
}

/// Optional second sender for the renew task. The
/// `on_token_privilege_will_expire` callback emits only to this
/// sender, so `Session::run` never sees `TokenWillExpire`.
pub(super) fn set_renew_sender(tx: Sender<ConnEvent>) {
    *RENEW_TX.lock().unwrap() = Some(tx);
}

/// Remove both event senders; subsequent trampoline callbacks become no-ops.
/// Called from `Session::Drop`.
pub(super) fn clear_event_sender() {
    *EVENT_TX.lock().unwrap() = None;
    *RENEW_TX.lock().unwrap() = None;
}

/// Send an event from a callback thread. Never panics, never unwinds.
fn emit(ev: ConnEvent) {
    if let Ok(guard) = EVENT_TX.lock() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(ev); // receiver gone => nothing to do
        }
    }
}

/// Run a callback body, swallowing any panic (unwinding into C is UB).
fn guard<F: FnOnce() + std::panic::UnwindSafe>(f: F) {
    let _ = catch_unwind(f);
}

/// Convert a (possibly-null) C string pointer to an owned `String`.
///
/// # Safety
/// If `p` is non-null it must point to a valid NUL-terminated C string
/// for the duration of this call. The SDK guarantees this for all `msg`
/// arguments within a callback invocation.
unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() }
}

// bindgen renders all fn-pointer fields as `Option<unsafe extern "C" fn(...)>`,
// so the trampolines must be `unsafe extern "C" fn` to match.
// The first parameter is `*mut c_void` (the SDK's opaque connection handle).

unsafe extern "C" fn on_connected(
    _conn: *mut c_void,
    info: *const sys::rtc_conn_info,
    _reason: c_int,
) {
    guard(|| {
        let conn_id = unsafe { info.as_ref().map(|i| i.id as u32).unwrap_or(0) };
        emit(ConnEvent::Connected { conn_id });
    });
}

unsafe extern "C" fn on_disconnected(
    _conn: *mut c_void,
    _info: *const sys::rtc_conn_info,
    reason: c_int,
) {
    guard(|| emit(ConnEvent::Disconnected { reason: reason as i32 }));
}

unsafe extern "C" fn on_connection_lost(
    _conn: *mut c_void,
    _info: *const sys::rtc_conn_info,
) {
    guard(|| emit(ConnEvent::ConnectionLost));
}

unsafe extern "C" fn on_connection_failure(
    _conn: *mut c_void,
    _info: *const sys::rtc_conn_info,
    reason: c_int,
) {
    guard(|| emit(ConnEvent::Failed { code: reason as i32, msg: String::new() }));
}

unsafe extern "C" fn on_error(
    _conn: *mut c_void,
    error: c_int,
    msg: *const c_char,
) {
    guard(|| {
        let m = unsafe { cstr(msg) };
        emit(ConnEvent::Failed { code: error as i32, msg: m });
    });
}

unsafe extern "C" fn on_token_privilege_will_expire(
    _conn: *mut c_void,
    token: *const c_char,
) {
    guard(|| {
        let current = unsafe { cstr(token) };
        // Emit ONLY to the renew sender. The main event channel doesn't
        // care about this event. If no renew task is registered, the
        // event is dropped.
        if let Ok(guard) = RENEW_TX.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(ConnEvent::TokenWillExpire { current });
            }
        }
    });
}

unsafe extern "C" fn on_token_privilege_did_expire(_conn: *mut c_void) {
    guard(|| {
        // Hard failure: SDK will start rejecting frames imminently.
        // Emit via the main channel so Session::run sees Failed.
        emit(ConnEvent::Failed {
            code: 109, // ERR_TOKEN_EXPIRED
            msg: "token expired without renewal".into(),
        });
    });
}

/// Build the observer struct: all fields zeroed (= no callback) except the
/// few Phase 1 cares about. `sys::rtc_conn_observer` derives `Default` via
/// bindgen (`derive_default(true)`); a zeroed struct of fn pointers means
/// "no handler", which the SDK tolerates.
pub(super) fn build_observer() -> sys::rtc_conn_observer {
    let mut o = sys::rtc_conn_observer::default();
    o.on_connected = Some(on_connected);
    o.on_disconnected = Some(on_disconnected);
    o.on_connection_lost = Some(on_connection_lost);
    o.on_connection_failure = Some(on_connection_failure);
    o.on_error = Some(on_error);
    o.on_token_privilege_will_expire = Some(on_token_privilege_will_expire);
    o.on_token_privilege_did_expire = Some(on_token_privilege_did_expire);
    o
}
