//! App-side single-producer/single-consumer queue, distinct from the HAL ring.
//! Endpoints are not Clone, and operations require exclusive access to their
//! endpoint. No allocation, waiting, locks or retry loop occurs during transfer.
use crate::StereoFrame;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

struct Slot {
    stereo: AtomicU64,
    generation: AtomicU64,
    publication_frames: AtomicUsize,
}
struct Shared {
    frames: Box<[Slot]>,
    mask: usize,
    write: AtomicU64,
    read: AtomicU64,
}
pub struct Producer {
    shared: Arc<Shared>,
}
pub struct Consumer {
    shared: Arc<Shared>,
}

/// Allocates one producer and one consumer before either I/O callback starts.
/// Each endpoint has one owner. Returns `None` unless capacity is a power of
/// two in `2..=1_048_576`, or if frame storage cannot be reserved.
#[must_use]
pub fn stereo_queue(capacity: usize) -> Option<(Producer, Consumer)> {
    if !capacity.is_power_of_two() || !(2..=1_048_576).contains(&capacity) {
        return None;
    }
    let mut frames = Vec::new();
    frames.try_reserve_exact(capacity).ok()?;
    frames.extend((0..capacity).map(|_| Slot {
        stereo: AtomicU64::new(0),
        generation: AtomicU64::new(0),
        publication_frames: AtomicUsize::new(0),
    }));
    let shared = Arc::new(Shared {
        frames: frames.into_boxed_slice(),
        mask: capacity - 1,
        write: AtomicU64::new(0),
        read: AtomicU64::new(0),
    });
    Some((
        Producer {
            shared: Arc::clone(&shared),
        },
        Consumer { shared },
    ))
}

impl Producer {
    /// Return the accepted prefix. Full queues never overwrite unread samples.
    /// The capture callback owns the policy/counter for the remaining frames.
    #[must_use]
    pub fn push(&mut self, input: &[StereoFrame]) -> usize {
        self.push_generation(input, 1)
    }

    /// Each accepted frame retains the offered slice length, including partial writes.
    /// Empty, rejected, and zero-generation writes publish no packet metadata.
    #[must_use]
    pub fn push_generation(&mut self, input: &[StereoFrame], generation: u64) -> usize {
        if generation == 0 {
            return 0;
        }
        let shared = &self.shared;
        let write = shared.write.load(Ordering::Relaxed);
        let read = shared.read.load(Ordering::Acquire);
        let used = write.wrapping_sub(read) as usize;
        let available = shared.frames.len().saturating_sub(used);
        let count = input.len().min(available);
        let mut accepted = 0;
        let accepted_frames = input.iter().take(count);
        for frame in accepted_frames {
            let index = write.wrapping_add(accepted as u64) as usize & shared.mask;
            let Some(slot) = shared.frames.get(index) else {
                break;
            };
            slot.stereo.store(frame.pack(), Ordering::Relaxed);
            slot.generation.store(generation, Ordering::Relaxed);
            slot.publication_frames
                .store(input.len(), Ordering::Relaxed);
            accepted += 1;
        }
        // Publish the complete prefix after all its sample writes.
        shared
            .write
            .store(write.wrapping_add(accepted as u64), Ordering::Release);
        accepted
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[must_use]
pub struct PopResult {
    pub received: usize,
    pub discarded: usize,
    /// Largest offered publication among matching frames copied into output.
    pub publication_frames: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GenerationView {
    pub pending: usize,
    pub publication_frames: usize,
}
impl Consumer {
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.frames.len()
    }

    #[must_use]
    pub fn available(&self) -> usize {
        let read = self.shared.read.load(Ordering::Relaxed);
        let write = self.shared.write.load(Ordering::Acquire);
        (write.wrapping_sub(read) as usize).min(self.shared.frames.len())
    }

    #[must_use]
    pub(crate) fn generation_view(&self, generation: u64) -> GenerationView {
        let shared = &self.shared;
        let read = shared.read.load(Ordering::Relaxed);
        let write = shared.write.load(Ordering::Acquire);
        let count = (write.wrapping_sub(read) as usize).min(shared.frames.len());
        let mut view = GenerationView::default();
        for offset in 0..count {
            let index = read.wrapping_add(offset as u64) as usize & shared.mask;
            let slot = &shared.frames[index];
            let stored_generation = slot.generation.load(Ordering::Relaxed);
            if stored_generation > generation {
                break;
            }
            if stored_generation == generation {
                view.pending += 1;
                view.publication_frames = view
                    .publication_frames
                    .max(slot.publication_frames.load(Ordering::Relaxed));
            }
        }
        view
    }

    /// Return the valid prefix. The remainder of output is untouched.
    #[must_use]
    pub fn pop(&mut self, output: &mut [StereoFrame]) -> usize {
        self.pop_generation(output, 1).received
    }

    /// Discard only older generations. A future generation remains queued so
    /// an output callback racing with a control change cannot erase new audio.
    /// Empty output can be used to retire the older prefix during a reset.
    pub fn pop_generation(&mut self, output: &mut [StereoFrame], generation: u64) -> PopResult {
        if generation == 0 {
            return PopResult::default();
        }
        let shared = &self.shared;
        let read = shared.read.load(Ordering::Relaxed);
        let write = shared.write.load(Ordering::Acquire);
        let count = (write.wrapping_sub(read) as usize).min(shared.frames.len());
        let mut received = 0;
        let mut consumed = 0;
        let mut discarded = 0;
        let mut publication_frames = 0;
        while consumed < count {
            let index = read.wrapping_add(consumed as u64) as usize & shared.mask;
            let Some(slot) = shared.frames.get(index) else {
                break;
            };
            let stored_generation = slot.generation.load(Ordering::Relaxed);
            if stored_generation < generation {
                consumed += 1;
                discarded += 1;
                continue;
            }
            if stored_generation != generation || received == output.len() {
                break;
            }
            let Some(frame) = output.get_mut(received) else {
                break;
            };
            *frame = StereoFrame::unpack(slot.stereo.load(Ordering::Relaxed));
            publication_frames =
                publication_frames.max(slot.publication_frames.load(Ordering::Relaxed));
            received += 1;
            consumed += 1;
        }
        // Reclaim slots only after the consumer has copied the entire prefix.
        shared
            .read
            .store(read.wrapping_add(consumed as u64), Ordering::Release);
        PopResult {
            received,
            discarded,
            publication_frames,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generations_do_not_replay_old_input_or_erase_future_input() {
        let (mut producer, mut consumer) =
            super::stereo_queue(8).expect("fixture queue capacity is a nonzero power of two");
        let old = [StereoFrame::new(0.1, -0.1); 3];
        let new = [StereoFrame::new(0.7, -0.7); 3];
        let _ = producer.push_generation(&old, 1);
        let _ = producer.push_generation(&new, 2);
        let mut output = [StereoFrame::SILENCE; 8];
        assert_eq!(
            consumer.pop_generation(&mut output, 1),
            PopResult {
                received: 3,
                discarded: 0,
                publication_frames: 3,
            }
        );
        assert_eq!(consumer.available(), 3);
        assert_eq!(
            consumer.pop_generation(&mut output, 2),
            PopResult {
                received: 3,
                discarded: 0,
                publication_frames: 3,
            }
        );
        assert_eq!(&output[..3], &new);
        let _ = producer.push_generation(&old, 2);
        let _ = producer.push_generation(&new, 3);
        assert_eq!(
            consumer.pop_generation(&mut [], 3),
            PopResult {
                received: 0,
                discarded: 3,
                publication_frames: 0,
            }
        );
        assert_eq!(consumer.pop_generation(&mut output, 3).received, 3);
        assert_eq!(&output[..3], &new);
    }
    #[test]
    fn shortage_full_queue_and_counter_wrap_preserve_order() {
        let (mut producer, mut consumer) =
            super::stereo_queue(4).expect("fixture queue capacity is a nonzero power of two");
        producer.shared.write.store(u64::MAX - 1, Ordering::Relaxed);
        producer.shared.read.store(u64::MAX - 1, Ordering::Relaxed);
        let input: Vec<_> = (0..6)
            .map(|i| StereoFrame::new(i as f32, -(i as f32)))
            .collect();
        assert_eq!(producer.push(&input), 4);
        assert_eq!(producer.push(&input[4..]), 0);
        let mut output = [StereoFrame::SILENCE; 6];
        assert_eq!(consumer.pop(&mut output[..2]), 2);
        assert_eq!(producer.push(&input[4..]), 2);
        assert_eq!(consumer.pop(&mut output[2..]), 4);
        assert_eq!(&output[..], &input[..]);
        assert_eq!(consumer.available(), 0);
        assert_eq!(consumer.pop(&mut output), 0);
        assert_eq!(&output[..], &input[..]);
    }

    #[test]
    fn publication_metadata_preserves_partial_writes_and_generation_barriers() {
        let (mut producer, mut consumer) =
            super::stereo_queue(8).expect("fixture capacity is supported");
        let pcm = [StereoFrame::new(0.5, -0.5); 16];
        let generation = u64::MAX - 1;
        assert_eq!(producer.push_generation(&pcm[..2], generation - 1), 2);
        assert_eq!(producer.push_generation(&pcm, generation), 6);
        assert_eq!(producer.push_generation(&pcm, generation), 0);
        assert_eq!(producer.push_generation(&[], generation), 0);
        assert_eq!(producer.push_generation(&pcm, 0), 0);
        assert_eq!(
            consumer.generation_view(generation),
            GenerationView {
                pending: 6,
                publication_frames: 16
            }
        );
        let mut output = [StereoFrame::SILENCE; 3];
        assert_eq!(
            consumer.pop_generation(&mut output, generation),
            PopResult {
                received: 3,
                discarded: 2,
                publication_frames: 16
            }
        );
        assert_eq!(output, [pcm[0]; 3]);
        assert_eq!(producer.push_generation(&pcm[..2], u64::MAX), 2);
        assert_eq!(producer.push_generation(&pcm[..1], generation), 1);
        assert_eq!(consumer.generation_view(generation).pending, 3);
        assert_eq!(consumer.generation_view(generation).publication_frames, 16);
        assert_eq!(consumer.pop_generation(&mut output, generation).received, 3);
        assert_eq!(
            consumer.generation_view(generation),
            GenerationView::default()
        );
        assert_eq!(consumer.pop_generation(&mut output, generation).received, 0);
        assert_eq!(consumer.pop_generation(&mut output, u64::MAX).received, 2);
        assert_eq!(consumer.available(), 0);
    }

    #[test]
    fn transfer_observes_a_publication_after_the_queue_snapshot() {
        let (mut producer, mut consumer) =
            super::stereo_queue(8).expect("fixture capacity is supported");
        assert_eq!(consumer.generation_view(1), GenerationView::default());
        let pcm = [StereoFrame::new(0.25, -0.25); 7];
        assert_eq!(producer.push(&pcm), 7);
        let mut output = [StereoFrame::SILENCE; 3];
        let report = consumer.pop_generation(&mut output, 1);
        assert_eq!(report.received, 3);
        assert_eq!(report.publication_frames, 7);
        assert_eq!(output, [pcm[0]; 3]);
    }

    #[test]
    fn independent_callbacks_publish_samples_and_packet_metadata_together() {
        let (mut producer, mut consumer) =
            super::stereo_queue(64).expect("fixture capacity is supported");
        let writer = std::thread::spawn(move || {
            for publication in 0..10_000 {
                let offered = publication % 17 + 1;
                let pcm = [StereoFrame::new(offered as f32, publication as f32); 17];
                let generation = u64::MAX - 9 + publication as u64 / 1000;
                loop {
                    let write = producer.shared.write.load(Ordering::Relaxed);
                    let read = producer.shared.read.load(Ordering::Acquire);
                    if 64 - write.wrapping_sub(read) as usize >= offered {
                        break;
                    }
                    std::thread::yield_now();
                }
                assert_eq!(
                    producer.push_generation(&pcm[..offered], generation),
                    offered
                );
            }
        });
        let reader = std::thread::spawn(move || {
            let mut output = [StereoFrame::SILENCE; 1];
            let mut publication = 0;
            let mut offset = 0;
            while publication < 10_000 {
                let generation = u64::MAX - 9 + publication as u64 / 1000;
                let report = consumer.pop_generation(&mut output, generation);
                if report.received == 0 {
                    std::thread::yield_now();
                    continue;
                }
                let offered = publication % 17 + 1;
                assert_eq!(report.discarded, 0);
                assert_eq!(report.publication_frames, offered);
                assert_eq!(
                    output[0],
                    StereoFrame::new(offered as f32, publication as f32)
                );
                offset += 1;
                if offset == offered {
                    publication += 1;
                    offset = 0;
                }
            }
        });
        writer.join().expect("fixture producer completes");
        reader.join().expect("fixture consumer completes");
    }

    #[test]
    fn independent_callbacks_transfer_without_torn_pairs() {
        let (mut producer, mut consumer) =
            super::stereo_queue(128).expect("fixture queue capacity is a nonzero power of two");
        let writer = std::thread::spawn(move || {
            for index in 0..50_000 {
                let frame = StereoFrame::new(index as f32, -(index as f32));
                while producer.push(&[frame]) == 0 {
                    std::thread::yield_now();
                }
            }
        });
        let reader = std::thread::spawn(move || {
            let mut block = [StereoFrame::SILENCE; 63];
            let mut expected = 0;
            while expected < 50_000 {
                let count = consumer.pop(&mut block);
                let received_frames = block.iter().take(count);
                for frame in received_frames {
                    assert_eq!(
                        *frame,
                        StereoFrame::new(expected as f32, -(expected as f32))
                    );
                    expected += 1;
                }
                if count == 0 {
                    std::thread::yield_now();
                }
            }
        });
        writer
            .join()
            .expect("fixture worker completes without panic");
        reader
            .join()
            .expect("fixture worker completes without panic");
    }
}
