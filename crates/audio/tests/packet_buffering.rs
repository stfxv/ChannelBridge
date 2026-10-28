use channelbridge_audio::StereoFrame;
use channelbridge_audio::fifo::{self, Producer};
use channelbridge_audio::output_processor::OutputProcessor;

const RATE: u32 = 44_100;
const TARGET: usize = 705;
const STARTUP_FRAMES: usize = 8192;

#[derive(Clone, Copy)]
struct Stage {
    packet_frames: usize,
    packets: usize,
}

#[derive(Debug, Default)]
struct Observed {
    produced: usize,
    nonzero: usize,
    missing: usize,
    dropped_lookahead: usize,
    stable_callbacks: usize,
    stable_produced: usize,
    stable_nonzero: usize,
    stable_missing: usize,
    stable_dropped_lookahead: usize,
    stable_priming: usize,
}

fn pipeline() -> (Producer, OutputProcessor) {
    let (producer, consumer) = fifo::stereo_queue(16_384).expect("valid queue capacity");
    let processor = OutputProcessor::new(consumer, RATE, RATE, TARGET)
        .expect("valid equal rates and buffering target");
    (producer, processor)
}

fn run_packets(
    producer: &mut Producer,
    processor: &mut OutputProcessor,
    stages: &[Stage],
    output_frames: usize,
    output_phase: usize,
    generation: u64,
    sample: f32,
) -> Vec<Observed> {
    let incoming = vec![StereoFrame::new(sample, -sample); 4096];
    let mut output = vec![StereoFrame::SILENCE; output_frames];
    let mut observations = Vec::new();
    let mut stage_start = 0;
    let mut next_output = output_phase;

    for stage in stages {
        let stage_end = stage_start + stage.packet_frames * stage.packets;
        let mut next_capture = stage_start;
        let mut observed = Observed::default();
        while next_output < stage_end {
            while next_capture <= next_output && next_capture < stage_end {
                assert_eq!(
                    producer.push_generation(&incoming[..stage.packet_frames], generation),
                    stage.packet_frames,
                    "capture must fit at input frame {next_capture}"
                );
                next_capture += stage.packet_frames;
            }
            let report = processor.render(&mut output, Some(1.0));
            let nonzero = output[..report.produced]
                .iter()
                .filter(|frame| frame.left.abs() > 0.01 && frame.right.abs() > 0.01)
                .count();
            for frame in &output[..report.produced] {
                assert!(frame.left.is_finite() && frame.right.is_finite());
                assert!(
                    frame.left * sample >= 0.0 && frame.right * sample <= 0.0,
                    "rendered PCM must belong to generation {generation}"
                );
            }
            observed.produced += report.produced;
            observed.nonzero += nonzero;
            observed.missing += report.missing;
            observed.dropped_lookahead += report.dropped_lookahead;
            if next_output >= stage_start + STARTUP_FRAMES {
                observed.stable_callbacks += 1;
                observed.stable_produced += report.produced;
                observed.stable_nonzero += nonzero;
                observed.stable_missing += report.missing;
                observed.stable_dropped_lookahead += report.dropped_lookahead;
                observed.stable_priming += usize::from(report.priming);
            }
            next_output += output_frames;
        }
        assert_eq!(next_capture, stage_end, "stage ends on a capture boundary");
        observations.push(observed);
        stage_start = stage_end;
    }
    observations
}

fn assert_continuous(observed: &Observed, output_frames: usize) {
    assert!(
        observed.produced > 0 && observed.nonzero > 0,
        "{observed:?}"
    );
    assert!(observed.stable_callbacks > 0, "{observed:?}");
    assert!(
        observed.stable_produced > 0 && observed.stable_nonzero > 0,
        "audio must actually start and remain audible, {observed:?}"
    );
    assert_eq!(
        observed.stable_missing, 0,
        "capture packetization must not cause recurring shortages, {observed:?}"
    );
    assert_eq!(observed.stable_dropped_lookahead, 0, "{observed:?}");
    assert_eq!(observed.stable_priming, 0, "{observed:?}");
    assert_eq!(
        observed.stable_produced,
        observed.stable_callbacks * output_frames,
        "every callback after settling must contain complete PCM, {observed:?}"
    );
}

#[test]
fn capture_packets_larger_than_target_keep_small_output_callbacks_continuous() {
    let mut phases = Vec::new();
    for phase in [0, 127] {
        let (mut producer, mut processor) = pipeline();
        let observations = run_packets(
            &mut producer,
            &mut processor,
            &[Stage {
                packet_frames: 1024,
                packets: 128,
            }],
            128,
            phase,
            1,
            0.25,
        );
        eprintln!("1024/128 phase={phase} {:?}", observations[0]);
        phases.extend(observations);
    }
    for observed in &phases {
        assert_continuous(observed, 128);
    }
}

#[test]
fn packet_size_changes_settle_without_recurring_shortages() {
    let (mut producer, mut processor) = pipeline();
    let stages = [
        Stage {
            packet_frames: 512,
            packets: 24,
        },
        Stage {
            packet_frames: 1024,
            packets: 12,
        },
        Stage {
            packet_frames: 4096,
            packets: 3,
        },
        Stage {
            packet_frames: 256,
            packets: 48,
        },
    ];
    let observations = run_packets(&mut producer, &mut processor, &stages, 192, 0, 1, 0.25);
    eprintln!("512 -> 1024 -> 4096 -> 256 / 192 {observations:?}");
    for observed in &observations {
        assert_continuous(observed, 192);
    }
}

#[test]
fn nearby_small_capture_packets_remain_continuous() {
    let (mut producer, mut processor) = pipeline();
    let observations = run_packets(
        &mut producer,
        &mut processor,
        &[Stage {
            packet_frames: 512,
            packets: 48,
        }],
        192,
        73,
        1,
        0.25,
    );
    assert_continuous(&observations[0], 192);
}

#[test]
fn restarted_small_packets_do_not_replay_late_old_generation_pcm() {
    let (mut producer, mut processor) = pipeline();
    let old = [StereoFrame::new(0.75, -0.75); 4096];
    assert_eq!(producer.push_generation(&old, 1), old.len());
    let mut output = [StereoFrame::SILENCE; 192];
    let mut before = processor.render(&mut output, Some(1.0));
    for _ in 0..16 {
        if before.produced != 0 {
            break;
        }
        assert!(before.priming);
        assert_eq!(producer.push_generation(&old[..512], 1), 512);
        before = processor.render(&mut output, Some(1.0));
    }
    assert!(before.produced > 0 && output.iter().any(|frame| frame.left > 0.01));

    let generation = u64::MAX - 1;
    processor.set_generation(generation);
    assert_eq!(producer.push_generation(&old, 1), old.len());
    let observations = run_packets(
        &mut producer,
        &mut processor,
        &[Stage {
            packet_frames: 512,
            packets: 48,
        }],
        192,
        0,
        generation,
        -0.25,
    );
    assert_continuous(&observations[0], 192);
    assert!(processor.stale_discarded() >= old.len() as u64);
}
