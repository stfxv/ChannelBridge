//! Audio algorithms with no macOS API dependencies.
//! All callback-facing data is allocated before I/O starts.
#![forbid(unsafe_code)]

#[cfg(not(target_has_atomic = "64"))]
compile_error!("ChannelBridge requires native 64-bit atomics (arm64 or x86_64 macOS).");

pub mod clock;
pub mod fifo;
pub mod frame;
pub mod ring;

pub use frame::{FramePosition, StereoFrame};
