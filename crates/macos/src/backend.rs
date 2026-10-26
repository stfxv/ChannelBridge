//! Native control backend for the selected deployment virtual microphone.
use crate::{
    devices,
    health::StreamHealth,
    rate::{RateFollow, Status},
    session::TestSession,
};
use channelbridge_audio::routing::{Mode, Selection};
use channelbridge_core::{Config, controller::Backend, storage};
use channelbridge_hal_abi::{StreamFormat, fourcc};
use std::{path::PathBuf, time::Instant};
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioGetHostClockFrequency() -> f64;
    fn AudioGetCurrentHostTime() -> u64;
}
pub struct NativeBackend {
    startup_failed: bool,
    retained_io_contexts: u64,
    settings: PathBuf,
    ticks_per_second: u64,
    stream_health: StreamHealth,
}
impl NativeBackend {
    pub fn try_new(settings: PathBuf) -> Result<Self, String> {
        let frequency = unsafe { AudioGetHostClockFrequency() };
        if !frequency.is_finite()
            || frequency <= 0.0
            || frequency.fract() != 0.0
            || frequency >= u64::MAX as f64
        {
            return Err("unsupported host clock frequency".into());
        }
        Ok(Self {
            startup_failed: false,
            retained_io_contexts: 0,
            settings,
            ticks_per_second: frequency as u64,
            stream_health: StreamHealth::default(),
        })
    }
}
/// UI entry point. The native backend is constructed inside the control thread.
pub fn spawn_worker(
    settings: PathBuf,
    config: Config,
) -> std::io::Result<channelbridge_core::worker::Worker> {
    channelbridge_core::worker::Worker::spawn(config, move || NativeBackend::try_new(settings))
}

fn project_inventory(
    entries: Result<Vec<Result<devices::Device, devices::Error>>, devices::Error>,
    mut read_streams: impl FnMut(
        channelbridge_hal_abi::ObjectId,
    ) -> Result<Vec<devices::Stream>, devices::Error>,
) -> Result<channelbridge_core::view::DeviceInventory, String> {
    use channelbridge_core::view::{DeviceChoice, InputFormat};

    let entries = entries.map_err(|error| error.to_string())?;
    let mut choices = Vec::new();
    let mut failed_entries = 0usize;
    let mut first_error = None;
    for entry in entries {
        let device = match entry {
            Ok(device) => device,
            Err(error) => {
                failed_entries += 1;
                first_error.get_or_insert_with(|| error.to_string());
                continue;
            }
        };
        if device.input_channels == 0
            || [
                channelbridge_contract::TEST_DEVICE_UID,
                channelbridge_contract::PRODUCTION_DEVICE_UID,
            ]
            .contains(&device.uid.as_str())
        {
            continue;
        }
        let (physical_inputs, format_error) = match read_streams(device.id) {
            Ok(streams) => (
                streams
                    .into_iter()
                    .map(|stream| {
                        let format = stream.physical_format;
                        InputFormat {
                            channels: format.channels_per_frame,
                            rate: format.sample_rate,
                            bits: format.bits_per_channel,
                            float: format.format_flags & 1 != 0,
                        }
                    })
                    .collect(),
                None,
            ),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        choices.push(DeviceChoice {
            uid: device.uid,
            name: device.name,
            channels: device.input_channels,
            rate: device.rate,
            physical_inputs,
            format_error,
        });
    }
    Ok(channelbridge_core::view::DeviceInventory {
        choices,
        completeness: match first_error {
            Some(error) => channelbridge_core::view::InventoryCompleteness::Partial {
                warning: format!("{failed_entries} device entries could not be read: {error}"),
            },
            None => channelbridge_core::view::InventoryCompleteness::Complete,
        },
    })
}

fn selection(config: &Config) -> Result<Selection, String> {
    let mode = match config.mode {
        channelbridge_core::Mode::Mono => Mode::Mono,
        channelbridge_core::Mode::Stereo => Mode::Stereo,
        channelbridge_core::Mode::Mix => Mode::Mix,
    };
    Selection::new(config.left, config.right, mode, config.muted)
        .ok_or("invalid input selection".into())
}
fn same_format(a: &StreamFormat, b: &StreamFormat) -> bool {
    a.sample_rate == b.sample_rate
        && a.format_id == b.format_id
        && a.format_flags == b.format_flags
        && a.bytes_per_packet == b.bytes_per_packet
        && a.frames_per_packet == b.frames_per_packet
        && a.bytes_per_frame == b.bytes_per_frame
        && a.channels_per_frame == b.channels_per_frame
        && a.bits_per_channel == b.bits_per_channel
}
fn compatible_driver() -> Result<(), String> {
    let counters = crate::diagnostics::read_target_driver()
        .map_err(|error| format!("仮想マイクの互換性を確認できません: {error}"))?;
    initialized_driver(counters)
}
fn initialized_driver(counters: channelbridge_contract::DriverCounters) -> Result<(), String> {
    // Decoding already rejects unsupported protocol/size/reserved fields.
    // Generation zero is the driver's uninitialized diagnostic fallback.
    if counters.generation == 0 {
        Err("仮想マイクの初期化が完了していません".into())
    } else {
        Ok(())
    }
}
impl Backend for NativeBackend {
    type Pending = RateFollow;
    type Session = TestSession;
    fn requested_rate(&self, pending: &RateFollow) -> Option<u32> {
        Some(pending.desired_rate())
    }
    fn driver_diagnostics(&self) -> Option<channelbridge_core::view::DriverReading> {
        use channelbridge_core::view::DriverReading;
        Some(match crate::diagnostics::read_target_driver() {
            Ok(counters) => {
                let identity = crate::diagnostics::read_target_driver_identity();
                let (version, identity_error) = match identity {
                    Ok(version) => (Some(version), None),
                    Err(error) => (None, Some(error.to_string())),
                };
                DriverReading::Available {
                    diagnostic_protocol: channelbridge_contract::PROTOCOL_VERSION,
                    loaded_driver_package_version: version,
                    identity_error,
                    generation: counters.generation,
                    written_frames: counters.written_frames,
                    missing_read_frames: counters.missing_read_frames,
                    rejected_write_frames: counters.rejected_write_frames,
                    invalid_timestamps: counters.invalid_timestamps,
                }
            }
            Err(error) => DriverReading::Unavailable {
                error: error.to_string(),
            },
        })
    }
    fn diagnostics(
        &self,
        session: &TestSession,
    ) -> Option<channelbridge_core::view::AudioDiagnostics> {
        let (capture, output) = session.stats();
        Some(channelbridge_core::view::AudioDiagnostics {
            capture_callbacks: capture.callbacks,
            capture_rejected: capture.rejected,
            capture_written_frames: capture.written,
            capture_overflow_frames: capture.overflow,
            capture_invalid_clock: capture.invalid_clock,
            capture_discontinuities: capture.discontinuities,
            capture_unavailable_channel: capture.unavailable_channel,
            capture_clock_generation: capture.clock_generation,
            observed_source_speed_ppm: capture
                .source_speed
                .map(|speed| (speed - 1.0) * 1_000_000.0),
            capture_failed: capture.failed,
            output_callbacks: output.callbacks,
            output_rejected: output.rejected,
            output_missing_frames: output.missing,
            output_priming_callbacks: output.priming_callbacks,
            output_dropped_lookahead_frames: output.dropped_lookahead,
            output_rendered_frames: output.rendered_frames,
            fifo_current_frames: output.fifo_current,
            fifo_min_frames: output.fifo_min,
            fifo_max_frames: output.fifo_max,
            buffered_input_target_frames: output.buffered_target,
            correction_ppm: output.correction_ppm,
            output_peak_dbfs: output
                .peak
                .map(|peak| peak.map(channelbridge_core::view::dbfs)),
            output_failed: output.failed,
        })
    }
    fn devices(&mut self) -> Result<channelbridge_core::view::DeviceInventory, String> {
        project_inventory(devices::list(), |id| devices::streams(id, fourcc(b"inpt")))
    }

    fn audio_view(
        &self,
        session: &TestSession,
        config: &Config,
        overview: bool,
    ) -> Option<channelbridge_core::view::AudioView> {
        use channelbridge_core::view::{AudioView, InputFormat, InputLevel, dbfs};
        let selected = selection(config).ok()?;
        let channels = session
            .source_streams
            .iter()
            .map(|stream| stream.virtual_format.channels_per_frame as usize)
            .sum::<usize>();
        let indices = if overview {
            (0..channels).collect::<Vec<_>>()
        } else if selected.left() == selected.right() {
            vec![selected.left()]
        } else {
            vec![selected.left(), selected.right()]
        };
        let now = unsafe { AudioGetCurrentHostTime() };
        let levels = indices
            .into_iter()
            .filter(|channel| *channel < channels)
            .map(|channel| {
                let level = session.meters.level(channel, now);
                InputLevel {
                    channel: channel as u32 + 1,
                    fresh: level.fresh,
                    current_dbfs: dbfs(level.current),
                    held_dbfs: dbfs(level.held_peak),
                }
            })
            .collect();
        let physical_inputs = session
            .source_streams
            .iter()
            .map(|stream| {
                let format = stream.physical_format;
                InputFormat {
                    channels: format.channels_per_frame,
                    rate: format.sample_rate,
                    bits: format.bits_per_channel,
                    float: format.format_flags & 1 != 0,
                }
            })
            .collect();
        Some(AudioView {
            physical_inputs,
            virtual_rate: session.rate,
            virtual_bits: 32,
            levels,
        })
    }
    fn save(&mut self, config: &Config) -> Result<(), String> {
        selection(config)?;
        storage::save(&self.settings, config).map_err(|error| error.to_string())
    }
    fn negotiate(&mut self, config: &Config) -> Result<RateFollow, String> {
        if !crate::permission::status().allowed() {
            return Err("microphone permission is not authorized".into());
        }
        selection(config)?;
        compatible_driver()?;
        let desired =
            RateFollow::source_rate(&config.device_uid).map_err(|error| error.to_string())?;
        RateFollow::request_target(desired)
            .map_err(|error| format!("仮想マイクの{desired} Hzへの交渉に失敗しました: {error}"))
    }
    fn ready(&mut self, pending: &RateFollow) -> Result<bool, String> {
        pending
            .poll()
            .map(|status| status == Status::Ready)
            .map_err(|error| error.to_string())
    }
    fn open(&mut self, config: &Config) -> Result<TestSession, String> {
        self.startup_failed = false;
        if !crate::permission::status().allowed() {
            return Err("microphone permission is not authorized".into());
        }
        // Check again after negotiation, before registering either IOProc.
        compatible_driver()?;
        let session = match TestSession::preview(
            &config.device_uid,
            selection(config)?,
            self.ticks_per_second,
        ) {
            Ok(session) => session,
            Err(error) => {
                self.startup_failed = error
                    .downcast_ref::<crate::session::StartupError>()
                    .is_some_and(|error| error.requires_stop)
                    || error
                        .downcast_ref::<crate::io::IoError>()
                        .is_some_and(|error| error.context_retained);
                self.retained_io_contexts += error
                    .downcast_ref::<crate::session::StartupError>()
                    .map_or_else(
                        || {
                            error
                                .downcast_ref::<crate::io::IoError>()
                                .map_or(0, |error| u64::from(error.context_retained))
                        },
                        |error| error.retained_io_contexts,
                    );
                return Err(error.to_string());
            }
        };
        self.stream_health.started(Instant::now());
        Ok(session)
    }
    fn healthy(&mut self, session: &TestSession, config: &Config) -> Result<bool, String> {
        if !crate::permission::status().allowed() {
            return Err("microphone permission was revoked".into());
        }
        let devices = devices::list().map_err(|error| error.to_string())?;
        let source = devices
            .iter()
            .filter_map(|entry| entry.as_ref().ok())
            .find(|device| device.uid == config.device_uid);
        let target = devices
            .iter()
            .filter_map(|entry| entry.as_ref().ok())
            .find(|device| device.uid == channelbridge_contract::DEPLOYMENT.device_uid);
        let (Some(source), Some(target)) = (source, target) else {
            return Ok(false);
        };
        if source.id != session.source_id
            || target.id != session.target_id
            || source.rate != f64::from(session.rate)
            || target.rate != source.rate
        {
            return Ok(false);
        }
        config
            .validate_available(source.input_channels)
            .map_err(|error| format!("{error:?}"))?;
        let streams =
            devices::streams(source.id, fourcc(b"inpt")).map_err(|error| error.to_string())?;
        if streams.len() != session.source_streams.len()
            || !streams.iter().zip(&session.source_streams).all(|(a, b)| {
                a.id == b.id
                    && same_format(&a.virtual_format, &b.virtual_format)
                    && same_format(&a.physical_format, &b.physical_format)
            })
        {
            return Ok(false);
        }
        let mut virtual_streams =
            devices::streams(target.id, fourcc(b"inpt")).map_err(|error| error.to_string())?;
        virtual_streams.extend(
            devices::streams(target.id, fourcc(b"outp")).map_err(|error| error.to_string())?,
        );
        if virtual_streams.len() != session.virtual_streams.len()
            || !virtual_streams
                .iter()
                .zip(&session.virtual_streams)
                .all(|(a, b)| {
                    a.id == b.id
                        && same_format(&a.virtual_format, &b.virtual_format)
                        && same_format(&a.physical_format, &b.physical_format)
                })
        {
            return Ok(false);
        }
        let (capture, output) = session.stats();
        self.stream_health.check(
            capture,
            output,
            session.routing.load().sending,
            Instant::now(),
        )
    }
    fn route(
        &mut self,
        session: &TestSession,
        config: &Config,
        sending: bool,
        overview: bool,
    ) -> Result<(), String> {
        let channels = session
            .source_streams
            .iter()
            .map(|stream| stream.virtual_format.channels_per_frame)
            .sum();
        config
            .validate_available(channels)
            .map_err(|error| format!("{error:?}"))?;
        session
            .routing
            .configure(selection(config)?, sending, overview)
            .ok_or("routing generation changed concurrently")?;
        self.stream_health
            .routed(sending, session.stats().1.rendered_frames, Instant::now());
        Ok(())
    }
    fn open_failure_requires_stop(&self) -> bool {
        self.startup_failed
    }
    fn retained_io_contexts(&self) -> u64 {
        self.retained_io_contexts
    }
    fn close(&mut self, session: TestSession) -> Result<(), String> {
        self.stream_health.reset();
        match session.close() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.retained_io_contexts += error.retained_io_contexts();
                if error.requires_stop() {
                    Err(error.to_string())
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(uid: &str) -> devices::Device {
        devices::Device {
            id: 42,
            uid: uid.into(),
            name: uid.into(),
            rate: 48000.0,
            input_channels: 2,
            output_channels: 0,
        }
    }
    #[test]
    fn inventory_retains_inputs_before_and_after_entry_failures() {
        use channelbridge_core::view::InventoryCompleteness;
        for entries in [
            vec![Ok(input("input")), Err(devices::Error::Hal(-1))],
            vec![Err(devices::Error::Hal(-1)), Ok(input("input"))],
        ] {
            let inventory = project_inventory(Ok(entries), |_| Ok(Vec::new())).unwrap();
            assert_eq!(inventory.choices.len(), 1);
            assert_eq!(inventory.choices[0].uid, "input");
            assert!(matches!(
                inventory.completeness,
                InventoryCompleteness::Partial { .. }
            ));
        }
    }
    #[test]
    fn inventory_all_entry_failures_have_one_bounded_warning() {
        use channelbridge_core::view::InventoryCompleteness;
        let inventory = project_inventory(
            Ok((0..100).map(|_| Err(devices::Error::Hal(-7))).collect()),
            |_| panic!("failed entries have no stream query"),
        )
        .unwrap();
        assert!(inventory.choices.is_empty());
        let InventoryCompleteness::Partial { warning } = inventory.completeness else {
            panic!("entry errors must mark inventory partial");
        };
        assert!(warning.contains("CoreAudio status -7"));
        assert!(warning.contains("100"));
        assert!(warning.len() < 200);
    }
    #[test]
    fn inventory_whole_list_failure_remains_an_error() {
        let error = project_inventory(Err(devices::Error::Hal(-8)), |_| {
            panic!("failed list has no stream query")
        })
        .unwrap_err();
        assert_eq!(error, "CoreAudio status -8");
    }
    #[test]
    fn inventory_stream_failure_keeps_definite_input_presence() {
        use channelbridge_core::view::InventoryCompleteness;
        let inventory = project_inventory(Ok(vec![Ok(input("input"))]), |_| {
            Err(devices::Error::Hal(-9))
        })
        .unwrap();
        assert_eq!(inventory.completeness, InventoryCompleteness::Complete);
        assert_eq!(inventory.choices[0].uid, "input");
        assert!(inventory.choices[0].physical_inputs.is_empty());
        assert_eq!(
            inventory.choices[0].format_error.as_deref(),
            Some("CoreAudio status -9")
        );
    }
    #[test]
    fn driver_initialization_requires_a_nonzero_generation_not_audio_activity() {
        use channelbridge_contract::DriverCounters;
        let counters = DriverCounters {
            generation: 0,
            written_frames: 0,
            missing_read_frames: 0,
            rejected_write_frames: 0,
            invalid_timestamps: 0,
        };
        assert!(initialized_driver(counters).is_err());
        // A newly initialized device with no writer is compatible. Silence
        // and cumulative missing reads must not prevent recovery or startup.
        assert!(
            initialized_driver(DriverCounters {
                generation: 1,
                missing_read_frames: u64::MAX,
                ..counters
            })
            .is_ok()
        );
    }
    #[test]
    fn native_policy_keeps_saved_mono_right_and_detects_same_rate_format_changes() {
        let config = Config {
            device_uid: "source".into(),
            left: 2,
            right: 16,
            ..Config::default()
        };
        let selected = selection(&config).unwrap();
        assert_eq!((selected.left(), selected.right()), (1, 1));
        assert_eq!(config.right, 16);
        let original = StreamFormat {
            sample_rate: 48000.0,
            channels_per_frame: 10,
            bits_per_channel: 24,
            ..Default::default()
        };
        let changed = StreamFormat {
            channels_per_frame: 2,
            ..original
        };
        assert!(!same_format(&original, &changed));
        let changed = StreamFormat {
            bits_per_channel: 32,
            ..original
        };
        assert!(!same_format(&original, &changed));
    }
}
