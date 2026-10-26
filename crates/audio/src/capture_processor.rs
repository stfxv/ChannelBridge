//! Capture-owned routing, dry input meters, clock observations and FIFO writes.
//! The HAL adapter supplies validated float samples, not OS pointers.
use crate::FramePosition;
use crate::StereoFrame;
use crate::drift::ClockEstimator;
use crate::drift::Observation;
use crate::fifo::Producer;
use crate::frame::finite;
use crate::meters::MeterWriter;
use crate::routing::AtomicRouting;
use crate::routing::Mode;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct CaptureTimestamp {
    pub frame: u64,
    pub host_tick: u64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureReport {
    pub written: usize,
    pub overflow: usize,
    pub invalid_clock: bool,
    pub format_changed: bool,
    pub unavailable_channel: bool,
    pub discontinuity: bool,
    pub generation: u64,
    pub speed: f64,
}
pub struct CaptureProcessor {
    routing: Arc<AtomicRouting>,
    meters: MeterWriter,
    producer: Producer,
    clock: ClockEstimator,
    scratch: Box<[StereoFrame]>,
    expected_frame: Option<u64>,
    had_clock: bool,
    pending_generation: bool,
}
impl CaptureProcessor {
    /// Takes ownership of the meter writer and queue producer for one callback.
    /// Returns `None` for a zero sample rate or tick frequency, or a failed
    /// reservation of the 4096-frame scratch buffer.
    pub fn new(
        routing: Arc<AtomicRouting>,
        meters: MeterWriter,
        producer: Producer,
        sample_rate: u32,
        ticks_per_second: u64,
    ) -> Option<Self> {
        let clock = ClockEstimator::new(sample_rate, ticks_per_second)?;
        let mut scratch = Vec::new();
        scratch.try_reserve_exact(4096).ok()?;
        scratch.resize(4096, StereoFrame::SILENCE);
        Some(Self {
            routing,
            meters,
            producer,
            clock,
            scratch: scratch.into_boxed_slice(),
            expected_frame: None,
            had_clock: false,
            pending_generation: false,
        })
    }

    /// now_tick is callback host time for meters; timestamp describes the first
    /// input sample for clock estimation. Preview and mute still update meters.
    pub fn process(
        &mut self,
        frames: usize,
        channels: usize,
        now_tick: u64,
        timestamp: Option<CaptureTimestamp>,
        mut sample: impl FnMut(usize, usize) -> f32,
    ) -> CaptureReport {
        let mut routing = self.routing.load();
        let mut report = CaptureReport {
            generation: routing.generation,
            speed: self.clock.speed(),
            ..Default::default()
        };
        if frames == 0 {
            return report;
        }
        if frames > self.scratch.len() || channels != self.meters.channels() {
            report.format_changed = true;
            return report;
        }
        let selection = routing.selection;
        for channel in 0..channels {
            if routing.overview || channel == selection.left() || channel == selection.right() {
                let mut peak = 0_f32;
                for frame in 0..frames {
                    peak = peak.max(finite(sample(frame, channel)).abs());
                }
                self.meters.record(channel, peak, now_tick);
            }
        }
        report.unavailable_channel = !selection.available(channels);
        let valid = timestamp.filter(|stamp| {
            FramePosition::from_integer(stamp.frame).is_some()
                && stamp
                    .frame
                    .checked_add(frames as u64)
                    .is_some_and(|end| end <= FramePosition::MAX_EXACT)
        });
        if let Some(stamp) = valid {
            if self
                .expected_frame
                .is_some_and(|expected| expected != stamp.frame)
            {
                self.pending_generation = true;
            }
            if self.clock.observe(stamp.frame, stamp.host_tick) == Observation::Discontinuity {
                self.pending_generation = true;
            }
            self.expected_frame = stamp.frame.checked_add(frames as u64);
            self.had_clock = true;
        } else {
            report.invalid_clock = true;
            if self.had_clock {
                self.pending_generation = true;
                self.expected_frame = None;
                self.had_clock = false;
                self.clock.reset();
            }
        }
        report.speed = self.clock.speed();
        report.discontinuity = self.pending_generation;
        if self.pending_generation {
            if let Some(next) = self.routing.advance_generation(routing) {
                routing = next;
                self.pending_generation = false;
            }
        }
        report.generation = routing.generation;
        if !routing.sending
            || report.unavailable_channel
            || report.invalid_clock
            || self.pending_generation
        {
            return report;
        }
        let output_frames = self.scratch.iter_mut().take(frames).enumerate();
        for (index, out) in output_frames {
            let left = finite(sample(index, selection.left()));
            let right = finite(sample(index, selection.right()));
            *out = match selection.mode() {
                Mode::Mono => StereoFrame::new(left, left),
                Mode::Stereo => StereoFrame::new(left, right),
                Mode::Mix => {
                    let mixed = left * 0.5 + right * 0.5;
                    StereoFrame::new(mixed, mixed)
                }
            };
        }
        report.written = self
            .producer
            .push_generation(&self.scratch[..frames], routing.generation);
        report.overflow = frames - report.written;
        if report.overflow != 0 {
            // A full queue contains late audio. Retire that generation rather
            // than playing its backlog after output recovers. The next callback
            // begins a fresh stream; all losses remain explicit in the report.
            report.discontinuity = true;
            self.pending_generation = true;
            if let Some(next) = self.routing.advance_generation(routing) {
                self.pending_generation = false;
                report.generation = next.generation;
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fifo::stereo_queue;
    use crate::meters::meter_bank;
    use crate::routing::Selection;
    #[test]
    fn preview_meters_remain_live_and_modes_keep_channels_coherent() {
        let selection = Selection::new(2, 3, Mode::Stereo, true)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        let routing = Arc::new(AtomicRouting::new(selection));
        let (meters, reader) = meter_bank(3, 48_000)
            .expect("fixture has 1..=512 channels and a nonzero host clock frequency");
        let (producer, mut consumer) =
            stereo_queue(1024).expect("fixture queue capacity is a nonzero power of two");
        let mut capture = CaptureProcessor::new(
            Arc::clone(&routing),
            meters,
            producer,
            48_000,
            48_000,
        )
        .expect(
            "fixture rate and host frequency are nonzero; fixed callback scratch allocation fits",
        );
        let empty = capture.process(0, 3, 0, None, |_, _| panic!("empty callback read"));
        assert!(!empty.format_changed && !empty.invalid_clock);
        assert_eq!(empty.generation, routing.load().generation);
        let sample = |_: usize, channel: usize| [0.1, 0.3, -0.6][channel];
        let report = capture.process(
            64,
            3,
            64,
            Some(CaptureTimestamp {
                frame: 0,
                host_tick: 0,
            }),
            sample,
        );
        assert_eq!(report.written, 0);
        assert_eq!(reader.level(1, 64).current, 0.3);
        assert_eq!(reader.level(2, 64).held_peak, 0.6);
        let started = routing
            .configure(selection, true, false)
            .expect("fixture selection is encodable and generation space is not exhausted");
        capture.process(
            64,
            3,
            128,
            Some(CaptureTimestamp {
                frame: 64,
                host_tick: 64,
            }),
            sample,
        );
        let mut audio = [StereoFrame::SILENCE; 64];
        assert_eq!(
            consumer
                .pop_generation(&mut audio, started.generation)
                .received,
            64
        );
        assert!(audio.iter().all(|f| *f == StereoFrame::new(0.3, -0.6)));
        let mix = Selection::new(2, 3, Mode::Mix, false)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        let changed = routing
            .configure(mix, true, true)
            .expect("fixture selection is encodable and generation space is not exhausted");
        assert_eq!(changed.generation, started.generation);
        capture.process(
            64,
            3,
            192,
            Some(CaptureTimestamp {
                frame: 128,
                host_tick: 128,
            }),
            sample,
        );
        consumer.pop_generation(&mut audio, changed.generation);
        assert!(audio.iter().all(|f| *f == StereoFrame::new(-0.15, -0.15)));
        assert_eq!(reader.level(0, 192).current, 0.1);
    }
    #[test]
    fn missing_channels_do_not_fall_back_and_clock_gaps_retire_audio() {
        let valid = Selection::new(1, 2, Mode::Mono, false)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        let routing = Arc::new(AtomicRouting::new(valid));
        routing
            .configure(valid, true, false)
            .expect("fixture selection is encodable and generation space is not exhausted");
        let (meters, _) = meter_bank(2, 48_000)
            .expect("fixture has 1..=512 channels and a nonzero host clock frequency");
        let (producer, mut consumer) =
            stereo_queue(1024).expect("fixture queue capacity is a nonzero power of two");
        let mut capture = CaptureProcessor::new(
            Arc::clone(&routing),
            meters,
            producer,
            48_000,
            48_000,
        )
        .expect(
            "fixture rate and host frequency are nonzero; fixed callback scratch allocation fits",
        );
        capture.process(
            64,
            2,
            64,
            Some(CaptureTimestamp {
                frame: 0,
                host_tick: 0,
            }),
            |_, _| 0.5,
        );
        let before = routing.load().generation;
        let gap = capture.process(
            64,
            2,
            256,
            Some(CaptureTimestamp {
                frame: 192,
                host_tick: 192,
            }),
            |_, _| -0.2,
        );
        assert!(gap.discontinuity && gap.generation > before);
        let mut audio = [StereoFrame::SILENCE; 64];
        let transferred = consumer.pop_generation(&mut audio, gap.generation);
        assert_eq!(transferred.discarded, 64);
        assert!(audio.iter().all(|f| *f == StereoFrame::new(-0.2, -0.2)));
        let missing = Selection::new(3, 4, Mode::Stereo, false)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        routing
            .configure(missing, true, false)
            .expect("fixture selection is encodable and generation space is not exhausted");
        let unavailable = capture.process(
            64,
            2,
            320,
            Some(CaptureTimestamp {
                frame: 256,
                host_tick: 256,
            }),
            |_, _| 0.9,
        );
        assert!(unavailable.unavailable_channel && unavailable.written == 0);
        assert_eq!(consumer.available(), 0);
    }
}
