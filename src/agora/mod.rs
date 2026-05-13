//! Safe Rust wrapper over the Agora NG SDK's flat C API.
mod sys;
mod error;

pub use error::AgoraError;

// Re-exported as the later tasks add them:
//   mod observer;
//   mod session;   pub use session::{Session, SessionConfig};
