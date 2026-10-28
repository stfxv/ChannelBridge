//! Virtual-output callback's owned DSP state. No OS APIs or shared mutable state.
use crate::StereoFrame;
use crate::drift::DriftController;
use crate::fifo::Consumer;
use crate::resample::StreamingResampler;

const MAX_BLOCK: usize = 4096;
const FADE_FRAMES: usize = 64;

#[derive(Clone, Copy, Debug, Default)]
#[must_use]
pub struct RenderReport {
    pub produced: usize,
    pub missing: usize,
    pub dropped_lookahead: usize,
    pub queued_input: usize,
    pub priming: bool,
}

pub struct OutputProcessor {
    input: Consumer,
    resampler: StreamingResampler,
    controller: DriftController,
    scratch: Box<[StereoFrame]>,
    requested_target: usize,
    target: usize,
    publication_frames: usize,
    maximum_step: f64,
    active: bool,
    fade_position: usize,
    generation: u64,
    stale_discarded: u64,
}
impl OutputProcessor {
    /// Takes ownership of the queue consumer and preallocates callback storage.
    /// Returns `None` for zero rates, an invalid resampler step, a target
    /// no larger than filter lookahead or above queue capacity, or a failed
    /// storage reservation. The target counts queue and filter history together.
    /// Each push is treated as one capture packet. Observed packet lengths raise
    /// the steady target for this generation. Startup additionally reserves one
    /// output block. History is allocated for that reserve and a whole packet
    /// of threshold overshoot before either callback starts.
    #[must_use]
    pub fn new(input: Consumer, input_rate: u32, output_rate: u32, target: usize) -> Option<Self> {
        let maximum_step = f64::from(input_rate) / f64::from(output_rate) * 1.002;
        let maximum_consumption = (MAX_BLOCK as f64 * maximum_step).ceil() as usize;
        let future_capacity = input
            .capacity()
            .checked_mul(2)?
            .checked_add(maximum_consumption)?;
        let resampler = StreamingResampler::with_future_capacity(maximum_step, future_capacity)?;
        if target <= resampler.lookahead_frames() || target > input.capacity() {
            return None;
        }
        let pending_limit = input.capacity().checked_add(resampler.pending_capacity())?;
        let controller = DriftController::new(input_rate, output_rate, target, pending_limit)?;
        let mut scratch = Vec::new();
        scratch.try_reserve_exact(MAX_BLOCK).ok()?;
        scratch.resize(MAX_BLOCK, StereoFrame::SILENCE);
        Some(Self {
            input,
            resampler,
            controller,
            scratch: scratch.into_boxed_slice(),
            requested_target: target,
            target,
            publication_frames: 0,
            maximum_step,
            active: false,
            fade_position: 0,
            generation: 1,
            stale_discarded: 0,
        })
    }

    #[must_use]
    pub fn correction_ppm(&self) -> f64 {
        self.controller.correction_ppm()
    }
    #[must_use]
    pub fn fifo_frames(&self) -> usize {
        self.input.available()
    }
    /// Effective steady target includes FIFO and interpolation history together.
    #[must_use]
    pub fn target_frames(&self) -> usize {
        self.target
    }
    #[must_use]
    pub fn stale_discarded(&self) -> u64 {
        self.stale_discarded
    }
    /// Called by the output callback after reading the atomic route generation.
    /// Old FIFO samples and interpolation history cannot cross Stop/Start or a
    /// source-clock discontinuity. Mode/mute edits do not change this generation.
    pub fn set_generation(&mut self, generation: u64) -> usize {
        if generation <= self.generation {
            return 0;
        }
        self.generation = generation;
        self.resampler.reset();
        self.publication_frames = 0;
        self.target = self.requested_target;
        self.controller.retarget(self.target);
        self.controller.reset();
        self.active = false;
        self.fade_position = 0;
        let discarded = self.input.pop_generation(&mut [], generation).discarded;
        self.stale_discarded = self.stale_discarded.saturating_add(discarded as u64);
        discarded
    }

    fn observe_publication(&mut self, publication_frames: usize) {
        self.publication_frames = self.publication_frames.max(publication_frames);
        let target = self.requested_target.max(
            self.publication_frames.min(self.input.capacity())
                + self.resampler.lookahead_frames()
                + 1,
        );
        if target != self.target {
            self.target = target;
            self.controller.retarget(target);
        }
    }

    fn queued(&mut self) -> usize {
        let view = self.input.generation_view(self.generation);
        self.observe_publication(view.publication_frames);
        view.pending + self.resampler.queued_input_frames()
    }

    fn fill_history(&mut self, budget: &mut usize) -> usize {
        let mut total = 0;
        // Each successful pass consumes at least one budgeted sample. An empty
        // FIFO, full working history or exhausted budget ends the loop.
        while *budget != 0 {
            let count = self
                .resampler
                .writable_frames()
                .min(self.scratch.len())
                .min(*budget);
            if count == 0 {
                break;
            }
            let transferred = self
                .input
                .pop_generation(&mut self.scratch[..count], self.generation);
            self.stale_discarded = self
                .stale_discarded
                .saturating_add(transferred.discarded as u64);
            self.observe_publication(transferred.publication_frames);
            let received = transferred.received;
            if received == 0 {
                break;
            }
            let accepted = self.resampler.push(&self.scratch[..received]);
            total += accepted;
            *budget -= received;
            if accepted != received {
                break;
            }
        }
        total
    }

    /// Startup waits for the safety margin and enough input for a complete block.
    /// A real shortage zeroes only missing output, fades the valid boundary and
    /// re-primes DSP history; it does not restart CoreAudio or mutate the producer's queue.
    pub fn render(
        &mut self,
        output: &mut [StereoFrame],
        source_speed: Option<f64>,
    ) -> RenderReport {
        output.fill(StereoFrame::SILENCE);
        if output.is_empty() || output.len() > MAX_BLOCK {
            return RenderReport {
                missing: output.len(),
                ..Default::default()
            };
        }
        let consumption = (output.len() as f64 * self.maximum_step).ceil() as usize;
        let required = consumption + self.resampler.lookahead_frames() + 1;
        if required > self.input.capacity() {
            return RenderReport {
                missing: output.len(),
                queued_input: self.queued(),
                ..Default::default()
            };
        }
        let mut budget =
            self.input.capacity() + consumption + 2 * self.resampler.lookahead_frames();
        if !self.active {
            self.fill_history(&mut budget);
            let queued = self.queued();
            let startup = self
                .requested_target
                .max(self.publication_frames.min(self.input.capacity()) + required);
            if queued < startup {
                return RenderReport {
                    queued_input: queued,
                    priming: true,
                    ..Default::default()
                };
            }
            self.active = true;
            self.fade_position = 0;
        } else {
            let _ = self.queued();
        }
        let step = self.controller.step();
        let mut produced = 0;
        while produced < output.len() {
            self.fill_history(&mut budget);
            let count = self.resampler.render(&mut output[produced..], step);
            produced += count;
            if count == 0 || budget == 0 {
                break;
            }
        }
        let produced_frames = output.iter_mut().take(produced);
        for frame in produced_frames {
            if self.fade_position < FADE_FRAMES {
                self.fade_position += 1;
                let gain = self.fade_position as f32 / FADE_FRAMES as f32;
                frame.left *= gain;
                frame.right *= gain;
            }
        }
        let missing = output.len() - produced;
        let queued = self.queued();
        let mut dropped = 0;
        if missing != 0 {
            let fade = produced.min(FADE_FRAMES);
            let fade_frames = output[produced - fade..produced].iter_mut().enumerate();
            for (index, frame) in fade_frames {
                let gain = (fade - index - 1) as f32 / FADE_FRAMES as f32;
                frame.left *= gain;
                frame.right *= gain;
            }
            dropped = self.resampler.queued_input_frames();
            self.resampler.reset();
            self.active = false;
        } else {
            self.controller.update(queued, output.len(), source_speed);
        }
        RenderReport {
            produced,
            missing,
            dropped_lookahead: dropped,
            queued_input: queued,
            priming: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::ClockEstimator;
    use crate::fifo;
    #[test]
    fn capacity_sized_packets_prime_small_fifo_and_survive_threshold_overshoot() {
        let (mut producer, consumer) =
            fifo::stereo_queue(1024).expect("fixture capacity is supported");
        let mut processor = OutputProcessor::new(consumer, 44_100, 44_100, 768)
            .expect("fixture rates and target are supported");
        let pcm = [StereoFrame::new(0.25, -0.25); 1024];
        let mut output = [StereoFrame::SILENCE; 128];
        let mut audible = 0;
        for callback in 0..256 {
            if callback % 8 == 0 {
                assert_eq!(producer.push(&pcm), pcm.len());
            }
            let report = processor.render(&mut output, Some(1.0));
            assert_eq!(report.missing, 0);
            assert_eq!(report.dropped_lookahead, 0);
            assert!(
                report.queued_input
                    <= processor.input.capacity() + processor.resampler.pending_capacity()
            );
            if callback >= 8 {
                assert!(!report.priming);
                assert_eq!(report.produced, output.len());
                assert!(output[127].left > 0.24 && output[127].right < -0.24);
                audible += report.produced;
            }
        }
        assert_eq!(audible, 248 * output.len());
        assert_eq!(processor.target_frames(), 1122);
    }

    #[test]
    fn packet_growth_preserves_running_pcm_and_generation_reset_lowers_target() {
        let (mut producer, consumer) =
            fifo::stereo_queue(8192).expect("fixture capacity is supported");
        let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 704)
            .expect("fixture rates and target are supported");
        let pcm = [StereoFrame::new(0.25, -0.25); 4096];
        for packet in pcm[..2048].chunks(256) {
            assert_eq!(producer.push(packet), packet.len());
        }
        let mut output = [StereoFrame::SILENCE; 128];
        assert_eq!(processor.render(&mut output, Some(1.001)).produced, 128);
        let correction = processor.correction_ppm();
        let cursor = processor.resampler.queued_input_frames();
        assert_eq!(producer.push(&pcm), pcm.len());
        let report = processor.render(&mut output, None);
        assert_eq!(report.produced, output.len());
        assert!(!report.priming);
        assert_eq!(report.missing, 0);
        assert!(
            output
                .iter()
                .all(|frame| frame.left > 0.24 && frame.right < -0.24)
        );
        assert!(report.queued_input >= cursor + pcm.len() - output.len() - 1);
        assert!((processor.correction_ppm() - correction).abs() < 1.0);
        assert_eq!(processor.target_frames(), 4194);
        processor.set_generation(u64::MAX - 1);
        assert_eq!(processor.target_frames(), 704);
        assert_eq!(producer.push_generation(&pcm, 1), pcm.len());
        assert_eq!(producer.push_generation(&pcm[..256], u64::MAX), 256);
        let report = processor.render(&mut output, None);
        assert!(report.priming);
        assert_eq!(report.queued_input, 0);
        assert_eq!(processor.target_frames(), 704);
        assert_eq!(processor.fifo_frames(), 256);
        processor.set_generation(u64::MAX);
        assert_eq!(producer.push_generation(&pcm[..512], u64::MAX), 512);
        assert_eq!(processor.render(&mut output, None).produced, output.len());
        assert_eq!(processor.target_frames(), 704);
    }

    #[test]
    fn partial_packet_reserve_is_bounded_and_source_stop_reports_real_shortage() {
        let (mut producer, consumer) =
            fifo::stereo_queue(1024).expect("fixture capacity is supported");
        let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 768)
            .expect("fixture rates and target are supported");
        let pcm = [StereoFrame::new(0.25, -0.25); 4096];
        let mut output = [StereoFrame::SILENCE; 128];
        assert_eq!(producer.push(&pcm), 1024);
        assert_eq!(producer.push(&pcm), 0);
        assert!(processor.render(&mut output, None).priming);
        assert_eq!(processor.publication_frames, pcm.len());
        assert_eq!(processor.target_frames(), 1122);
        assert_eq!(producer.push(&pcm[..1024]), 1024);
        assert_eq!(processor.render(&mut output, None).produced, output.len());
        let mut shortage = false;
        for _ in 0..20 {
            let report = processor.render(&mut output, None);
            if report.missing != 0 {
                assert!(!report.priming);
                assert!(report.dropped_lookahead > 0);
                assert!(
                    output[report.produced..]
                        .iter()
                        .all(|frame| *frame == StereoFrame::SILENCE)
                );
                shortage = true;
                break;
            }
        }
        assert!(shortage);
        assert!(processor.render(&mut output, None).priming);
        assert_eq!(processor.target_frames(), 1122);
        assert_eq!(producer.push(&pcm[..1024]), 1024);
        assert!(processor.render(&mut output, None).priming);
        assert_eq!(producer.push(&pcm[..1024]), 1024);
        assert_eq!(processor.render(&mut output, None).produced, output.len());
    }

    #[test]
    fn session_target_primes_complete_large_callbacks() {
        for frames in [1024, 4096] {
            let target = 48_000 * 16 / 1000;
            let (mut producer, consumer) = fifo::stereo_queue(16384)
                .expect("fixture queue capacity is a nonzero power of two");
            let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, target).expect(
                "fixture rates are supported and the target lies above lookahead within queue capacity",
            );
            let incoming = vec![StereoFrame::new(0.25, -0.25); frames];
            let mut output = vec![StereoFrame::SILENCE; frames];
            let mut audible_blocks = 0;
            for cycle in 0..16 {
                assert_eq!(producer.push(&incoming), frames);
                let report = processor.render(&mut output, Some(1.0));
                assert_eq!(
                    report.missing, 0,
                    "frames={frames} cycle={cycle} {report:?}"
                );
                assert_eq!(report.dropped_lookahead, 0);
                if report.priming {
                    assert!(cycle < 2, "startup must finish by the third callback");
                    assert_eq!(report.queued_input, (cycle + 1) * frames);
                    assert!(output.iter().all(|frame| *frame == StereoFrame::SILENCE));
                } else {
                    assert_eq!(report.produced, frames);
                    assert!((output[frames - 1].left - 0.25).abs() < 0.0001);
                    assert!((output[frames - 1].right + 0.25).abs() < 0.0001);
                    audible_blocks += 1;
                }
            }
            assert!(audible_blocks >= 14);
            assert_eq!(
                processor.target_frames(),
                target.max(frames + processor.resampler.lookahead_frames() + 1)
            );
        }
    }

    #[test]
    fn priming_tracks_callback_size_after_restart() {
        let (mut producer, consumer) =
            fifo::stereo_queue(16384).expect("fixture queue capacity is a nonzero power of two");
        let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 768).expect(
            "fixture rates are supported and the target lies above lookahead within queue capacity",
        );
        let pcm = [StereoFrame::new(0.25, -0.25); 4096];
        let mut output = [StereoFrame::SILENCE; 4096];
        for packet in pcm[..768].chunks(128) {
            assert_eq!(producer.push(packet), packet.len());
        }
        assert_eq!(
            processor.render(&mut output[..128], Some(1.0)).produced,
            128
        );
        processor.set_generation(2);
        assert_eq!(producer.push_generation(&pcm, 2), 4096);
        let report = processor.render(&mut output, Some(1.0));
        assert!(report.priming);
        assert_eq!(report.queued_input, 4096);
        assert_eq!(producer.push_generation(&pcm, 2), 4096);
        assert!(processor.render(&mut output, Some(1.0)).priming);
        assert_eq!(producer.push_generation(&pcm, 2), 4096);
        let report = processor.render(&mut output, Some(1.0));
        assert_eq!(report.produced, 4096);
        assert_eq!(report.missing, 0);
        assert!((output[4095].left - 0.25).abs() < 0.0001);
    }

    #[test]
    fn callback_larger_than_queue_capacity_reports_shortage_without_consuming() {
        let (mut producer, consumer) =
            fifo::stereo_queue(1024).expect("fixture queue capacity is a nonzero power of two");
        let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 768).expect(
            "fixture rates are supported and the target lies above lookahead within queue capacity",
        );
        let pcm = [StereoFrame::new(0.25, -0.25); 1024];
        assert_eq!(producer.push(&pcm), pcm.len());
        let mut output = [StereoFrame::SILENCE; 4096];
        for _ in 0..2 {
            let report = processor.render(&mut output, Some(1.0));
            assert!(!report.priming);
            assert_eq!(report.produced, 0);
            assert_eq!(report.missing, output.len());
            assert_eq!(report.queued_input, pcm.len());
            assert_eq!(report.dropped_lookahead, 0);
        }
        assert!(processor.render(&mut output[..128], Some(1.0)).priming);
        assert_eq!(producer.push(&pcm), pcm.len());
        assert_eq!(
            processor.render(&mut output[..128], Some(1.0)).produced,
            128
        );
    }

    #[test]
    fn restart_retires_interpolation_history_and_late_old_packets() {
        let (mut producer, consumer) =
            fifo::stereo_queue(8192).expect("fixture queue capacity is a nonzero power of two");
        let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 1024).expect(
            "fixture rates are supported and the target lies above lookahead within queue capacity",
        );
        let old = [StereoFrame::new(0.8, 0.8); 2048];
        for packet in old.chunks(256) {
            assert_eq!(producer.push_generation(packet, 1), packet.len());
        }
        let mut output = [StereoFrame::SILENCE; 512];
        assert_eq!(processor.render(&mut output, None).missing, 0);
        assert!(output[511].left > 0.79);
        processor.set_generation(2);
        let _ = producer.push_generation(&old[..64], 1);
        let fresh = [StereoFrame::new(-0.25, -0.25); 2048];
        for packet in fresh.chunks(256) {
            assert_eq!(producer.push_generation(packet, 2), packet.len());
        }
        assert_eq!(processor.render(&mut output, None).missing, 0);
        assert!(
            output
                .iter()
                .all(|frame| frame.left <= 0.0 && frame.right <= 0.0)
        );
        assert!((output[511].left + 0.25).abs() < 0.0001);
        assert!(processor.stale_discarded() >= 64);
        assert_eq!(processor.set_generation(1), 0);
        assert_eq!(processor.render(&mut output, None).missing, 0);
        assert!((output[511].left + 0.25).abs() < 0.0001);
    }
    #[test]
    fn pcm_pipeline_survives_clock_drift_variable_blocks_and_arrival_jitter() {
        // Four seconds of actual PCM per case, advanced without real-time waits.
        // The separate eight-hour test covers only the control/queue model.
        for ppm in [-1000.0, -500.0, -100.0, 100.0, 500.0, 1000.0] {
            let (mut producer, consumer) = fifo::stereo_queue(16384)
                .expect("fixture queue capacity is a nonzero power of two");
            let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 4608).expect(
                "fixture rates are supported and the target lies above lookahead within queue capacity",
            );
            let mut estimator = ClockEstimator::new(48_000, 1_000_000_000)
                .expect("fixture sample rate and host clock frequency are positive");
            let mut incoming = [StereoFrame::SILENCE; 8192];
            let mut output = [StereoFrame::SILENCE; 4096];
            let physical_rate = 48_000.0 * (1.0 + ppm / 1_000_000.0);
            let prefix = 4608;
            let mut supplied = 0;
            let mut elapsed = 0.0;
            let mut iteration = 0;
            let mut output_frames = 0;
            let _ = estimator.observe(prefix, 1_000_000_000);
            while elapsed < 4.0 {
                let frames = [64, 128, 256, 512, 1024, 4096][iteration % 6];
                elapsed += frames as f64 / 48_000.0;
                let jitter = [0.002, -0.002, 0.0][iteration % 3];
                let arrived = prefix + ((elapsed + jitter).max(0.0) * physical_rate).floor() as u64;
                let next = arrived.max(supplied);
                let count = (next - supplied) as usize;
                assert!(count <= incoming.len());
                let indexed_input_frames = incoming.iter_mut().take(count).enumerate();
                for (index, frame) in indexed_input_frames {
                    let phase = (supplied + index as u64) as f64 * 997.0 * std::f64::consts::TAU
                        / physical_rate;
                    *frame = StereoFrame::new(phase.sin() as f32, phase.cos() as f32);
                }
                for packet in incoming[..count].chunks(512) {
                    assert_eq!(producer.push(packet), packet.len(), "ppm={ppm} overflow");
                }
                supplied = next;
                let host_tick = 1_000_000_000
                    + ((supplied - prefix) as f64 / physical_rate * 1_000_000_000.0).round() as u64;
                let _ = estimator.observe(supplied, host_tick);
                let report = processor.render(&mut output[..frames], Some(estimator.speed()));
                assert_eq!(report.produced, frames, "ppm={ppm} report={report:?}");
                assert_eq!(report.missing, 0);
                assert_eq!(report.dropped_lookahead, 0);
                assert!((3000..6500).contains(&report.queued_input));
                let rendered_frames = output.iter().take(frames);
                for frame in rendered_frames {
                    assert!(frame.left.is_finite() && frame.right.is_finite());
                    if output_frames > 256 {
                        let amplitude = frame.left.hypot(frame.right);
                        assert!(
                            (amplitude - 1.0).abs() < 0.0001,
                            "ppm={ppm} amplitude={amplitude}"
                        );
                    }
                    output_frames += 1;
                }
                iteration += 1;
            }
            assert!(output_frames >= 192_000);
        }
    }
    #[test]
    fn startup_shortage_and_recovery_never_replay_old_samples() {
        let (_, undersized) =
            fifo::stereo_queue(128).expect("fixture queue capacity is a nonzero power of two");
        assert!(OutputProcessor::new(undersized, 48_000, 48_000, 704).is_none());
        let (mut producer, consumer) =
            fifo::stereo_queue(4096).expect("fixture queue capacity is a nonzero power of two");
        let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 704).expect(
            "fixture rates are supported and the target lies above lookahead within queue capacity",
        );
        let mut output = [StereoFrame::new(9.0, 9.0); 512];
        assert!(processor.render(&mut output, None).priming);
        assert!(output.iter().all(|f| *f == StereoFrame::SILENCE));
        let first = [StereoFrame::new(0.5, -0.5); 1024];
        for packet in first.chunks(256) {
            assert_eq!(producer.push(packet), packet.len());
        }
        assert_eq!(processor.render(&mut output, Some(1.0)).produced, 512);
        assert!(output[256].left > 0.49 && output[256].right < -0.49);
        let shortage = processor.render(&mut output, None);
        assert!(shortage.produced > 0 && shortage.missing > 0 && shortage.dropped_lookahead > 0);
        assert!(
            output
                .iter()
                .skip(shortage.produced)
                .all(|f| *f == StereoFrame::SILENCE)
        );
        assert!(processor.render(&mut output, None).priming);
        assert!(output.iter().all(|f| *f == StereoFrame::SILENCE));
        let second = [StereoFrame::new(-0.25, 0.25); 1024];
        for packet in second.chunks(256) {
            assert_eq!(producer.push(packet), packet.len());
        }
        assert_eq!(processor.render(&mut output, Some(1.0)).produced, 512);
        assert!(output[256].left < -0.24 && output[256].right > 0.24);
    }
}
