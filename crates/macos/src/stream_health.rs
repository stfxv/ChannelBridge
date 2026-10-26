//! Control-thread callback progress watchdog, independent of device queries.
//! The caller supplies time so stalled callbacks can be tested without HAL I/O.
use crate::{capture::CaptureStats, output::OutputStats};
use std::time::{Duration, Instant};

const STALL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(crate) struct StreamHealth {
    input: Option<Progress>,
    output: Option<Progress>,
    rendering: Option<Progress>,
}

struct Progress {
    frames: u64,
    observed_at: Instant,
}

impl Progress {
    fn new(frames: u64, now: Instant) -> Self {
        Self {
            frames,
            observed_at: now,
        }
    }

    fn stalled(&mut self, frames: u64, now: Instant) -> bool {
        if frames != self.frames {
            self.frames = frames;
            self.observed_at = now;
        }
        now.saturating_duration_since(self.observed_at) >= STALL_TIMEOUT
    }
}

impl StreamHealth {
    pub(crate) fn started(&mut self, now: Instant) {
        self.input = Some(Progress::new(0, now));
        self.output = Some(Progress::new(0, now));
        self.rendering = None;
    }

    pub(crate) fn reset(&mut self) {
        self.input = None;
        self.output = None;
        self.rendering = None;
    }

    /// Start a fresh buffering grace period only when sending is enabled.
    /// Reapplying mode/mute settings must not hide a stalled output stream.
    pub(crate) fn routed(&mut self, sending: bool, rendered_frames: u64, now: Instant) {
        if sending {
            self.rendering
                .get_or_insert_with(|| Progress::new(rendered_frames, now));
        } else {
            self.rendering = None;
        }
    }

    pub(crate) fn check(
        &mut self,
        capture: CaptureStats,
        output: OutputStats,
        sending: bool,
        now: Instant,
    ) -> Result<bool, String> {
        if capture.failed {
            return Err("input audio callback failed".into());
        }
        if output.failed {
            return Err("virtual output audio callback failed".into());
        }
        self.routed(sending, output.rendered_frames, now);
        if self
            .input
            .get_or_insert_with(|| Progress::new(capture.accepted_frames, now))
            .stalled(capture.accepted_frames, now)
        {
            return Err("input audio frames stopped".into());
        }
        if self
            .output
            .get_or_insert_with(|| Progress::new(output.accepted_frames, now))
            .stalled(output.accepted_frames, now)
        {
            return Err("virtual output buffer processing stopped".into());
        }
        if self
            .rendering
            .as_mut()
            .is_some_and(|progress| progress.stalled(output.rendered_frames, now))
        {
            return Err("virtual output audio rendering stopped".into());
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buffers::CaptureLayout, capture::CaptureCallback, output::OutputCallback};
    use channelbridge_audio::{
        capture_processor::CaptureProcessor,
        fifo::stereo_queue,
        meters::meter_bank,
        output_processor::OutputProcessor,
        routing::{AtomicRouting, Mode, Selection},
    };
    use channelbridge_hal_abi::{
        AudioBuffer, AudioBufferList, AudioTimestamp, StreamFormat, fourcc,
    };
    use std::sync::Arc;

    struct SyntheticStream {
        capture: CaptureCallback,
        output: OutputCallback,
        routing: Arc<AtomicRouting>,
        frame: u64,
        sample: [f32; 2],
    }

    impl SyntheticStream {
        fn new(sending: bool, muted: bool) -> Self {
            let selection = Selection::new(1, 2, Mode::Stereo, muted)
                .expect("fixture channel numbers are nonzero and within the selection encoding");
            let routing = Arc::new(AtomicRouting::new(selection));
            routing
                .configure(selection, sending, false)
                .expect("fixture selection is encodable and generation space is not exhausted");
            let (meters, _) = meter_bank(2, 48_000)
                .expect("fixture has 1..=512 channels and a nonzero host clock frequency");
            let (producer, consumer) =
                stereo_queue(8192).expect("fixture queue capacity is a nonzero power of two");
            let capture = CaptureCallback::new(
                CaptureLayout::try_new(&[StreamFormat {
                    sample_rate: 48_000.0,
                    format_id: fourcc(b"lpcm"),
                    format_flags: 9,
                    bytes_per_packet: 8,
                    frames_per_packet: 1,
                    bytes_per_frame: 8,
                    channels_per_frame: 2,
                    bits_per_channel: 32,
                    reserved: 0,
                }])
                .expect("fixture is interleaved stereo float32 PCM at 48 kHz"),
                CaptureProcessor::new(Arc::clone(&routing), meters, producer, 48_000, 48_000)
                    .expect(
                        "fixture rate and host frequency are nonzero; fixed callback scratch allocation fits",
                    ),
            );
            let output = OutputCallback::new(
                OutputProcessor::new(consumer, 48_000, 48_000, 512).expect("fixture rates are supported and the target lies above lookahead within queue capacity"),
                Arc::clone(&routing),
            )
            .expect("fixture can allocate the fixed 4096-frame callback scratch buffer")
            .with_source_clock(capture.source_clock());
            Self {
                capture,
                output,
                routing,
                frame: 0,
                sample: [0.25, -0.5],
            }
        }

        fn capture(&mut self, frames: usize, valid_clock: bool, valid_buffer: bool) {
            let mut samples = [self.sample; 512];
            let input = AudioBufferList {
                count: 1,
                buffers: [AudioBuffer {
                    channels: if valid_buffer { 2 } else { 1 },
                    byte_size: (frames * 8) as u32,
                    data: samples.as_mut_ptr().cast(),
                }],
            };
            let now = AudioTimestamp {
                host_time: self.frame,
                flags: 2,
                ..Default::default()
            };
            let input_time = AudioTimestamp {
                host_time: self.frame,
                sample_time: self.frame as f64,
                flags: if valid_clock { 3 } else { 0 },
                ..Default::default()
            };
            // SAFETY: live float payload and complete descriptors/timestamps.
            unsafe { self.capture.process(&input, &now, &input_time) };
            self.frame += frames as u64;
        }

        fn output(&self, frames: usize, valid_buffer: bool) -> [[f32; 2]; 512] {
            let mut samples = [[9.0_f32; 2]; 512];
            let mut output = AudioBufferList {
                count: 1,
                buffers: [AudioBuffer {
                    channels: if valid_buffer { 2 } else { 1 },
                    byte_size: (frames * 8) as u32,
                    data: samples.as_mut_ptr().cast(),
                }],
            };
            // SAFETY: complete descriptor and uniquely writable float payload.
            unsafe { self.output.process(&mut output, None) };
            samples
        }

        fn check(&self, health: &mut StreamHealth, now: Instant) -> Result<bool, String> {
            health.check(
                self.capture.stats(),
                self.output.stats(),
                self.routing.load().sending,
                now,
            )
        }
    }

    #[test]
    fn callbacks_have_startup_grace_and_real_preview_progress_remains_healthy() {
        let start = Instant::now();
        let mut health = StreamHealth::default();
        health.started(start);
        let mut stream = SyntheticStream::new(false, false);
        assert!(
            stream
                .check(&mut health, start + STALL_TIMEOUT - Duration::from_nanos(1))
                .expect("startup grace has not yet reached its deadline")
        );
        assert_eq!(
            stream
                .check(&mut health, start + STALL_TIMEOUT)
                .expect_err("no input frames have arrived when startup grace expires"),
            "input audio frames stopped"
        );
        health.started(start);
        for millis in [500, 1000, 1500, 2000, 2500, 3000] {
            stream.capture(512, true, true);
            let samples = stream.output(512, true);
            assert!(samples.iter().all(|frame| *frame == [0.0; 2]));
            assert!(
                stream
                    .check(&mut health, start + Duration::from_millis(millis))
                    .expect("valid capture and output callbacks keep preview progress fresh")
            );
        }
        assert_eq!(stream.capture.stats().written, 0);
        assert_eq!(stream.output.stats().rendered_frames, 0);
    }

    #[test]
    fn invalid_input_clock_callbacks_trigger_recovery_after_two_seconds() {
        let start = Instant::now();
        let mut health = StreamHealth::default();
        health.started(start);
        let mut stream = SyntheticStream::new(false, false);
        stream.capture(512, true, true);
        stream.output(512, true);
        assert!(
            stream
                .check(&mut health, start)
                .expect("initial valid callbacks establish healthy input and output progress")
        );
        for millis in [500, 1000, 1500] {
            stream.capture(512, false, true);
            stream.output(512, true);
            assert!(
                stream
                    .check(&mut health, start + Duration::from_millis(millis))
                    .expect(
                        "invalid clock callbacks are still within the input stall grace period"
                    )
            );
        }
        stream.capture(512, false, true);
        stream.output(512, true);
        assert_eq!(stream.capture.stats().invalid_clock, 4);
        assert_eq!(stream.capture.stats().callbacks, 5);
        let error = stream
            .check(&mut health, start + STALL_TIMEOUT)
            .expect_err("invalid clocks prevent input progress until its stall deadline");
        assert!(error.contains("input"), "{error}");
    }

    #[test]
    fn rejected_and_empty_output_callbacks_trigger_recovery_in_preview() {
        for (frames, valid_buffer) in [(512, false), (0, true)] {
            let start = Instant::now();
            let mut health = StreamHealth::default();
            health.started(start);
            let mut stream = SyntheticStream::new(false, false);
            stream.capture(512, true, true);
            stream.output(512, true);
            assert!(
                stream
                    .check(&mut health, start)
                    .expect("initial valid callbacks establish healthy input and output progress")
            );
            for millis in [500, 1000, 1500] {
                stream.capture(512, true, true);
                stream.output(frames, valid_buffer);
                assert!(
                    stream
                        .check(&mut health, start + Duration::from_millis(millis))
                        .expect("rejected output callbacks are still within the output stall grace period")
                );
            }
            stream.capture(512, true, true);
            stream.output(frames, valid_buffer);
            assert_eq!(stream.output.stats().callbacks, 5);
            assert_eq!(stream.capture.stats().written, 0);
            let error = stream
                .check(&mut health, start + STALL_TIMEOUT)
                .expect_err("rejected or empty output prevents progress until its stall deadline");
            assert_eq!(error, "virtual output buffer processing stopped");
        }
    }

    #[test]
    fn perpetual_output_priming_triggers_recovery_while_callbacks_continue() {
        let start = Instant::now();
        let mut health = StreamHealth::default();
        health.started(start);
        let mut stream = SyntheticStream::new(true, false);
        for millis in [0, 500, 1000, 1500, 2000] {
            // Clock gaps retire each 256-frame generation before the 512-frame
            // startup margin can fill. Native input/output callbacks stay valid.
            stream.frame += 256;
            stream.capture(256, true, true);
            stream.output(512, true);
            let result = stream.check(&mut health, start + Duration::from_millis(millis));
            if millis < 2000 {
                assert!(result.expect("priming is permitted before the rendering stall deadline"));
            } else {
                assert_eq!(
                    result.expect_err("perpetual priming reaches the rendering stall deadline"),
                    "virtual output audio rendering stopped"
                );
            }
        }
        assert_eq!(stream.capture.stats().rejected, 0);
        assert_eq!(stream.output.stats().rejected, 0);
        assert_eq!(stream.output.stats().priming_callbacks, 5);
        assert_eq!(stream.output.stats().rendered_frames, 0);
    }

    #[test]
    fn malformed_and_empty_input_callbacks_do_not_count_as_capture_progress() {
        for (frames, valid_buffer) in [(512, false), (0, true)] {
            let start = Instant::now();
            let mut health = StreamHealth::default();
            health.started(start);
            let mut stream = SyntheticStream::new(false, false);
            stream.capture(512, true, true);
            stream.output(512, true);
            assert!(
                stream
                    .check(&mut health, start)
                    .expect("initial valid callbacks establish healthy input and output progress")
            );
            for millis in [500, 1000, 1500, 2000] {
                stream.capture(frames, true, valid_buffer);
                stream.output(512, true);
                let result = stream.check(&mut health, start + Duration::from_millis(millis));
                if millis < 2000 {
                    assert!(
                        result.expect(
                            "malformed input remains within its initial stall grace period"
                        )
                    );
                } else {
                    assert_eq!(
                        result.expect_err(
                            "malformed or empty input reaches the capture stall deadline"
                        ),
                        "input audio frames stopped"
                    );
                }
            }
            assert_eq!(stream.capture.stats().accepted_frames, 512);
            assert_eq!(stream.capture.stats().callbacks, 5);
        }
    }

    #[test]
    fn muted_and_silent_audio_keep_sending_healthy() {
        for muted in [false, true] {
            let start = Instant::now();
            let mut health = StreamHealth::default();
            health.started(start);
            let mut stream = SyntheticStream::new(true, muted);
            if !muted {
                stream.sample = [0.0; 2];
            }
            for millis in [0, 500, 1000, 1500, 2000, 2500, 3000] {
                stream.capture(512, true, true);
                let samples = stream.output(512, true);
                assert!(samples.iter().all(|frame| *frame == [0.0; 2]));
                assert!(
                    stream
                        .check(&mut health, start + Duration::from_millis(millis))
                        .expect("valid silent or muted frames still advance capture and rendering")
                );
            }
            assert!(stream.capture.stats().written > 0);
            assert!(stream.output.stats().rendered_frames > 0);
            assert_eq!(stream.output.stats().peak, Some([0.0; 2]));
        }
    }

    #[test]
    fn stop_and_restart_between_health_polls_grants_fresh_buffering_time() {
        let start = Instant::now();
        let mut health = StreamHealth::default();
        health.started(start);
        let mut stream = SyntheticStream::new(true, false);
        stream.capture(512, true, true);
        stream.output(512, true);
        assert!(
            stream
                .check(&mut health, start)
                .expect("initial valid callbacks establish healthy input and output progress")
        );
        let selection = stream.routing.load().selection;
        // Route updates can happen between health polls. Record both so the
        // restarted sender cannot inherit the previous generation's deadline.
        stream
            .routing
            .configure(selection, false, false)
            .expect("fixture selection is encodable and generation space is not exhausted");
        health.routed(
            false,
            stream.output.stats().rendered_frames,
            start + Duration::from_secs(1),
        );
        stream
            .routing
            .configure(selection, true, false)
            .expect("fixture selection is encodable and generation space is not exhausted");
        health.routed(
            true,
            stream.output.stats().rendered_frames,
            start + Duration::from_millis(1500),
        );
        for millis in [2000, 2500, 3000, 3499, 3500] {
            stream.frame += 256;
            stream.capture(256, true, true);
            stream.output(512, true);
            let result = stream.check(&mut health, start + Duration::from_millis(millis));
            if millis < 3500 {
                assert!(
                    result.expect("restarted sending has not reached its fresh rendering deadline")
                );
            } else {
                assert_eq!(
                    result.expect_err(
                        "restarted perpetual priming reaches its fresh rendering deadline"
                    ),
                    "virtual output audio rendering stopped"
                );
            }
        }
    }
}
