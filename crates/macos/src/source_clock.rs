//! Atomic publication of source speed together with its stream generation.
use std::sync::atomic::{AtomicU64, Ordering};
const SHIFT: u32 = 23;
const MASK: u64 = (1 << SHIFT) - 1;
const CENTER: i64 = 1 << 22;
const SCALE: f64 = 100_000_000.0;
#[derive(Default)]
pub struct SourceClock(AtomicU64);
impl SourceClock {
    /// One bounded CAS; stale callbacks cannot overwrite a newer generation.
    pub fn publish(&self, generation: u64, speed: Option<f64>) -> bool {
        if generation == 0 || generation > u64::MAX >> SHIFT {
            return false;
        }
        let encoded = speed
            .filter(|speed| speed.is_finite() && (0.99..=1.01).contains(speed))
            .map(|speed| (CENTER + ((speed - 1.0) * SCALE).round() as i64) as u64)
            .unwrap_or(0);
        let old = self.0.load(Ordering::Acquire);
        if old >> SHIFT > generation {
            return false;
        }
        self.0
            .compare_exchange(
                old,
                (generation << SHIFT) | encoded,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_ok()
    }
    pub fn speed(&self, generation: u64) -> Option<f64> {
        self.snapshot()
            .filter(|(observed, _)| *observed == generation)
            .and_then(|(_, speed)| speed)
    }
    /// A single atomic word keeps diagnostic speed and generation paired too.
    pub fn snapshot(&self) -> Option<(u64, Option<f64>)> {
        let word = self.0.load(Ordering::Acquire);
        let encoded = word & MASK;
        let generation = word >> SHIFT;
        (generation != 0).then(|| {
            (
                generation,
                (encoded != 0).then(|| 1.0 + (encoded as i64 - CENTER) as f64 / SCALE),
            )
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_snapshot_is_generation_tagged_and_rejects_invalid_speed() {
        let clock = SourceClock::default();
        assert_eq!(clock.speed(1), None);
        assert_eq!(clock.snapshot(), None);
        assert!(clock.publish(2, Some(1.000_123_456)));
        assert!((clock.speed(2).expect("the published matching generation contains a finite positive source speed") - 1.000_123_456).abs() <= 0.000_000_005);
        assert_eq!(clock.speed(1), None);
        assert_eq!(clock.snapshot().expect("publish installed a nonzero clock generation").0, 2);
        assert!(!clock.publish(1, Some(1.0)));
        assert!(clock.publish(3, Some(f64::NAN)));
        assert_eq!(clock.speed(3), None);
        assert_eq!(clock.snapshot(), Some((3, None)));
        assert!(clock.publish(4, Some(1.5)));
        assert_eq!(clock.speed(4), None);
    }
}
