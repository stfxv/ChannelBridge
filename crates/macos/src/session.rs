//! Prepare all audio ownership before HAL registration. No I/O starts here.
use crate::{
    buffers::CaptureLayout,
    capture::CaptureCallback,
    devices::{self, Stream},
    output::OutputCallback,
};
use channelbridge_audio::{
    capture::CaptureProcessor,
    fifo::stereo_queue,
    meters::{MeterReader, meter_bank},
    output::OutputProcessor,
    routing::{AtomicRouting, Selection},
};
use channelbridge_hal_abi::{ObjectId, StreamFormat, fourcc};
use std::sync::Arc;

pub struct PreparedSession {
    pub capture: CaptureCallback,
    pub output: OutputCallback,
    pub routing: Arc<AtomicRouting>,
    pub meters: MeterReader,
    pub source_streams: Vec<Stream>,
    pub input_rate: u32,
    pub output_rate: u32,
}
impl PreparedSession {
    /// Caller chooses device IDs by explicit UID and checks they remain valid.
    /// Rate-follow negotiation and per-IOProc stream disabling precede Start.
    pub fn prepare(
        source: ObjectId,
        target: ObjectId,
        selection: Selection,
        ticks_per_second: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if source == target {
            return Err("input and virtual output must be different devices".into());
        }
        let source_streams = devices::streams(source, fourcc(b"inpt"))?;
        let target_streams = devices::streams(target, fourcc(b"outp"))?;
        let input = source_streams
            .iter()
            .map(|stream| stream.virtual_format)
            .collect::<Vec<_>>();
        let output = target_streams
            .iter()
            .map(|stream| stream.virtual_format)
            .collect::<Vec<_>>();
        let mut session = Self::from_formats(&input, &output, selection, ticks_per_second)?;
        session.source_streams = source_streams;
        Ok(session)
    }
    fn from_formats(
        input: &[StreamFormat],
        output: &[StreamFormat],
        selection: Selection,
        ticks_per_second: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let layout = CaptureLayout::try_new(input)?;
        if !selection.available(layout.channels()) {
            return Err("selected input channel is unavailable".into());
        }
        let [format] = output else {
            return Err("virtual output must have one stereo stream".into());
        };
        if format.format_id != fourcc(b"lpcm")
            || format.format_flags != 9
            || format.bits_per_channel != 32
            || format.channels_per_frame != 2
            || format.bytes_per_frame != 8
            || format.bytes_per_packet != 8
            || format.frames_per_packet != 1
        {
            return Err("virtual output must be native interleaved float32 stereo".into());
        }
        let rate = |value: f64| -> Result<u32, Box<dyn std::error::Error>> {
            if !value.is_finite() || value.fract() != 0.0 || !(8000.0..=768000.0).contains(&value) {
                return Err("unsupported audio rate".into());
            }
            Ok(value as u32)
        };
        let input_rate = rate(layout.sample_rate())?;
        let output_rate = rate(format.sample_rate)?;
        let routing = Arc::new(AtomicRouting::new(selection));
        let (writer, meters) = meter_bank(layout.channels(), ticks_per_second)
            .ok_or("cannot allocate input meters")?;
        // A provisional 16 ms input target. Actual latency/jitter margins must be
        // measured, including driver's latency and filter history.
        let target = (input_rate as usize * 16 / 1000).max(256);
        let capacity = (target + 8192).next_power_of_two();
        let (producer, consumer) = stereo_queue(capacity).ok_or("cannot allocate input queue")?;
        let processor = CaptureProcessor::new(
            Arc::clone(&routing),
            writer,
            producer,
            input_rate,
            ticks_per_second,
        )
        .ok_or("cannot prepare capture")?;
        let capture = CaptureCallback::new(layout, processor);
        let processor = OutputProcessor::new(consumer, input_rate, output_rate, target)
            .ok_or("cannot prepare rate conversion")?;
        let output = OutputCallback::new(processor, Arc::clone(&routing))
            .ok_or("cannot allocate output storage")?
            .with_source_clock(capture.source_clock());
        Ok(Self {
            capture,
            output,
            routing,
            meters,
            source_streams: Vec::new(),
            input_rate,
            output_rate,
        })
    }
}
/// Development session, restricted to the separate Rust test device UID.
/// Both IOProcs keep running for preview/stop; sending is controlled atomically.
pub struct TestSession {
    pub(crate) source_id: ObjectId,
    pub(crate) target_id: ObjectId,
    pub(crate) rate: u32,
    pub(crate) virtual_streams: Vec<Stream>,
    capture: crate::io::Registration<CaptureCallback>,
    output: crate::io::Registration<OutputCallback>,
    pub routing: Arc<AtomicRouting>,
    pub meters: MeterReader,
    pub source_streams: Vec<Stream>,
}
impl TestSession {
    pub fn preview(
        source_uid: &str,
        selection: Selection,
        ticks_per_second: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let devices = devices::list()?;
        let find = |uid: &str| -> Result<ObjectId, Box<dyn std::error::Error>> {
            let mut matching = devices
                .iter()
                .filter_map(|entry| entry.as_ref().ok())
                .filter(|device| device.uid == uid);
            let device = matching.next().ok_or("selected device is unavailable")?;
            if matching.next().is_some() {
                return Err("duplicate device UID".into());
            }
            Ok(device.id)
        };
        let source = find(source_uid)?;
        let target = find(channelbridge_contract::DEPLOYMENT.device_uid)?;
        let prepared = PreparedSession::prepare(source, target, selection, ticks_per_second)?;
        if prepared.input_rate != prepared.output_rate {
            return Err("virtual rate must follow source before starting preview".into());
        }
        let mut virtual_streams = devices::streams(target, fourcc(b"inpt"))?;
        virtual_streams.extend(devices::streams(target, fourcc(b"outp"))?);
        let mut capture = crate::io::Registration::try_new(source, prepared.capture)?;
        if let Err(error) = capture.disable_streams(crate::io::Direction::Output) {
            return Err(failed_start(error, Some(capture), None));
        }
        let mut output = match crate::io::Registration::try_new(target, prepared.output) {
            Ok(output) => output,
            Err(error) => return Err(failed_start(error.into(), Some(capture), None)),
        };
        if let Err(error) = output.disable_streams(crate::io::Direction::Input) {
            return Err(failed_start(error, Some(capture), Some(output)));
        }
        // Output starts silent; source first fills dry preview meters. No sending
        // is enabled until the caller explicitly changes the routing snapshot.
        if let Err(error) = output.start() {
            return Err(failed_start(error.into(), Some(capture), Some(output)));
        }
        if let Err(error) = capture.start() {
            return Err(failed_start(error.into(), Some(capture), Some(output)));
        }
        Ok(Self {
            virtual_streams,
            source_id: source,
            target_id: target,
            rate: prepared.input_rate,
            capture,
            output,
            routing: prepared.routing,
            meters: prepared.meters,
            source_streams: prepared.source_streams,
        })
    }
    pub fn stats(&self) -> (crate::capture::CaptureStats, crate::output::OutputStats) {
        (
            self.capture.handler().stats(),
            self.output.handler().stats(),
        )
    }
    pub fn close(self) -> Result<(), ShutdownError> {
        let route = self.routing.load();
        let route_stopped = self
            .routing
            .configure(route.selection, false, route.overview)
            .is_some();
        // Attempt both cleanups even if one reports an error. Each owner retains
        // its own callback storage whenever successful teardown is unconfirmed.
        let output = self.output.close();
        let capture = self.capture.close();
        shutdown_result(output, capture, route_stopped)
    }
}

/// Preserve both directions' teardown outcomes instead of returning the first.
#[derive(Debug)]
pub struct ShutdownError {
    pub output: Option<crate::io::IoError>,
    pub input: Option<crate::io::IoError>,
    pub route_stopped: bool,
}
impl ShutdownError {
    pub fn retained_io_contexts(&self) -> u64 {
        self.output
            .iter()
            .chain(self.input.iter())
            .filter(|error| error.context_retained)
            .count() as u64
    }
    pub fn requires_stop(&self) -> bool {
        !self.route_stopped
            || self
                .output
                .iter()
                .chain(self.input.iter())
                .any(|error| error.disposition == crate::io::IoDisposition::Fatal)
    }
    pub fn context_retained(&self) -> bool {
        self.output
            .iter()
            .chain(self.input.iter())
            .any(|error| error.context_retained)
    }
}
impl std::fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "session shutdown failed; route stopped: {}; output: {:?}; input: {:?}; context retained: {}",
            self.route_stopped,
            self.output,
            self.input,
            self.context_retained()
        )
    }
}
impl std::error::Error for ShutdownError {}
fn shutdown_result<O, I>(
    output: Result<O, crate::io::IoError>,
    input: Result<I, crate::io::IoError>,
    route_stopped: bool,
) -> Result<(), ShutdownError> {
    let output = output.err();
    let input = input.err();
    if output.is_none() && input.is_none() && route_stopped {
        Ok(())
    } else {
        Err(ShutdownError {
            output,
            input,
            route_stopped,
        })
    }
}

/// Startup cause and all cleanup errors, preserving whether callbacks may remain.
#[derive(Debug)]
pub struct StartupError {
    pub cause: String,
    pub cleanup_errors: Vec<String>,
    pub context_retained: bool,
    pub requires_stop: bool,
    pub retained_io_contexts: u64,
}
impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}; cleanup: {:?}; callback context retained: {}",
            self.cause, self.cleanup_errors, self.context_retained
        )
    }
}
impl std::error::Error for StartupError {}
fn failed_start(
    cause: Box<dyn std::error::Error>,
    capture: Option<crate::io::Registration<CaptureCallback>>,
    output: Option<crate::io::Registration<OutputCallback>>,
) -> Box<dyn std::error::Error> {
    let retained = cause
        .downcast_ref::<crate::io::IoError>()
        .is_some_and(|error| error.context_retained);
    let mut outcomes = Vec::new();
    if let Some(output) = output {
        if let Err(error) = output.close() {
            outcomes.push(error);
        }
    }
    if let Some(capture) = capture {
        if let Err(error) = capture.close() {
            outcomes.push(error);
        }
    }
    Box::new(startup_error(cause.to_string(), retained, outcomes))
}
fn startup_error(cause: String, retained: bool, outcomes: Vec<crate::io::IoError>) -> StartupError {
    StartupError {
        cause,
        context_retained: retained || outcomes.iter().any(|error| error.context_retained),
        requires_stop: retained
            || outcomes
                .iter()
                .any(|error| error.disposition == crate::io::IoDisposition::Fatal),
        retained_io_contexts: u64::from(retained)
            + outcomes
                .iter()
                .filter(|error| error.context_retained)
                .count() as u64,
        cleanup_errors: outcomes
            .into_iter()
            .map(|error| error.to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelbridge_audio::routing::Mode;
    #[test]
    fn removed_device_retention_permits_retry_and_is_counted_in_startup_and_shutdown() {
        let removed = || crate::io::IoError {
            operation: "destroy",
            status: channelbridge_hal_abi::fourcc(b"!dev") as i32,
            context_retained: true,
            disposition: crate::io::IoDisposition::RemovedDevice,
        };
        let shutdown = shutdown_result::<(), ()>(Err(removed()), Ok(()), true).unwrap_err();
        assert!(!shutdown.requires_stop());
        assert_eq!(shutdown.retained_io_contexts(), 1);
        let startup = startup_error("device disconnected".into(), false, vec![removed()]);
        assert!(!startup.requires_stop);
        assert!(startup.context_retained);
        assert_eq!(startup.retained_io_contexts, 1);
        let shutdown = shutdown_result::<(), ()>(Err(removed()), Ok(()), false).unwrap_err();
        assert!(shutdown.requires_stop());
    }
    #[test]
    fn shutdown_preserves_both_failures_and_retained_context_information() {
        let error = shutdown_result::<(), ()>(
            Err(crate::io::IoError {
                operation: "callback panic",
                status: 0,
                context_retained: false,
                disposition: crate::io::IoDisposition::Fatal,
            }),
            Err(crate::io::IoError {
                operation: "input stop",
                status: -1,
                context_retained: true,
                disposition: crate::io::IoDisposition::Fatal,
            }),
            true,
        )
        .unwrap_err();
        assert!(error.output.is_some() && error.input.is_some());
        assert!(error.context_retained());
        assert!(error.route_stopped);
        assert!(shutdown_result(Ok(()), Ok(()), true).is_ok());
        let error = shutdown_result(Ok(()), Ok(()), false).unwrap_err();
        assert!(!error.context_retained());
        assert!(!error.route_stopped);
    }
    #[test]
    fn startup_error_distinguishes_clean_failure_from_retained_callbacks() {
        let clean = startup_error("start failed".into(), false, Vec::new());
        assert!(!clean.context_retained);
        let retained = startup_error(
            "start failed".into(),
            false,
            vec![
                crate::io::IoError {
                    operation: "output stop",
                    status: -1,
                    context_retained: true,
                    disposition: crate::io::IoDisposition::Fatal,
                },
                crate::io::IoError {
                    operation: "input destroy",
                    status: -2,
                    context_retained: true,
                    disposition: crate::io::IoDisposition::Fatal,
                },
            ],
        );
        assert!(retained.context_retained);
        assert_eq!(retained.cleanup_errors.len(), 2);
        assert_eq!(retained.cause, "start failed");
        assert!(startup_error("registration failed".into(), true, Vec::new()).context_retained);
    }
    #[test]
    fn preparation_preserves_rates_and_rejects_missing_channels() {
        let format = StreamFormat {
            sample_rate: 48000.0,
            format_id: fourcc(b"lpcm"),
            format_flags: 9,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        };
        let selection = Selection::new(2, 1, Mode::Stereo, false).unwrap();
        let output = StreamFormat {
            sample_rate: 96000.0,
            ..format
        };
        let prepared =
            PreparedSession::from_formats(&[format], &[output], selection, 24000000).unwrap();
        assert_eq!((prepared.input_rate, prepared.output_rate), (48000, 96000));
        assert!(!prepared.routing.load().sending);
        assert_eq!(prepared.routing.load().selection, selection);
        assert_eq!(prepared.capture.stats().callbacks, 0);
        let generation = prepared
            .routing
            .configure(selection, true, false)
            .unwrap()
            .generation;
        let mut incoming = [[0.25_f32, -0.5]; 512];
        let input_buffer = channelbridge_hal_abi::AudioBufferList {
            count: 1,
            buffers: [channelbridge_hal_abi::AudioBuffer {
                channels: 2,
                byte_size: 4096,
                data: incoming.as_mut_ptr().cast(),
            }],
        };
        for block in 0..2 {
            let time = channelbridge_hal_abi::AudioTimestamp {
                sample_time: (block * 512) as f64,
                host_time: block * 512 * 500,
                flags: 3,
                ..Default::default()
            };
            // SAFETY: valid immutable input and timestamps matching prepared layout.
            unsafe {
                prepared.capture.process(&input_buffer, &time, &time);
            }
        }
        assert_eq!(prepared.capture.source_clock().speed(generation), Some(1.0));
        let mut outgoing = [[0_f32; 2]; 512];
        let mut output_buffer = channelbridge_hal_abi::AudioBufferList {
            count: 1,
            buffers: [channelbridge_hal_abi::AudioBuffer {
                channels: 2,
                byte_size: 4096,
                data: outgoing.as_mut_ptr().cast(),
            }],
        };
        // SAFETY: separate, uniquely writable native output with matching layout.
        unsafe {
            prepared.output.process(&mut output_buffer, None);
        }
        assert!((outgoing[511][0] + 0.5).abs() < 0.0001);
        assert!((outgoing[511][1] - 0.25).abs() < 0.0001);
        assert_eq!(prepared.output.stats().missing, 0);
        let missing = Selection::new(3, 1, Mode::Stereo, false).unwrap();
        assert!(PreparedSession::from_formats(&[format], &[output], missing, 24000000).is_err());
    }
}
