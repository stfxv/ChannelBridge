//! Release-mode output DSP wall-time probe. No devices or audio files are used.
//! Excludes capture, HAL scheduling, driver, UI and construction costs.
use channelbridge_audio::StereoFrame;
use channelbridge_audio::fifo;
use channelbridge_audio::output_processor::OutputProcessor;
use std::hint;
use std::time::Instant;

fn measure(rate: u32, capture: usize, block: usize) {
    let target = (rate as usize * 16 / 1000).max(256);
    let capacity = (target + 8192).next_power_of_two();
    let (mut producer, consumer) =
        fifo::stereo_queue(capacity).expect("fixture queue capacity is a nonzero power of two");
    let mut processor = OutputProcessor::new(consumer, rate, rate, target).expect(
        "fixture rates are supported and the target lies above lookahead within queue capacity",
    );
    let prefill = vec![StereoFrame::new(0.25, -0.125); target];
    for packet in prefill.chunks(capture) {
        assert_eq!(producer.push(packet), packet.len());
    }
    let incoming = vec![StereoFrame::new(0.25, -0.125); capture];
    let mut outgoing = vec![StereoFrame::SILENCE; block];
    let mut next_capture = 0;
    let mut timings = Vec::with_capacity(256);
    for iteration in 0..384 {
        while next_capture <= iteration * block {
            assert_eq!(producer.push(hint::black_box(&incoming)), capture);
            next_capture += capture;
        }
        let started = Instant::now();
        let report =
            hint::black_box(&mut processor).render(hint::black_box(&mut outgoing), Some(1.0));
        let elapsed = started.elapsed().as_secs_f64() * 1_000_000.0;
        assert_eq!(report.missing, 0);
        assert_eq!(report.dropped_lookahead, 0);
        hint::black_box(&outgoing);
        if iteration >= 128 {
            assert_eq!(report.produced, block);
            assert!(!report.priming);
            assert!(outgoing.iter().all(|frame| {
                (frame.left - 0.25).abs() < 0.001 && (frame.right + 0.125).abs() < 0.001
            }));
            timings.push(elapsed);
        }
    }
    timings.sort_by(f64::total_cmp);
    let budget = block as f64 / f64::from(rate) * 1_000_000.0;
    let p99 = timings[253];
    println!(
        "{rate}\t{capture}\t{block}\t{budget:.2}\t{:.2}\t{:.2}\t{p99:.2}\t{:.2}\t{:.2}",
        timings[127],
        timings[243],
        timings[255],
        p99 / budget * 100.0
    );
}

fn main() {
    println!(
        "Output DSP only; 128 warmup + 256 measured callbacks; wall time includes process preemption."
    );
    println!(
        "rate_hz\tcapture\toutput\tbudget_us\tp50_us\tp95_us\tp99_us\tmax_us\tp99_budget_percent"
    );
    for rate in measured_rates() {
        measure(rate, 512, 512);
    }
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for block in [64, 192, 4096] {
            measure(rate, block, block);
        }
    }
    for (rate, capture, block) in [
        (44_100, 1024, 128),
        (48_000, 4096, 64),
        (44_100, 4096, 192),
        (768_000, 4096, 64),
    ] {
        measure(rate, capture, block);
    }
}

// Keep this standalone audio example independent of the HAL contract crate.
fn measured_rates() -> [u32; 16] {
    [
        8_000, 16_000, 24_000, 32_000, 44_100, 48_000, 64_000, 88_200, 96_000, 128_000, 176_400,
        192_000, 352_800, 384_000, 705_600, 768_000,
    ]
}
