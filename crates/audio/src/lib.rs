//! Audio algorithms with no macOS API dependencies.
//! All callback-facing data is allocated before I/O starts.
#![forbid(unsafe_code)]

#[cfg(not(target_has_atomic = "64"))]
compile_error!("ChannelBridge requires native 64-bit atomics (arm64 or x86_64 macOS).");

pub mod capture;
pub mod clock;
pub mod drift;
pub mod fifo;
pub mod frame;
pub mod meters;
pub mod output;
pub mod resample;
pub mod ring;
pub mod routing;

pub use frame::{FramePosition, StereoFrame};
