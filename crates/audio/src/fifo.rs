//! App-side single-producer/single-consumer queue, distinct from the HAL ring.
//! Endpoints are not Clone, and operations require exclusive access to their
//! endpoint. No allocation, waiting, locks or retry loop occurs during transfer.
use crate::StereoFrame;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct Shared {
    frames: Box<[AtomicU64]>,
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
    frames.extend((0..capacity).map(|_| AtomicU64::new(0)));
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
        let shared = &self.shared;
        let write = shared.write.load(Ordering::Relaxed);
        let read = shared.read.load(Ordering::Acquire);
        let used = write.wrapping_sub(read) as usize;
        let available = shared.frames.len().saturating_sub(used);
        let count = input.len().min(available);
        let mut accepted = 0;
        for frame in input.iter().take(count) {
            let index = write.wrapping_add(accepted as u64) as usize & shared.mask;
            let Some(slot) = shared.frames.get(index) else {
                break;
            };
            slot.store(frame.pack(), Ordering::Relaxed);
            accepted += 1;
        }
        // Publish the complete prefix after all its sample writes.
        shared
            .write
            .store(write.wrapping_add(accepted as u64), Ordering::Release);
        accepted
    }
}
impl Consumer {
    pub fn available(&self) -> usize {
        let read = self.shared.read.load(Ordering::Relaxed);
        let write = self.shared.write.load(Ordering::Acquire);
        (write.wrapping_sub(read) as usize).min(self.shared.frames.len())
    }

    /// Return the valid prefix. The remainder of output is untouched.
    pub fn pop(&mut self, output: &mut [StereoFrame]) -> usize {
        let shared = &self.shared;
        let read = shared.read.load(Ordering::Relaxed);
        let write = shared.write.load(Ordering::Acquire);
        let count = output
            .len()
            .min(write.wrapping_sub(read) as usize)
            .min(shared.frames.len());
        let mut received = 0;
        for frame in output.iter_mut().take(count) {
            let index = read.wrapping_add(received as u64) as usize & shared.mask;
            let Some(slot) = shared.frames.get(index) else {
                break;
            };
            *frame = StereoFrame::unpack(slot.load(Ordering::Relaxed));
            received += 1;
        }
        // Reclaim slots only after the consumer has copied the entire prefix.
        shared
            .read
            .store(read.wrapping_add(received as u64), Ordering::Release);
        received
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortage_full_queue_and_counter_wrap_preserve_order() {
        let (mut producer, mut consumer) = stereo_queue(4).unwrap();
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
        let (mut producer, mut consumer) = stereo_queue(128).unwrap();
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
                for frame in block.iter().take(count) {
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
        writer.join().unwrap();
        reader.join().unwrap();
    }
}
