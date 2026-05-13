//! Safe Rust wrapper over the Agora NG SDK's flat C API.
mod sys;

// Re-exported as the later tasks add them:
//   mod error;     pub use error::AgoraError;
//   mod observer;
//   mod session;   pub use session::{Session, SessionConfig};
