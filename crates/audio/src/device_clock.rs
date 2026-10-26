/// A virtual device clock, derived directly from the monotonic host clock.
/// No accumulated floating-point increments and no dependency on poll frequency.
#[derive(Clone, Copy, Debug)]
pub struct DeviceClock {
    anchor_ticks: u64,
    ticks_per_second: u64,
    sample_rate: u32,
    period_frames: u32,
    seed: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZeroTimestamp {
    pub sample_frame: u64,
    pub host_ticks: u64,
    pub seed: u64,
}

impl DeviceClock {
    pub fn new(
        anchor_ticks: u64,
        ticks_per_second: u64,
        sample_rate: u32,
        period_frames: u32,
        seed: u64,
    ) -> Option<Self> {
        if ticks_per_second == 0 || sample_rate == 0 || period_frames == 0 || seed == 0 {
            return None;
        }
        Some(Self {
            anchor_ticks,
            ticks_per_second,
            sample_rate,
            period_frames,
            seed,
        })
    }

    pub fn timestamp(self, now_ticks: u64) -> Option<ZeroTimestamp> {
        let elapsed = now_ticks.checked_sub(self.anchor_ticks)?;
        let frames =
            u128::from(elapsed) * u128::from(self.sample_rate) / u128::from(self.ticks_per_second);
        let period = u128::from(self.period_frames);
        let sample_frame = u64::try_from(frames / period * period).ok()?;
        let offset = u128::from(sample_frame) * u128::from(self.ticks_per_second)
            / u128::from(self.sample_rate);
        let host_ticks = self.anchor_ticks.checked_add(u64::try_from(offset).ok()?)?;
        Some(ZeroTimestamp {
            sample_frame,
            host_ticks,
            seed: self.seed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_poll_catches_up_in_one_call() {
        let clock = DeviceClock::new(100, 24_000_000, 48_000, 512, 7).expect("fixture clock frequency, sample rate, period and seed are nonzero");
        let t = clock.timestamp(100 + 24_000_000 * 900).expect("fixture time follows the anchor and fits the timestamp range");
        assert_eq!(t.sample_frame, 43_200_000 / 512 * 512);
        assert_eq!(t.seed, 7);
        assert!(clock.timestamp(99).is_none());
    }

    #[test]
    fn eight_hours_do_not_accumulate_drift() {
        for rate in [8_000, 32_000, 44_100, 48_000, 96_000, 192_000, 768_000] {
            let clock = DeviceClock::new(1234, 1_000_000_000, rate, 512, 1).expect("fixture clock frequency, sample rate, period and seed are nonzero");
            let now = 1234 + 28_800 * 1_000_000_000;
            let t = clock.timestamp(now).expect("fixture time follows the anchor and fits the timestamp range");
            let expected = u64::from(rate) * 28_800;
            assert!(t.sample_frame <= expected && expected - t.sample_frame < 512);
            assert!(t.host_ticks <= now);
            assert!(now - t.host_ticks <= 512 * 1_000_000_000 / u64::from(rate) + 1);
        }
    }

    #[test]
    fn invalid_clock_configuration_is_rejected() {
        assert!(DeviceClock::new(0, 0, 48_000, 512, 1).is_none());
        assert!(DeviceClock::new(0, 1, 0, 512, 1).is_none());
        assert!(DeviceClock::new(0, 1, 48_000, 0, 1).is_none());
        assert!(DeviceClock::new(0, 1, 48_000, 512, 0).is_none());
    }
}
