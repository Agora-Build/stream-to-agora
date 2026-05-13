//! RAII wrapper over the Agora service + one RTC connection.
//!
//! Lifecycle: `Session::connect(cfg)` → creates+initializes the service,
//! creates the RTC connection, registers the observer, calls
//! `agora_rtc_conn_connect`, and waits up to `connect_timeout` for the
//! `on_connected` event. `run()` then idles until SIGINT / `--duration`.
//! `Drop` disconnects, destroys the connection, and releases the service —
//! in that order.

use std::ffi::CString;
use std::os::raw::c_void;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use super::error::{check, AgoraError};
use super::observer::{self, ConnEvent, Outcome};
use super::sys;

// CHANNEL_PROFILE_TYPE / CLIENT_ROLE_TYPE values from the SDK's AgoraBase.h
// (they live in the C++ headers, not the C ones, so we spell them out).
const CHANNEL_PROFILE_LIVE_BROADCASTING: i32 = 1;
const CLIENT_ROLE_BROADCASTER: i32 = 1;

/// Everything `Session::connect` needs, derived from the CLI in `main.rs`.
pub struct SessionConfig {
    pub app_id: String,
    pub channel: String,
    /// The value to pass to `agora_rtc_conn_connect` as `user_id` — always a
    /// string (the C API's `user_id_t` is `const char*`); for an all-digit id
    /// this is just the digits.
    pub user_id: String,
    /// `true` when `user_id` is a string account (non-digit, or `s/`-prefixed).
    pub use_string_uid: bool,
    pub token: String,
    pub connect_timeout: Duration,
}

pub struct Session {
    svc: *mut c_void,
    conn: *mut c_void,
    /// Boxed so its address is stable; the SDK holds the pointer we pass to
    /// `agora_rtc_conn_register_observer`.
    _observer: Box<sys::rtc_conn_observer>,
    rx: Receiver<ConnEvent>,
    /// Sender clone so the SIGINT handler / `--duration` timer can push `Shutdown`.
    tx: mpsc::Sender<ConnEvent>,
    /// Connection id reported by `on_connected`.
    pub conn_id: u32,
}


impl Session {
    pub fn connect(cfg: &SessionConfig) -> Result<Session, AgoraError> {
        // Reject interior NULs up front (before any FFI).
        let app_id = CString::new(cfg.app_id.as_str())
            .map_err(|_| AgoraError::msg("app id", "contains a NUL byte"))?;
        let channel = CString::new(cfg.channel.as_str())
            .map_err(|_| AgoraError::msg("channel", "contains a NUL byte"))?;
        let user_id = CString::new(cfg.user_id.as_str())
            .map_err(|_| AgoraError::msg("rtc user id", "contains a NUL byte"))?;
        let token = CString::new(cfg.token.as_str())
            .map_err(|_| AgoraError::msg("token", "contains a NUL byte"))?;

        // 1. Service.
        let svc = unsafe { sys::agora_service_create() };
        if svc.is_null() {
            return Err(AgoraError::null("agora_service_create"));
        }

        let mut svc_cfg: sys::agora_service_config = unsafe { std::mem::zeroed() };
        svc_cfg.enable_audio_processor = 0;
        svc_cfg.enable_audio_device = 0;
        svc_cfg.enable_video = 0;
        svc_cfg.app_id = app_id.as_ptr();
        svc_cfg.area_code = 0xFFFF_FFFF; // AREA_CODE_GLOB
        svc_cfg.channel_profile = CHANNEL_PROFILE_LIVE_BROADCASTING;
        svc_cfg.use_string_uid = if cfg.use_string_uid { 1 } else { 0 };
        let rc = unsafe { sys::agora_service_initialize(svc, &svc_cfg) };
        if let Err(e) = check(rc, "agora_service_initialize") {
            unsafe { sys::agora_service_release(svc) };
            return Err(e);
        }

        // 2. RTC connection.
        let mut conn_cfg: sys::rtc_conn_config = unsafe { std::mem::zeroed() };
        conn_cfg.auto_subscribe_audio = 0;
        conn_cfg.auto_subscribe_video = 0;
        conn_cfg.enable_audio_recording_or_playout = 0;
        conn_cfg.client_role_type = CLIENT_ROLE_BROADCASTER;
        conn_cfg.channel_profile = CHANNEL_PROFILE_LIVE_BROADCASTING;
        let conn = unsafe { sys::agora_rtc_conn_create(svc, &conn_cfg) };
        if conn.is_null() {
            unsafe { sys::agora_service_release(svc) };
            return Err(AgoraError::null("agora_rtc_conn_create"));
        }

        // 3. Observer + event channel.
        let (tx, rx) = mpsc::channel::<ConnEvent>();
        let tx_clone = tx.clone();
        observer::set_event_sender(tx);
        let mut observer = Box::new(observer::build_observer());
        let rc = unsafe {
            sys::agora_rtc_conn_register_observer(conn, observer.as_mut() as *mut _)
        };
        if let Err(e) = check(rc, "agora_rtc_conn_register_observer") {
            observer::clear_event_sender();
            unsafe {
                sys::agora_rtc_conn_destroy(conn);
                sys::agora_service_release(svc);
            }
            return Err(e);
        }

        // 4. Connect.
        let rc = unsafe {
            sys::agora_rtc_conn_connect(conn, token.as_ptr(), channel.as_ptr(), user_id.as_ptr())
        };
        if let Err(e) = check(rc, "agora_rtc_conn_connect") {
            observer::clear_event_sender();
            unsafe {
                sys::agora_rtc_conn_unregister_observer(conn);
                sys::agora_rtc_conn_destroy(conn);
                sys::agora_service_release(svc);
            }
            return Err(e);
        }

        // 5. Wait for on_connected (or a fatal event, or timeout).
        let conn_id = loop {
            match rx.recv_timeout(cfg.connect_timeout) {
                Ok(ConnEvent::Connected { conn_id }) => break conn_id,
                Ok(other) => match observer::outcome_for(&other) {
                    Outcome::Fatal { message } => {
                        observer::clear_event_sender();
                        unsafe {
                            sys::agora_rtc_conn_disconnect(conn);
                            sys::agora_rtc_conn_unregister_observer(conn);
                            sys::agora_rtc_conn_destroy(conn);
                            sys::agora_service_release(svc);
                        }
                        return Err(AgoraError::msg("connect", message));
                    }
                    // Connecting/Reconnecting noise isn't surfaced as events here, but be defensive:
                    _ => continue,
                },
                Err(RecvTimeoutError::Timeout) => {
                    observer::clear_event_sender();
                    unsafe {
                        sys::agora_rtc_conn_disconnect(conn);
                        sys::agora_rtc_conn_unregister_observer(conn);
                        sys::agora_rtc_conn_destroy(conn);
                        sys::agora_service_release(svc);
                    }
                    return Err(AgoraError::msg(
                        "connect",
                        format!(
                            "timed out after {:?} waiting to connect \
                             — check app id / token / channel / network",
                            cfg.connect_timeout
                        ),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    observer::clear_event_sender();
                    unsafe {
                        sys::agora_rtc_conn_disconnect(conn);
                        sys::agora_rtc_conn_unregister_observer(conn);
                        sys::agora_rtc_conn_destroy(conn);
                        sys::agora_service_release(svc);
                    }
                    return Err(AgoraError::msg("connect", "event channel closed unexpectedly"));
                }
            }
        };

        Ok(Session { svc, conn, _observer: observer, rx, tx: tx_clone, conn_id })
    }

    /// Hand out a clonable sender so the SIGINT handler (and a `--duration`
    /// timer) can push `ConnEvent::Shutdown` into the same channel `run`
    /// listens on.
    pub fn sender(&self) -> mpsc::Sender<ConnEvent> {
        self.tx.clone()
    }

    /// Block until a `Shutdown` event arrives (clean, exit 0) or a fatal
    /// connection event arrives (returns `Err`, caller exits non-zero).
    pub fn run(&self) -> Result<(), AgoraError> {
        loop {
            match self.rx.recv() {
                Ok(ev) => match observer::outcome_for(&ev) {
                    Outcome::Stop => return Ok(()),
                    Outcome::Fatal { message } => {
                        return Err(AgoraError::msg("connection", message))
                    }
                    Outcome::Ready { .. } => continue, // shouldn't recur; ignore
                },
                Err(_) => return Ok(()), // all senders dropped — treat as shutdown
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Best-effort teardown in the required order. Errors can't propagate
        // from Drop; log them.
        unsafe {
            let rc = sys::agora_rtc_conn_disconnect(self.conn);
            if rc != 0 {
                eprintln!("warning: agora_rtc_conn_disconnect returned {rc}");
            }
            sys::agora_rtc_conn_unregister_observer(self.conn);
            sys::agora_rtc_conn_destroy(self.conn);
            let rc = sys::agora_service_release(self.svc);
            if rc != 0 {
                eprintln!("warning: agora_service_release returned {rc}");
            }
        }
        observer::clear_event_sender();
    }
}
