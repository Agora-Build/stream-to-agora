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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::error::{check, AgoraError};
use super::observer::{self, ConnEvent, Outcome};
use super::sys;

// CHANNEL_PROFILE_TYPE / CLIENT_ROLE_TYPE values from the SDK's AgoraBase.h
// (they live in the C++ headers, not the C ones, so we spell them out).
const CHANNEL_PROFILE_LIVE_BROADCASTING: i32 = 1;
const CLIENT_ROLE_BROADCASTER: i32 = 1;

/// Latching cancellation primitive shared between `Session::run` and the
/// pump tasks. A bare `Notify` doesn't work for this: `notify_waiters()`
/// only wakes waiters currently registered at the moment of the call. If
/// the pump is mid-iteration (draining a buffered HLS segment with
/// `sleep_until` between frames), it's not registered as a waiter; the
/// notify is lost and a subsequent `cancel.notified().await` blocks
/// forever. The AtomicBool latches the cancel signal so any later check
/// returns immediately.
pub struct CancelToken {
    flag: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Arc<Self> {
        Arc::new(CancelToken { flag: AtomicBool::new(false), notify: Notify::new() })
    }
    /// Fire the cancel signal — sets the flag, then wakes any registered waiters.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
    /// Await cancellation. Returns immediately if already fired.
    pub async fn cancelled(&self) {
        if self.is_cancelled() { return; }
        // Register the waiter BEFORE re-checking the flag — Notify
        // semantics: `notified()` returns a future that only registers
        // for wake-up after its first poll. Polling once via `.await`
        // ensures registration, but we must re-check the flag after
        // registration to close the race where `cancel()` runs between
        // our `is_cancelled` check and `notified().await`.
        let notified = self.notify.notified();
        tokio::pin!(notified);
        // Register as waiter (first poll completes registration, doesn't resolve).
        // We use `.as_mut().enable()` to register without awaiting.
        // tokio 1.x: `Notified::enable` was added in 1.13.
        notified.as_mut().enable();
        if self.is_cancelled() { return; }
        notified.await;
    }
}

/// Send + Sync cap holding only the connection handle. Used by the
/// token-renew task, which lives on its own Tokio worker. The SDK's
/// `agora_rtc_conn_renew_token` is safe to call from any thread; we
/// `unsafe impl Send + Sync` to assert that.
pub struct RenewHandle {
    conn: *mut c_void,
}
unsafe impl Send for RenewHandle {}
unsafe impl Sync for RenewHandle {}

impl RenewHandle {
    pub fn renew(&self, new_token: &str) -> Result<(), AgoraError> {
        renew_token_inner(self.conn, new_token)
    }
}

fn renew_token_inner(conn: *mut c_void, new_token: &str) -> Result<(), AgoraError> {
    let c = std::ffi::CString::new(new_token).map_err(|_| {
        AgoraError::msg("renew token", "new token contains a NUL byte")
    })?;
    let rc = unsafe { sys::agora_rtc_conn_renew_token(conn, c.as_ptr()) };
    check(rc, "agora_rtc_conn_renew_token")
}

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
    /// Media-node factory; needed to create the encoded/raw senders that
    /// back AudioPublisher / VideoPublisher. Owned by Session; destroyed
    /// before service release in Drop.
    factory: *mut c_void,
    /// `app_id` CString — the SDK likely retains the pointer from
    /// `agora_service_config.app_id` past `initialize()`; keep it alive
    /// for the lifetime of `svc`. Underscored — never read by Rust.
    _app_id: CString,
    /// `channel` CString — pinned past `agora_rtc_conn_connect` in case the
    /// SDK aliases the pointer for reconnect / logging. Same rationale as
    /// `_app_id`. Underscored — never read by Rust.
    _channel: CString,
    /// `token` CString — pinned for the same reason. Tokens can be rotated
    /// via `agora_rtc_conn_renew_token` later, but we don't drop the
    /// original until `Session` itself drops.
    _token: CString,
    /// `user_id` CString — pinned for the same reason.
    _user_id: CString,
    rx: UnboundedReceiver<ConnEvent>,
    /// Sender clone so the SIGINT handler / `--duration` timer can push `Shutdown`.
    tx: UnboundedSender<ConnEvent>,
    /// Connection id reported by `on_connected`.
    pub conn_id: u32,
    /// Latched cancellation token shared with pump tasks. `Session::run`
    /// fires it before joining handles, so publishers (moved into pump
    /// tasks) Drop while `conn` is still alive. Latched (vs raw Notify)
    /// so the signal survives the pump being mid-iteration when cancel
    /// fires — see CancelToken docs above.
    cancel: Arc<CancelToken>,
    /// JoinHandles for tokio::spawn'd pump tasks. `Session::run`
    /// takes these and awaits them after notifying cancel.
    pump_handles: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
}


impl Session {
    pub async fn connect(cfg: &SessionConfig) -> Result<Session, AgoraError> {
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
        // Phase 2 needs both audio_processor and video enabled — the SDK
        // returns NULL from `agora_service_create_custom_audio_track_encoded`
        // (and friends) with INVALID_STATE when enable_audio_processor=0.
        // audio_device stays disabled: we don't use a physical mic/speaker.
        svc_cfg.enable_audio_processor = 1;
        svc_cfg.enable_audio_device = 0;
        svc_cfg.enable_video = 1;
        svc_cfg.app_id = app_id.as_ptr();
        svc_cfg.area_code = 0xFFFF_FFFF; // AREA_CODE_GLOB
        svc_cfg.channel_profile = CHANNEL_PROFILE_LIVE_BROADCASTING;
        svc_cfg.use_string_uid = if cfg.use_string_uid { 1 } else { 0 };
        let rc = unsafe { sys::agora_service_initialize(svc, &svc_cfg) };
        if let Err(e) = check(rc, "agora_service_initialize") {
            unsafe { sys::agora_service_release(svc) };
            return Err(e);
        }

        let factory = unsafe { sys::agora_service_create_media_node_factory(svc) };
        if factory.is_null() {
            unsafe { sys::agora_service_release(svc); }
            return Err(AgoraError::null("agora_service_create_media_node_factory"));
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
            unsafe {
                sys::agora_media_node_factory_destroy(factory);
                sys::agora_service_release(svc);
            }
            return Err(AgoraError::null("agora_rtc_conn_create"));
        }

        // 3. Observer + event channel.
        let (tx, mut rx) = mpsc::unbounded_channel::<ConnEvent>();
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
                sys::agora_media_node_factory_destroy(factory);
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
                let _ = sys::agora_rtc_conn_unregister_observer(conn);
                sys::agora_rtc_conn_destroy(conn);
                sys::agora_media_node_factory_destroy(factory);
                sys::agora_service_release(svc);
            }
            return Err(e);
        }

        // 5. Wait for on_connected (or a fatal event, or timeout).
        let deadline = std::time::Instant::now() + cfg.connect_timeout;
        let conn_id = loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                // Treat zero-remaining the same as Elapsed: full teardown, return.
                observer::clear_event_sender();
                unsafe {
                    sys::agora_rtc_conn_disconnect(conn);
                    let _ = sys::agora_rtc_conn_unregister_observer(conn);
                    sys::agora_rtc_conn_destroy(conn);
                    sys::agora_media_node_factory_destroy(factory);
                    sys::agora_service_release(svc);
                }
                return Err(AgoraError::msg("connect", format!(
                    "timed out after {:?} waiting to connect \
                     — check app id / token / channel / network",
                    cfg.connect_timeout)));
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(ConnEvent::Connected { conn_id })) => break conn_id,
                Ok(Some(other)) => match observer::outcome_for(&other) {
                    Outcome::Fatal { message } => {
                        observer::clear_event_sender();
                        unsafe {
                            sys::agora_rtc_conn_disconnect(conn);
                            let _ = sys::agora_rtc_conn_unregister_observer(conn);
                            sys::agora_rtc_conn_destroy(conn);
                            sys::agora_media_node_factory_destroy(factory);
                            sys::agora_service_release(svc);
                        }
                        return Err(AgoraError::msg("connect", message));
                    }
                    // Outcome::Ready was matched above; Outcome::Stop can't arrive
                    // here because Session.tx (the only handle to the shutdown
                    // sender) doesn't exist until `connect()` returns Ok.
                    // Treat both as benign noise and keep waiting.
                    _ => continue,
                },
                Ok(None) => {
                    observer::clear_event_sender();
                    unsafe {
                        sys::agora_rtc_conn_disconnect(conn);
                        let _ = sys::agora_rtc_conn_unregister_observer(conn);
                        sys::agora_rtc_conn_destroy(conn);
                        sys::agora_media_node_factory_destroy(factory);
                        sys::agora_service_release(svc);
                    }
                    return Err(AgoraError::msg("connect", "event channel closed unexpectedly"));
                }
                Err(_elapsed) => {
                    observer::clear_event_sender();
                    unsafe {
                        sys::agora_rtc_conn_disconnect(conn);
                        let _ = sys::agora_rtc_conn_unregister_observer(conn);
                        sys::agora_rtc_conn_destroy(conn);
                        sys::agora_media_node_factory_destroy(factory);
                        sys::agora_service_release(svc);
                    }
                    return Err(AgoraError::msg("connect", format!(
                        "timed out after {:?} waiting to connect \
                         — check app id / token / channel / network",
                        cfg.connect_timeout)));
                }
            }
        };

        Ok(Session {
            svc,
            conn,
            _observer: observer,
            factory,
            _app_id: app_id,
            _channel: channel,
            _token: token,
            _user_id: user_id,
            rx,
            tx: tx_clone,
            conn_id,
            cancel: CancelToken::new(),
            pump_handles: tokio::sync::Mutex::new(Vec::new()),
        })
    }

    /// Hand out a clonable sender so the SIGINT handler (and a `--duration`
    /// timer) can push `ConnEvent::Shutdown` into the same channel `run`
    /// listens on.
    pub fn sender(&self) -> UnboundedSender<ConnEvent> {
        self.tx.clone()
    }

    /// Create a publisher for the chosen codec mode. The returned
    /// AudioPublisher owns the underlying SDK sender + track handles.
    /// Wired in Task 11; per-mode publishers come from Tasks 9–10.
    pub fn create_audio_publisher(&self, mode: super::publisher::CodecMode)
        -> Result<super::publisher::AudioPublisher, AgoraError>
    {
        super::publisher::create_audio(self.svc, self.conn, self.factory, mode)
    }

    pub fn create_video_publisher(&self, mode: super::publisher::CodecMode)
        -> Result<super::publisher::VideoPublisher, AgoraError>
    {
        super::publisher::create_video(self.svc, self.conn, self.factory, mode)
    }

    /// Block on the event channel until Shutdown (Ok) or a fatal event
    /// (Err). Before returning, fire the cancellation Notify so pump
    /// tasks exit promptly, then await every registered pump JoinHandle
    /// so publisher Drop runs on a live conn.
    pub async fn run(&mut self) -> Result<(), AgoraError> {
        let outcome = loop {
            match self.rx.recv().await {
                Some(ev) => match observer::outcome_for(&ev) {
                    Outcome::Stop => break Ok(()),
                    Outcome::Fatal { message } => {
                        break Err(AgoraError::msg("connection", message));
                    }
                    Outcome::Ready { .. } |
                    Outcome::Continue => continue,
                },
                None => break Ok(()), // all senders dropped — treat as Shutdown
            }
        };

        // Notify pumps + renew task to exit; await their JoinHandles.
        // CancelToken (vs raw Notify) latches the signal so pumps that
        // were mid-iteration when cancel fired still see it on their
        // next select! check.
        self.cancel.cancel();
        let mut handles = self.pump_handles.lock().await;
        for h in handles.drain(..) {
            let _ = h.await;
        }

        outcome
    }

    /// Clone the cancellation Notify so pump / renew tasks can `select!` on it.
    pub fn cancel_signal(&self) -> Arc<CancelToken> {
        self.cancel.clone()
    }

    /// Register a pump task's JoinHandle. `Session::run` awaits all
    /// registered handles before returning, so publishers Drop on a
    /// live connection.
    pub async fn register_pump(&self, h: JoinHandle<()>) {
        self.pump_handles.lock().await.push(h);
    }

    /// Hand out a thread-safe cap for calling `agora_rtc_conn_renew_token`.
    /// `Session` itself is `!Send` (raw pointer fields), so the renew task
    /// (which lives on its own Tokio worker) can't own a `&Session`.
    pub fn renew_handle(&self) -> RenewHandle {
        RenewHandle { conn: self.conn }
    }

    /// Direct renew (for tests / callers that already have `&self`).
    pub fn renew_token(&self, new_token: &str) -> Result<(), AgoraError> {
        renew_token_inner(self.conn, new_token)
    }

    /// Optional second event sender for the renew task. Forwarded into
    /// `observer::set_renew_sender`. Called by main.rs once during startup
    /// when `--token-renew-cmd` is set.
    pub fn set_renew_sender(&self, tx: UnboundedSender<ConnEvent>) {
        super::observer::set_renew_sender(tx);
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
            // unregister_observer returns i32; per SDK docs it doesn't fail, and
            // even if it did there's nothing meaningful Drop could do — explicit
            // discard for consistency with how disconnect/release returns are
            // inspected above.
            let _ = sys::agora_rtc_conn_unregister_observer(self.conn);
            sys::agora_rtc_conn_destroy(self.conn);
            sys::agora_media_node_factory_destroy(self.factory);
            let rc = sys::agora_service_release(self.svc);
            if rc != 0 {
                eprintln!("warning: agora_service_release returned {rc}");
            }
        }
        observer::clear_event_sender();
    }
}
