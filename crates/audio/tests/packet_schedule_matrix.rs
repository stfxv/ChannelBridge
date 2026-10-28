use channelbridge_audio::StereoFrame;
use channelbridge_audio::fifo::{self, Producer};
use channelbridge_audio::output_processor::OutputProcessor;

#[derive(Clone, Copy, Debug)]
struct Schedule {
    input_rate: u32,
    output_rate: u32,
    capture_frames: usize,
    output_frames: usize,
    output_phase: usize,
}

impl Schedule {
    #[must_use]
    fn input_per_output(self) -> u64 {
        (self.output_frames as u64 * u64::from(self.input_rate))
            .div_ceil(u64::from(self.output_rate))
    }

    #[must_use]
    fn startup_allowance(self) -> u64 {
        // Allow several complete arrivals and output callbacks, including the initial fade.
        u64::from(self.input_rate) * 16 / 1000
            + 3 * self.capture_frames as u64
            + 2 * self.input_per_output()
            + 256
    }

    #[must_use]
    fn bounded_packets(self) -> u64 {
        let cycle = (self.capture_frames as u64).max(self.input_per_output());
        (self.startup_allowance() + 4 * cycle).div_ceil(self.capture_frames as u64)
    }
}

#[derive(Debug, Default)]
struct Observed {
    produced: usize,
    nonzero: usize,
    nonfinite: usize,
    steady_callbacks: usize,
    steady_produced: usize,
    steady_missing: usize,
    steady_dropped: usize,
    steady_priming: usize,
    steady_invalid_pcm: usize,
}

impl Observed {
    #[must_use]
    fn continuous(&self, output_frames: usize) -> bool {
        self.produced > 0
            && self.nonzero > 0
            && self.nonfinite == 0
            && self.steady_callbacks >= 4
            && self.steady_produced == self.steady_callbacks * output_frames
            && self.steady_missing == 0
            && self.steady_dropped == 0
            && self.steady_priming == 0
            && self.steady_invalid_pcm == 0
    }
}

struct Timeline {
    producer: Producer,
    processor: OutputProcessor,
    input_position: u64,
    next_output: u64,
}

impl Timeline {
    #[must_use]
    fn new(schedule: Schedule) -> Self {
        let (producer, consumer) =
            fifo::stereo_queue(16_384).expect("valid production queue capacity");
        let target = (schedule.input_rate as usize * 16) / 1000;
        let processor =
            OutputProcessor::new(consumer, schedule.input_rate, schedule.output_rate, target)
                .expect("supported rates and production buffering target");
        Self {
            producer,
            processor,
            input_position: 0,
            next_output: schedule.output_phase as u64,
        }
    }

    #[must_use]
    fn run(&mut self, schedule: Schedule, packets: u64) -> Observed {
        let start = self.input_position;
        let end = start + packets * schedule.capture_frames as u64;
        let steady_start = start + schedule.startup_allowance();
        let input_rate = u64::from(schedule.input_rate);
        let output_rate = u64::from(schedule.output_rate);
        let incoming = vec![StereoFrame::new(0.25, -0.25); schedule.capture_frames];
        let mut output = vec![StereoFrame::SILENCE; schedule.output_frames];
        let mut observed = Observed::default();
        while self.next_output * input_rate < end * output_rate {
            // A capture wins simultaneous events. Cross-products retain exact unequal-rate times.
            while self.input_position < end
                && self.input_position * output_rate <= self.next_output * input_rate
            {
                assert_eq!(
                    self.producer.push(&incoming),
                    incoming.len(),
                    "capture overflow at input frame {} for {schedule:?}",
                    self.input_position
                );
                self.input_position += schedule.capture_frames as u64;
            }
            let report = self.processor.render(&mut output, Some(1.0));
            let produced = &output[..report.produced];
            observed.produced += report.produced;
            observed.nonzero += produced
                .iter()
                .filter(|frame| frame.left.abs() > 0.01 && frame.right.abs() > 0.01)
                .count();
            observed.nonfinite += produced
                .iter()
                .filter(|frame| !frame.left.is_finite() || !frame.right.is_finite())
                .count();
            if self.next_output * input_rate >= steady_start * output_rate {
                observed.steady_callbacks += 1;
                observed.steady_produced += report.produced;
                observed.steady_missing += report.missing;
                observed.steady_dropped += report.dropped_lookahead;
                observed.steady_priming += usize::from(report.priming);
                observed.steady_invalid_pcm += produced
                    .iter()
                    .filter(|frame| {
                        !frame.left.is_finite()
                            || !frame.right.is_finite()
                            || (frame.left - 0.25).abs() > 0.005
                            || (frame.right + 0.25).abs() > 0.005
                    })
                    .count();
            }
            self.next_output += schedule.output_frames as u64;
        }
        while self.input_position < end {
            assert_eq!(
                self.producer.push(&incoming),
                incoming.len(),
                "capture overflow before stage boundary for {schedule:?}"
            );
            self.input_position += schedule.capture_frames as u64;
        }
        assert_eq!(self.input_position, end, "stage ends on a capture boundary");
        observed
    }
}

#[test]
fn equal_rate_capture_output_and_phase_matrix_remains_audible() {
    let mut failures = Vec::new();
    for capture_frames in [64, 512, 1024, 4096] {
        for output_frames in [64, 192, 4096] {
            for output_phase in [0, output_frames - 1] {
                let schedule = Schedule {
                    input_rate: 44_100,
                    output_rate: 44_100,
                    capture_frames,
                    output_frames,
                    output_phase,
                };
                let observed = Timeline::new(schedule).run(schedule, schedule.bounded_packets());
                eprintln!("{schedule:?} {observed:?}");
                if !observed.continuous(output_frames) {
                    failures.push(format!("{schedule:?} {observed:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn unequal_rate_packet_schedules_remain_audible() {
    let mut failures = Vec::new();
    for (input_rate, output_rate) in [
        (44_100, 48_000),
        (48_000, 44_100),
        (96_000, 48_000),
        (48_000, 96_000),
    ] {
        for (capture_frames, output_frames, output_phase) in [(4096, 192, 191), (512, 4096, 0)] {
            let schedule = Schedule {
                input_rate,
                output_rate,
                capture_frames,
                output_frames,
                output_phase,
            };
            let observed = Timeline::new(schedule).run(schedule, schedule.bounded_packets());
            eprintln!("{schedule:?} {observed:?}");
            if !observed.continuous(output_frames) {
                failures.push(format!("{schedule:?} {observed:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn packet_growth_keeps_its_reserve_for_thirty_seconds() {
    let initial = Schedule {
        input_rate: 44_100,
        output_rate: 44_100,
        capture_frames: 512,
        output_frames: 128,
        output_phase: 0,
    };
    let mut timeline = Timeline::new(initial);
    let before = timeline.run(initial, 32);
    assert!(before.continuous(initial.output_frames), "{before:?}");
    let grown = Schedule {
        capture_frames: 4096,
        ..initial
    };
    let packets = (30 * u64::from(grown.input_rate)).div_ceil(grown.capture_frames as u64);
    let after = timeline.run(grown, packets);
    eprintln!("512 to 4096 capture, 128 output, at least 30 seconds {after:?}");
    assert!(after.continuous(grown.output_frames), "{after:?}");
}
