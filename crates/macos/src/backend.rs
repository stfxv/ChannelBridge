//! Native control backend for the separate development virtual microphone.
use crate::{
    devices,
    rate::{RateFollow, Status},
    session::TestSession,
};
use channelbridge_audio::routing::{Mode, Selection};
use channelbridge_core::{Config, controller::Backend, storage};
use channelbridge_hal_abi::{StreamFormat, fourcc};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioGetHostClockFrequency() -> f64;
}
pub struct NativeBackend {
    startup_failed: bool,
    settings: PathBuf,
    ticks_per_second: u64,
    activity: Option<((u64, u64), Instant)>,
}
impl NativeBackend {
    pub fn new(settings: PathBuf) -> Result<Self, String> {
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
            settings,
            ticks_per_second: frequency as u64,
            activity: None,
        })
    }
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
impl Backend for NativeBackend {
    type Pending = RateFollow;
    type Session = TestSession;
    fn save(&mut self, config: &Config) -> Result<(), String> {
        selection(config)?;
        storage::save(&self.settings, config).map_err(|error| error.to_string())
    }
    fn negotiate(&mut self, config: &Config) -> Result<RateFollow, String> {
        selection(config)?;
        RateFollow::request_for_source(&config.device_uid).map_err(|error| error.to_string())
    }
    fn ready(&mut self, pending: &RateFollow) -> Result<bool, String> {
        pending
            .poll()
            .map(|status| status == Status::Ready)
            .map_err(|error| error.to_string())
    }
    fn open(&mut self, config: &Config) -> Result<TestSession, String> {
        self.startup_failed = true;
        let session = TestSession::preview(
            &config.device_uid,
            selection(config)?,
            self.ticks_per_second,
        )
        .map_err(|error| error.to_string())?;
        self.startup_failed = false;
        self.activity = Some(((0, 0), Instant::now()));
        Ok(session)
    }
    fn healthy(&mut self, session: &TestSession, config: &Config) -> Result<bool, String> {
        let devices = devices::list().map_err(|error| error.to_string())?;
        let source = devices
            .iter()
            .filter_map(|entry| entry.as_ref().ok())
            .find(|device| device.uid == config.device_uid);
        let target = devices
            .iter()
            .filter_map(|entry| entry.as_ref().ok())
            .find(|device| device.uid == channelbridge_contract::TEST_DEVICE_UID);
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
        if capture.failed || output.failed {
            return Err("audio callback failed".into());
        }
        let counts = (capture.callbacks, output.callbacks);
        if let Some((previous, time)) = self.activity {
            if (counts.0 == previous.0 || counts.1 == previous.1)
                && time.elapsed() >= Duration::from_secs(2)
            {
                return Ok(false);
            }
            if counts.0 != previous.0 && counts.1 != previous.1 {
                self.activity = Some((counts, Instant::now()));
            }
        } else {
            self.activity = Some((counts, Instant::now()));
        }
        Ok(true)
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
        Ok(())
    }
    fn open_failure_requires_stop(&self) -> bool {
        self.startup_failed
    }
    fn close(&mut self, session: TestSession) -> Result<(), String> {
        self.activity = None;
        session.close().map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
