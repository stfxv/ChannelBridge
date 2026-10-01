//! HAL client operations. This crate must never be linked into the HAL driver.
#[cfg(target_os = "macos")]
pub mod buffers;
#[cfg(target_os = "macos")]
pub mod devices;
#[cfg(target_os = "macos")]
pub mod loopback;
