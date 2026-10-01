//! Virtual-output callback's owned DSP state. No OS APIs or shared mutable state.
use crate::{StereoFrame, drift::DriftController, fifo::Consumer, resample::StreamingResampler};

const MAX_BLOCK: usize = 4096;
const FADE_FRAMES: usize = 64;

#[derive(Clone, Copy, Debug, Default)]
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
    target: usize,
    maximum_step: f64,
    active: bool,
    fade_position: usize,
    generation: u64,
    stale_discarded: u64,
}
impl OutputProcessor {
    pub fn new(input: Consumer, input_rate: u32, output_rate: u32, target: usize) -> Option<Self> {
        let controller = DriftController::new(input_rate, output_rate, target)?;
        let maximum_step = f64::from(input_rate) / f64::from(output_rate) * 1.002;
        let resampler = StreamingResampler::new(maximum_step, MAX_BLOCK)?;
        if target <= resampler.lookahead_frames() || target > input.capacity() {
            return None;
        }
        let mut scratch = Vec::new();
        scratch.try_reserve_exact(MAX_BLOCK).ok()?;
        scratch.resize(MAX_BLOCK, StereoFrame::SILENCE);
        Some(Self {
            input,
            resampler,
            controller,
            scratch: scratch.into_boxed_slice(),
            target,
            maximum_step,
            active: false,
            fade_position: 0,
            generation: 1,
            stale_discarded: 0,
        })
    }

    pub fn correction_ppm(&self) -> f64 {
        self.controller.correction_ppm()
    }
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
        self.controller.reset();
        self.active = false;
        self.fade_position = 0;
        let discarded = self.input.pop_generation(&mut [], generation).discarded;
        self.stale_discarded = self.stale_discarded.saturating_add(discarded as u64);
        discarded
    }

    fn queued(&self) -> usize {
        self.input
            .available()
            .saturating_add(self.resampler.queued_input_frames())
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

    /// Startup waits for a configured safety margin. A real shortage zeroes
    /// only missing output, fades the valid boundary and re-primes DSP history;
    /// it does not restart CoreAudio or mutate the producer's queue.
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
        if !self.active {
            if self.queued() < self.target {
                return RenderReport {
                    queued_input: self.queued(),
                    priming: true,
                    ..Default::default()
                };
            }
            self.active = true;
            self.fade_position = 0;
        }
        let step = self.controller.step();
        let mut budget = (output.len() as f64 * self.maximum_step).ceil() as usize
            + 2 * self.resampler.lookahead_frames()
            + MAX_BLOCK;
        let mut produced = 0;
        while produced < output.len() {
            self.fill_history(&mut budget);
            let count = self.resampler.render(&mut output[produced..], step);
            produced += count;
            if count == 0 || budget == 0 {
                break;
            }
        }
        for frame in output.iter_mut().take(produced) {
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
            for (index, frame) in output[produced - fade..produced].iter_mut().enumerate() {
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
    use crate::fifo::stereo_queue;
    #[test]
    fn restart_retires_interpolation_history_and_late_old_packets() {
        let (mut producer, consumer) = stereo_queue(8192).unwrap();
        let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 1024).unwrap();
        let old = [StereoFrame::new(0.8, 0.8); 2048];
        producer.push_generation(&old, 1);
        let mut output = [StereoFrame::SILENCE; 512];
        assert_eq!(processor.render(&mut output, None).missing, 0);
        assert!(output[511].left > 0.79);
        processor.set_generation(2);
        producer.push_generation(&old[..64], 1);
        let fresh = [StereoFrame::new(-0.25, -0.25); 2048];
        producer.push_generation(&fresh, 2);
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
            let (mut producer, consumer) = stereo_queue(16384).unwrap();
            let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 4608).unwrap();
            let mut estimator = ClockEstimator::new(48_000, 1_000_000_000).unwrap();
            let mut incoming = [StereoFrame::SILENCE; 8192];
            let mut output = [StereoFrame::SILENCE; 4096];
            let physical_rate = 48_000.0 * (1.0 + ppm / 1_000_000.0);
            let prefix = 4608;
            let mut supplied = 0;
            let mut elapsed = 0.0;
            let mut iteration = 0;
            let mut output_frames = 0;
            estimator.observe(prefix, 1_000_000_000);
            while elapsed < 4.0 {
                let frames = [64, 128, 256, 512, 1024, 4096][iteration % 6];
                elapsed += frames as f64 / 48_000.0;
                let jitter = [0.002, -0.002, 0.0][iteration % 3];
                let arrived = prefix + ((elapsed + jitter).max(0.0) * physical_rate).floor() as u64;
                let next = arrived.max(supplied);
                let count = (next - supplied) as usize;
                assert!(count <= incoming.len());
                for (index, frame) in incoming.iter_mut().take(count).enumerate() {
                    let phase = (supplied + index as u64) as f64 * 997.0 * std::f64::consts::TAU
                        / physical_rate;
                    *frame = StereoFrame::new(phase.sin() as f32, phase.cos() as f32);
                }
                assert_eq!(
                    producer.push(&incoming[..count]),
                    count,
                    "ppm={ppm} overflow"
                );
                supplied = next;
                let host_tick = 1_000_000_000
                    + ((supplied - prefix) as f64 / physical_rate * 1_000_000_000.0).round() as u64;
                estimator.observe(supplied, host_tick);
                let report = processor.render(&mut output[..frames], Some(estimator.speed()));
                assert_eq!(report.produced, frames, "ppm={ppm} report={report:?}");
                assert_eq!(report.missing, 0);
                assert_eq!(report.dropped_lookahead, 0);
                assert!((3000..6500).contains(&report.queued_input));
                for frame in output.iter().take(frames) {
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
        let (_, undersized) = stereo_queue(128).unwrap();
        assert!(OutputProcessor::new(undersized, 48_000, 48_000, 704).is_none());
        let (mut producer, consumer) = stereo_queue(4096).unwrap();
        let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 704).unwrap();
        let mut output = [StereoFrame::new(9.0, 9.0); 512];
        assert!(processor.render(&mut output, None).priming);
        assert!(output.iter().all(|f| *f == StereoFrame::SILENCE));
        let first = [StereoFrame::new(0.5, -0.5); 1024];
        assert_eq!(producer.push(&first), 1024);
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
        assert_eq!(producer.push(&second), 1024);
        assert_eq!(processor.render(&mut output, Some(1.0)).produced, 512);
        assert!(output[256].left < -0.24 && output[256].right > 0.24);
    }
}
