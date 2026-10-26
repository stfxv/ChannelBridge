use channelbridge_audio::FramePosition;
use channelbridge_audio::StereoFrame;
use channelbridge_audio::timeline_ring::TimelineRing;
use channelbridge_audio::timeline_ring::Transfer;
use channelbridge_contract::DriverCounters;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct DriverAudio {
    ring: TimelineRing,
    generation: AtomicU64,
    written: AtomicU64,
    missing: AtomicU64,
    rejected: AtomicU64,
    invalid: AtomicU64,
}

impl DriverAudio {
    #[must_use]
    pub fn new(capacity: usize) -> Option<Self> {
        Some(Self {
            ring: TimelineRing::new(capacity)?,
            generation: AtomicU64::new(1),
            written: AtomicU64::new(0),
            missing: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            invalid: AtomicU64::new(0),
        })
    }

    /// Called while HAL I/O is quiescent. A new generation invalidates old audio
    /// without clearing shared slots that might still be inspected by readers.
    pub fn next_generation(&self) -> Option<u64> {
        self.generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(1)
            })
            .ok()
            .and_then(|old| old.checked_add(1))
    }

    pub fn write(&self, time: f64, frames: &[StereoFrame]) -> Transfer {
        let Some(start) = FramePosition::from_sample_time(time) else {
            self.invalid.fetch_add(1, Ordering::Relaxed);
            self.rejected
                .fetch_add(frames.len() as u64, Ordering::Relaxed);
            return Transfer {
                valid: 0,
                missing: frames.len(),
            };
        };
        let result = self
            .ring
            .write(self.generation.load(Ordering::Acquire), start, frames);
        self.written
            .fetch_add(result.valid as u64, Ordering::Relaxed);
        self.rejected
            .fetch_add(result.missing as u64, Ordering::Relaxed);
        result
    }

    pub fn read(&self, time: f64, out: &mut [StereoFrame]) -> Transfer {
        let result = if let Some(start) = FramePosition::from_sample_time(time) {
            self.ring
                .read(self.generation.load(Ordering::Acquire), start, out)
        } else {
            self.invalid.fetch_add(1, Ordering::Relaxed);
            out.fill(StereoFrame::SILENCE);
            Transfer {
                valid: 0,
                missing: out.len(),
            }
        };
        self.missing
            .fetch_add(result.missing as u64, Ordering::Relaxed);
        result
    }

    #[must_use]
    pub fn counters(&self) -> DriverCounters {
        DriverCounters {
            generation: self.generation.load(Ordering::Acquire),
            written_frames: self.written.load(Ordering::Relaxed),
            missing_read_frames: self.missing.load(Ordering::Relaxed),
            rejected_write_frames: self.rejected.load(Ordering::Relaxed),
            invalid_timestamps: self.invalid.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortage_and_epoch_statistics_are_visible() {
        let audio =
            DriverAudio::new(512).expect("fixture timeline capacity is a nonzero power of two");
        let input = [StereoFrame::new(0.25, -0.25); 255];
        audio.write(128.25, &input);
        let mut out = [StereoFrame::SILENCE; 256];
        assert_eq!(audio.read(128.5, &mut out).valid, 255);
        assert_eq!(audio.counters().missing_read_frames, 1);
        audio.next_generation();
        assert_eq!(audio.read(128.0, &mut out).valid, 0);
        assert_eq!(audio.counters().generation, 2);
        audio.read(f64::NAN, &mut out);
        assert_eq!(audio.counters().invalid_timestamps, 1);
    }
}
