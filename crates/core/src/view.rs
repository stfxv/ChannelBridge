//! Owned display data, with no HAL objects or callback pointers.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ControlEvent {
    /// None for UI commands without a worker tick timestamp.
    pub worker_ms: Option<u64>,
    pub kind: ControlEventKind,
    pub detail: Option<String>,
    pub requested_rate_hz: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlEventKind {
    AutomaticSendRequested,
    InputReconnected,
    SendStarted,
    SendStopped,
    NegotiationStarted,
    NegotiationRequested,
    NegotiationReady,
    SessionOpened,
    RetryScheduled,
    TeardownFailed,
    PermissionRevoked,
    DeviceChanged,
    RouteFailed,
    SystemSleep,
    SystemWake,
    ScreensWake,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DriverDiagnostics {
    /// Monotonic time since control worker startup, not a wall-clock timestamp.
    pub observed_worker_ms: u64,
    pub reading: DriverReading,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DriverReading {
    Available {
        diagnostic_protocol: u32,
        /// Only an identity returned by the live driver may populate this.
        loaded_driver_package_version: Option<String>,
        identity_error: Option<String>,
        generation: u64,
        written_frames: u64,
        missing_read_frames: u64,
        rejected_write_frames: u64,
        invalid_timestamps: u64,
    },
    Unavailable {
        error: String,
    },
}
/// Session-local cumulative counters, read independently rather than as one
/// synchronized audio epoch. No samples, device UID or user paths are included.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct AudioDiagnostics {
    pub capture_callbacks: u64,
    pub capture_rejected: u64,
    pub capture_written_frames: u64,
    pub capture_overflow_frames: u64,
    pub capture_invalid_clock: u64,
    pub capture_discontinuities: u64,
    pub capture_unavailable_channel: u64,
    pub capture_clock_generation: Option<u64>,
    pub observed_source_speed_ppm: Option<f64>,
    pub capture_failed: bool,
    pub output_callbacks: u64,
    pub output_rejected: u64,
    pub output_missing_frames: u64,
    pub output_priming_callbacks: u64,
    pub output_dropped_lookahead_frames: u64,
    pub output_rendered_frames: u64,
    pub fifo_current_frames: Option<u64>,
    pub fifo_min_frames: Option<u64>,
    pub fifo_max_frames: Option<u64>,
    pub buffered_input_target_frames: u64,
    pub correction_ppm: Option<f64>,
    /// Latest accepted output block after fades/mute. None means unobserved or invalid;
    /// [None, None] inside Some represents known stereo silence (-infinity).
    pub output_peak_dbfs: Option<[Option<f32>; 2]>,
    pub output_failed: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceInventory {
    pub choices: Vec<DeviceChoice>,
    pub completeness: InventoryCompleteness,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum InventoryCompleteness {
    #[default]
    Complete,
    Partial {
        warning: String,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceChoice {
    pub uid: String,
    pub name: String,
    pub channels: u32,
    pub rate: f64,
    pub physical_inputs: Vec<InputFormat>,
    pub format_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct InputFormat {
    pub channels: u32,
    pub rate: f64,
    pub bits: u32,
    pub float: bool,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct InputLevel {
    pub channel: u32,
    pub fresh: bool,
    pub current_dbfs: Option<f32>,
    pub held_dbfs: Option<f32>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AudioView {
    pub physical_inputs: Vec<InputFormat>,
    pub virtual_rate: u32,
    pub virtual_bits: u32,
    pub levels: Vec<InputLevel>,
}
#[must_use]
pub fn dbfs(amplitude: f32) -> Option<f32> {
    (amplitude.is_finite() && amplitude > 0.0).then(|| 20.0 * amplitude.log10())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silence_is_distinct_from_zero_dbfs_and_invalid_levels_are_not_displayed() {
        assert_eq!(dbfs(0.0), None);
        assert_eq!(dbfs(1.0), Some(0.0));
        assert!(
            (dbfs(0.5).expect("a positive finite amplitude has a decibel representation") + 6.0206)
                .abs()
                < 0.0001
        );
        assert_eq!(dbfs(f32::NAN), None);
        assert_eq!(dbfs(f32::INFINITY), None);
    }
}
