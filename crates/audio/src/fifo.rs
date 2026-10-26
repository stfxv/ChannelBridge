//! App-side single-producer/single-consumer queue, distinct from the HAL ring.
//! Endpoints are not Clone, and operations require exclusive access to their
//! endpoint. No allocation, waiting, locks or retry loop occurs during transfer.
use crate::StereoFrame;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct Slot {
    stereo: AtomicU64,
    generation: AtomicU64,
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

/// Allocate the queue before starting either I/O callback.
pub fn stereo_queue(capacity: usize) -> Option<(Producer, Consumer)> {
    if !capacity.is_power_of_two() || !(2..=1_048_576).contains(&capacity) {
        return None;
    }
    let mut frames = Vec::new();
    frames.try_reserve_exact(capacity).ok()?;
    frames.extend((0..capacity).map(|_| Slot {
        stereo: AtomicU64::new(0),
        generation: AtomicU64::new(0),
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
    pub fn push(&mut self, input: &[StereoFrame]) -> usize {
        self.push_generation(input, 1)
    }

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
pub struct PopResult {
    pub received: usize,
    pub discarded: usize,
}
impl Consumer {
    pub fn capacity(&self) -> usize {
        self.shared.frames.len()
    }

    pub fn available(&self) -> usize {
        let read = self.shared.read.load(Ordering::Relaxed);
        let write = self.shared.write.load(Ordering::Acquire);
        (write.wrapping_sub(read) as usize).min(self.shared.frames.len())
    }

    /// Return the valid prefix. The remainder of output is untouched.
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generations_do_not_replay_old_input_or_erase_future_input() {
        let (mut producer, mut consumer) =
            stereo_queue(8).expect("fixture queue capacity is a nonzero power of two");
        let old = [StereoFrame::new(0.1, -0.1); 3];
        let new = [StereoFrame::new(0.7, -0.7); 3];
        producer.push_generation(&old, 1);
        producer.push_generation(&new, 2);
        let mut output = [StereoFrame::SILENCE; 8];
        assert_eq!(
            consumer.pop_generation(&mut output, 1),
            PopResult {
                received: 3,
                discarded: 0
            }
        );
        assert_eq!(consumer.available(), 3);
        assert_eq!(
            consumer.pop_generation(&mut output, 2),
            PopResult {
                received: 3,
                discarded: 0
            }
        );
        assert_eq!(&output[..3], &new);
        producer.push_generation(&old, 2);
        producer.push_generation(&new, 3);
        assert_eq!(
            consumer.pop_generation(&mut [], 3),
            PopResult {
                received: 0,
                discarded: 3
            }
        );
        assert_eq!(consumer.pop_generation(&mut output, 3).received, 3);
        assert_eq!(&output[..3], &new);
    }
    #[test]
    fn shortage_full_queue_and_counter_wrap_preserve_order() {
        let (mut producer, mut consumer) =
            stereo_queue(4).expect("fixture queue capacity is a nonzero power of two");
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
    fn independent_callbacks_transfer_without_torn_pairs() {
        let (mut producer, mut consumer) =
            stereo_queue(128).expect("fixture queue capacity is a nonzero power of two");
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
