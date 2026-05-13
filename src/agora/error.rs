//! Mapping Agora C API integer return/error codes to legible messages,
//! plus the `AgoraError` type the safe layer returns.

use std::fmt;

/// `ERROR_CODE_TYPE` names from the SDK's `AgoraBase.h`. Returns `None`
/// for codes we don't have a name for (still rendered numerically).
pub fn error_name(code: i32) -> Option<&'static str> {
    Some(match code {
        0 => "ERR_OK",
        1 => "ERR_FAILED",
        2 => "ERR_INVALID_ARGUMENT",
        3 => "ERR_NOT_READY",
        4 => "ERR_NOT_SUPPORTED",
        5 => "ERR_REFUSED",
        7 => "ERR_NOT_INITIALIZED",
        8 => "ERR_INVALID_STATE",
        10 => "ERR_TIMEDOUT",
        17 => "ERR_JOIN_CHANNEL_REJECTED",
        18 => "ERR_LEAVE_CHANNEL_REJECTED",
        101 => "ERR_INVALID_APP_ID",
        102 => "ERR_INVALID_CHANNEL_NAME",
        109 => "ERR_TOKEN_EXPIRED",
        110 => "ERR_INVALID_TOKEN",
        _ => return None,
    })
}

/// An error from the Agora C API: a failing return code, a NULL handle,
/// or a fatal observer callback.
#[derive(Debug, Clone)]
pub struct AgoraError {
    pub context: String,
    pub code: Option<i32>,
    pub detail: Option<String>,
}

impl AgoraError {
    pub fn code(context: impl Into<String>, code: i32) -> Self {
        AgoraError { context: context.into(), code: Some(code), detail: None }
    }
    pub fn null(context: impl Into<String>) -> Self {
        AgoraError { context: context.into(), code: None, detail: Some("returned NULL".into()) }
    }
    pub fn msg(context: impl Into<String>, detail: impl Into<String>) -> Self {
        AgoraError { context: context.into(), code: None, detail: Some(detail.into()) }
    }
}

impl fmt::Display for AgoraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.context)?;
        if let Some(c) = self.code {
            match error_name(c) {
                Some(name) => write!(f, " (code {c}: {name})")?,
                None => write!(f, " (code {c})")?,
            }
        }
        if let Some(d) = &self.detail {
            write!(f, ": {d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AgoraError {}

/// Convert a C API integer return into a `Result`. 0 = success.
pub fn check(code: i32, context: &str) -> Result<(), AgoraError> {
    if code == 0 { Ok(()) } else { Err(AgoraError::code(context, code)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_name_known_codes() {
        assert_eq!(error_name(0), Some("ERR_OK"));
        assert_eq!(error_name(110), Some("ERR_INVALID_TOKEN"));
        assert_eq!(error_name(109), Some("ERR_TOKEN_EXPIRED"));
        assert_eq!(error_name(101), Some("ERR_INVALID_APP_ID"));
        assert_eq!(error_name(424242), None);
    }

    #[test]
    fn check_ok_and_err() {
        assert!(check(0, "ctx").is_ok());
        let e = check(110, "agora_rtc_conn_connect").unwrap_err();
        assert_eq!(e.code, Some(110));
        assert!(e.to_string().contains("agora_rtc_conn_connect"));
        assert!(e.to_string().contains("ERR_INVALID_TOKEN"));
    }

    #[test]
    fn display_includes_context_and_detail() {
        let e = AgoraError::msg("connect", "timed out after 10s");
        assert_eq!(e.to_string(), "connect: timed out after 10s");
        let e = AgoraError::null("agora_service_create");
        assert_eq!(e.to_string(), "agora_service_create: returned NULL");
    }
}
