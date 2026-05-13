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
