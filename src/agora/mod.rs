//! Safe Rust wrapper over the Agora NG SDK's flat C API.
mod sys;
mod error;
mod observer;
mod session;
mod publisher;
pub mod video;
pub mod audio;

// AgoraError is the error type returned by Session::connect and Session::run.
// It's re-exported for callers that want to match on it directly; the binary
// currently propagates via `?` into `anyhow::Error`, so the import shows as
// unused inside the crate.
#[allow(unused_imports)]
pub use error::AgoraError;
pub use observer::ConnEvent;
pub use publisher::{decide, AudioPublisher, CodecMode, VideoPublisher};
pub use session::{Session, SessionConfig};
