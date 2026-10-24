//! HAL client operations. This crate must never be linked into the HAL driver.
#[cfg(target_os = "macos")]
pub mod buffers;
#[cfg(target_os = "macos")]
pub mod devices;
#[cfg(target_os = "macos")]
pub mod loopback;

#[cfg(target_os = "macos")]
pub mod monitor;

#[cfg(target_os = "macos")]
pub mod diagnostics;

#[cfg(target_os = "macos")]
pub mod capture;

#[cfg(target_os = "macos")]
pub mod output;

#[cfg(target_os = "macos")]
pub mod io;

#[cfg(target_os = "macos")]
pub mod source_clock;

#[cfg(target_os = "macos")]
pub mod session;

#[cfg(target_os = "macos")]
pub mod rate;

#[cfg(target_os = "macos")]
pub mod backend;

#[cfg(target_os = "macos")]
pub(crate) mod health;

#[cfg(target_os = "macos")]
pub mod permission;
