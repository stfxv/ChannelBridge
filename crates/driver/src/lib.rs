//! New Rust virtual driver, independent of BlackHole.
//! This crate must never call CoreAudio's client HAL APIs.
mod driver_audio;
pub use driver_audio::DriverAudio;

#[cfg(target_os = "macos")]
mod plugin;

pub(crate) fn kernel() -> Option<DriverAudio> {
    DriverAudio::new(65_536)
}
