//! Audio algorithms with no macOS API dependencies.
//! All callback-facing data is allocated before I/O starts.
#![forbid(unsafe_code)]

#[cfg(not(target_has_atomic = "64"))]
compile_error!("ChannelBridge requires native 64-bit atomics.");

pub mod capture_processor;
pub mod device_clock;
pub mod drift;
pub mod fifo;
pub mod frame;
pub mod meters;
pub mod output_processor;
pub mod resample;
pub mod routing;
pub mod timeline_ring;

pub use frame::{FramePosition, StereoFrame};
