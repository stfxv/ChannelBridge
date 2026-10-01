//! Single capture writer, independent read-only UI snapshots. A 50 ms history
//! keeps a three-second peak even if the UI skips a refresh. Readers never clear
//! shared peaks. All fields are atomic and all retry/work bounds are finite.
use crate::frame::finite;
#[cfg(feature = "model-check")]
use loom::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(not(feature = "model-check"))]
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const BUCKETS: usize = 64;
const BUCKETS_PER_SECOND: u64 = 20;
struct Slot {
    sequence: AtomicU64,
    bucket: AtomicU64,
    peak: AtomicU32,
}
impl Slot {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            bucket: AtomicU64::new(u64::MAX),
            peak: AtomicU32::new(0),
        }
    }
    fn publish(&self, sequence: u64, bucket: u64, peak: f32) {
        // Each Release publication prevents a reader accepting a new field
        // while its final sequence check still observes the old sequence.
        self.sequence.store(sequence - 1, Ordering::Release);
        self.bucket.store(bucket, Ordering::Release);
        self.peak.store(peak.to_bits(), Ordering::Release);
        self.sequence.store(sequence, Ordering::Release);
    }
    fn read(&self, expected: u64) -> Option<f32> {
        let before = self.sequence.load(Ordering::Acquire);
        if before & 1 != 0 {
            return None;
        }
        let stored = self.bucket.load(Ordering::Acquire);
        let value = self.peak.load(Ordering::Acquire);
        let after = self.sequence.load(Ordering::Acquire);
        (before == after && stored == expected).then(|| f32::from_bits(value))
    }
}
struct Shared {
    ticks_per_second: u64,
    levels: Box<[AtomicU32]>,
    updated: Box<[AtomicU64]>,
    history: Box<[Slot]>,
}
#[derive(Clone, Copy)]
struct Current {
    bucket: u64,
    peak: f32,
    sequence: u64,
    tick: u64,
}
pub struct MeterWriter {
    shared: Arc<Shared>,
    current: Box<[Current]>,
}
#[derive(Clone)]
pub struct MeterReader {
    shared: Arc<Shared>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Level {
    pub current: f32,
    pub held_peak: f32,
    /// A callback updated this channel within the last 250 ms.
    pub fresh: bool,
}

fn allocated<T>(count: usize, mut make: impl FnMut() -> T) -> Option<Box<[T]>> {
    let mut values = Vec::new();
    values.try_reserve_exact(count).ok()?;
    values.extend((0..count).map(|_| make()));
    Some(values.into_boxed_slice())
}
pub fn meter_bank(channels: usize, ticks_per_second: u64) -> Option<(MeterWriter, MeterReader)> {
    if !(1..=512).contains(&channels) || ticks_per_second == 0 {
        return None;
    }
    let levels = allocated(channels, || AtomicU32::new(0))?;
    let updated = allocated(channels, || AtomicU64::new(u64::MAX))?;
    let history = allocated(channels * BUCKETS, Slot::new)?;
    let current = allocated(channels, || Current {
        bucket: u64::MAX,
        peak: 0.0,
        sequence: 0,
        tick: 0,
    })?;
    let shared = Arc::new(Shared {
        ticks_per_second,
        levels,
        updated,
        history,
    });
    Some((
        MeterWriter {
            shared: Arc::clone(&shared),
            current,
        },
        MeterReader { shared },
    ))
}
impl Shared {
    fn bucket(&self, tick: u64) -> Option<u64> {
        u64::try_from(
            u128::from(tick) * u128::from(BUCKETS_PER_SECOND) / u128::from(self.ticks_per_second),
        )
        .ok()
    }
}
impl MeterWriter {
    pub fn channels(&self) -> usize {
        self.current.len()
    }
    pub fn record(&mut self, channel: usize, peak: f32, tick: u64) -> bool {
        let Some(bucket) = self.shared.bucket(tick) else {
            return false;
        };
        let Some(state) = self.current.get_mut(channel) else {
            return false;
        };
        if tick < state.tick {
            return false;
        }
        let peak = finite(peak).abs();
        let sequence = match state.sequence.checked_add(2) {
            Some(v) => v,
            None => return false,
        };
        let held = if bucket == state.bucket {
            state.peak.max(peak)
        } else {
            peak
        };
        let index = channel * BUCKETS + (bucket % BUCKETS as u64) as usize;
        let Some(slot) = self.shared.history.get(index) else {
            return false;
        };
        slot.publish(sequence, bucket, held);
        if let Some(level) = self.shared.levels.get(channel) {
            level.store(peak.to_bits(), Ordering::Release);
        }
        if let Some(updated) = self.shared.updated.get(channel) {
            updated.store(tick, Ordering::Release);
        }
        *state = Current {
            bucket,
            peak: held,
            sequence,
            tick,
        };
        true
    }
}
impl MeterReader {
    pub fn level(&self, channel: usize, now: u64) -> Level {
        let Some(bucket) = self.shared.bucket(now) else {
            return Level::default();
        };
        let Some(updated) = self.shared.updated.get(channel) else {
            return Level::default();
        };
        let tick = updated.load(Ordering::Acquire);
        let fresh =
            tick != u64::MAX && now >= tick && now - tick <= self.shared.ticks_per_second / 4;
        let current = if fresh {
            self.shared
                .levels
                .get(channel)
                .map(|v| f32::from_bits(v.load(Ordering::Acquire)))
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let mut peak = 0_f32;
        for age in 0..=3 * BUCKETS_PER_SECOND {
            let Some(expected) = bucket.checked_sub(age) else {
                break;
            };
            let index = channel * BUCKETS + (expected % BUCKETS as u64) as usize;
            let Some(slot) = self.shared.history.get(index) else {
                break;
            };
            if let Some(value) = slot.read(expected) {
                peak = peak.max(value);
            }
        }
        Level {
            current,
            held_peak: peak,
            fresh,
        }
    }
}
#[cfg(all(test, not(feature = "model-check")))]
mod tests {
    use super::*;
    #[test]
    fn fresh_silence_is_distinct_from_missing_and_stale_callbacks() {
        let (mut writer, reader) = meter_bank(1, 1000).unwrap();
        assert!(!reader.level(0, 0).fresh);
        assert!(writer.record(0, 0.0, 0));
        assert!(reader.level(0, 0).fresh);
        assert!(reader.level(0, 250).fresh);
        assert_eq!(reader.level(0, 250).current, 0.0);
        assert!(!reader.level(0, 251).fresh);
        assert!(!reader.level(1, 0).fresh);
    }
    #[test]
    fn readers_do_not_clear_peaks_and_old_ring_slots_expire() {
        let (mut writer, reader) = meter_bank(10, 1000).unwrap();
        assert_eq!(reader.level(9, 100).held_peak, 0.0);
        writer.record(9, 0.8, 100);
        writer.record(9, 0.2, 120);
        assert_eq!(reader.level(9, 120).current, 0.2);
        assert_eq!(reader.level(9, 120).held_peak, 0.8);
        assert_eq!(reader.clone().level(9, 3000).held_peak, 0.8);
        assert_eq!(reader.level(9, 3200).held_peak, 0.0);
        assert_eq!(reader.level(9, 3200).current, 0.0);
        writer.record(9, 0.1, 3300);
        assert_eq!(reader.level(9, 3300).held_peak, 0.1);
        assert!(!writer.record(9, 0.9, 3000));
        assert_eq!(reader.level(9, 3300).held_peak, 0.1);
    }
}

#[cfg(all(test, feature = "model-check"))]
mod model_tests {
    use super::*;
    use loom::{sync::Arc, thread};
    #[test]
    fn overwritten_history_never_attributes_a_peak_to_the_wrong_bucket() {
        let mut model = loom::model::Builder::new();
        model.preemption_bound = Some(2);
        model.check(|| {
            let slot = Arc::new(Slot::new());
            slot.publish(2, 3, 0.25);
            let updating = Arc::clone(&slot);
            let writer = thread::spawn(move || updating.publish(4, 67, 0.75));
            let reading = Arc::clone(&slot);
            let reader = thread::spawn(move || {
                if let Some(peak) = reading.read(3) {
                    assert_eq!(peak, 0.25);
                }
                if let Some(peak) = reading.read(67) {
                    assert_eq!(peak, 0.75);
                }
            });
            writer.join().unwrap();
            reader.join().unwrap();
            assert_eq!(slot.read(3), None);
            assert_eq!(slot.read(67), Some(0.75));
        });
    }
}
