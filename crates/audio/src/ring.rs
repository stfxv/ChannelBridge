//! Timestamp-addressed virtual-driver storage, independent of any reader cursor.
//! Every slot field is atomic: a sequence check alone would not make plain
//! floating-point loads/stores safe under concurrent HAL callbacks.
use crate::{FramePosition, StereoFrame};
#[cfg(feature = "model-check")]
use loom::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(feature = "model-check"))]
use std::sync::atomic::{AtomicU64, Ordering};

struct Slot {
    sequence: AtomicU64,
    generation: AtomicU64,
    frame: AtomicU64,
    stereo: AtomicU64,
}

impl Slot {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            frame: AtomicU64::new(0),
            stereo: AtomicU64::new(0),
        }
    }

    fn write(&self, generation: u64, frame: u64, value: StereoFrame) -> bool {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return false;
        }
        let Some(published_sequence) = sequence.checked_add(2) else {
            return false;
        };
        if self
            .sequence
            .compare_exchange(
                sequence,
                sequence.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        // If a reader observes any field from this write, its acquire load also
        // observes the preceding odd sequence. The final sequence check cannot
        // then accept the previous even sequence with newly written data.
        self.generation.store(generation, Ordering::Release);
        self.frame.store(frame, Ordering::Release);
        self.stereo.store(
            StereoFrame::new(value.left, value.right).pack(),
            Ordering::Release,
        );
        self.sequence.store(published_sequence, Ordering::Release);
        true
    }

    fn read(&self, generation: u64, frame: u64) -> Option<StereoFrame> {
        let before = self.sequence.load(Ordering::Acquire);
        if before & 1 != 0 {
            return None;
        }
        let stored_generation = self.generation.load(Ordering::Acquire);
        let stored_frame = self.frame.load(Ordering::Acquire);
        let stereo = self.stereo.load(Ordering::Acquire);
        let after = self.sequence.load(Ordering::Acquire);
        if before != after || stored_generation != generation || stored_frame != frame {
            return None;
        }
        Some(StereoFrame::unpack(stereo))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Transfer {
    pub valid: usize,
    pub missing: usize,
}

pub struct TimelineRing {
    slots: Vec<Slot>,
}

impl TimelineRing {
    /// Allocation belongs to initialization, never a HAL I/O callback.
    pub fn new(capacity_frames: usize) -> Option<Self> {
        if capacity_frames == 0 || capacity_frames > 1_048_576 {
            return None;
        }
        let mut slots = Vec::new();
        slots.try_reserve_exact(capacity_frames).ok()?;
        slots.extend((0..capacity_frames).map(|_| Slot::new()));
        Some(Self { slots })
    }

    pub fn write(&self, generation: u64, start: FramePosition, audio: &[StereoFrame]) -> Transfer {
        let mut result = Transfer::default();
        if generation == 0 || audio.len() > self.slots.len() {
            result.missing = audio.len();
            return result;
        }
        for (offset, value) in audio.iter().enumerate() {
            let Some(frame) = start
                .get()
                .checked_add(offset as u64)
                .filter(|f| *f <= FramePosition::MAX_EXACT)
            else {
                result.missing += 1;
                continue;
            };
            let index = (frame % self.slots.len() as u64) as usize;
            if let Some(slot) = self.slots.get(index) {
                if slot.write(generation, frame, *value) {
                    result.valid += 1;
                } else {
                    result.missing += 1;
                }
            }
        }
        result
    }

    pub fn read(
        &self,
        generation: u64,
        start: FramePosition,
        audio: &mut [StereoFrame],
    ) -> Transfer {
        let mut result = Transfer::default();
        for (offset, out) in audio.iter_mut().enumerate() {
            *out = StereoFrame::SILENCE;
            let frame = start
                .get()
                .checked_add(offset as u64)
                .filter(|f| *f <= FramePosition::MAX_EXACT);
            let value = frame.and_then(|f| {
                let index = (f % self.slots.len() as u64) as usize;
                self.slots
                    .get(index)
                    .and_then(|slot| slot.read(generation, f))
            });
            if let Some(value) = value.filter(|_| generation != 0) {
                *out = value;
                result.valid += 1;
            } else {
                result.missing += 1;
            }
        }
        result
    }
}

#[cfg(all(test, not(feature = "model-check")))]
mod tests {
    use super::*;

    fn at(frame: u64) -> FramePosition {
        FramePosition::from_integer(frame).unwrap()
    }
    fn tone() -> StereoFrame {
        StereoFrame::new(0.25, -0.5)
    }

    #[test]
    fn shortage_preserves_available_frames_and_other_readers() {
        let ring = TimelineRing::new(512).unwrap();
        assert_eq!(ring.write(1, at(128), &[tone(); 255]).valid, 255);
        let mut out = [StereoFrame::SILENCE; 256];
        assert_eq!(
            ring.read(1, at(128), &mut out),
            Transfer {
                valid: 255,
                missing: 1
            }
        );
        assert_eq!(out[0], tone());
        assert_eq!(out[254], tone());
        assert_eq!(out[255], StereoFrame::SILENCE);
        ring.read(1, at(400), &mut out);
        assert_eq!(ring.read(1, at(128), &mut out).valid, 255);
    }

    #[test]
    fn fractional_timestamps_do_not_spuriously_mute() {
        let ring = TimelineRing::new(512).unwrap();
        ring.write(
            1,
            FramePosition::from_sample_time(128.25).unwrap(),
            &[tone(); 256],
        );
        let mut out = [StereoFrame::SILENCE; 256];
        assert_eq!(
            ring.read(1, FramePosition::from_sample_time(128.5).unwrap(), &mut out)
                .missing,
            0
        );
    }

    #[test]
    fn wrap_and_restart_do_not_read_stale_audio() {
        let ring = TimelineRing::new(4).unwrap();
        ring.write(1, at(3), &[tone(); 4]);
        let mut out = [StereoFrame::SILENCE; 4];
        assert_eq!(ring.read(1, at(3), &mut out).valid, 4);
        assert_eq!(ring.read(2, at(3), &mut out).valid, 0);
        ring.write(1, at(7), &[tone(); 4]);
        assert_eq!(ring.read(1, at(3), &mut out).valid, 0);
        assert_eq!(ring.read(1, at(7), &mut out).valid, 4);
    }

    #[test]
    fn readers_cannot_observe_torn_stereo_frames() {
        use std::sync::Arc;
        let ring = Arc::new(TimelineRing::new(64).unwrap());
        std::thread::scope(|scope| {
            for writer in 0..2 {
                let ring = ring.clone();
                scope.spawn(move || {
                    for n in 0..40_000 {
                        let v = (n + writer * 40_000 + 1) as f32;
                        ring.write(1, at(0), &[StereoFrame::new(v, -v)]);
                    }
                });
            }
            for _ in 0..2 {
                let ring = ring.clone();
                scope.spawn(move || {
                    for _ in 0..40_000 {
                        let mut out = [StereoFrame::SILENCE];
                        if ring.read(1, at(0), &mut out).valid == 1 {
                            assert_eq!(out[0].left, -out[0].right);
                        }
                    }
                });
            }
        });
    }
}

#[cfg(all(test, feature = "model-check"))]
mod model_tests {
    use super::*;
    use loom::{sync::Arc, thread};

    #[test]
    fn publication_never_mixes_old_tags_with_new_samples() {
        let mut model = loom::model::Builder::new();
        model.preemption_bound = Some(2);
        model.check(|| {
            let slot = Arc::new(Slot::new());
            assert!(slot.write(1, 3, StereoFrame::new(1.0, -1.0)));
            let writer_slot = slot.clone();
            let writer = thread::spawn(move || {
                writer_slot.write(2, 7, StereoFrame::new(2.0, -2.0));
            });
            let reader_slot = slot.clone();
            let reader = thread::spawn(move || {
                if let Some(frame) = reader_slot.read(1, 3) {
                    assert_eq!(frame, StereoFrame::new(1.0, -1.0));
                }
            });
            writer.join().unwrap();
            reader.join().unwrap();
        });
    }
}
