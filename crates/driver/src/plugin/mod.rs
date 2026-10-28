mod host_boundary;
mod properties;
mod state;
mod system;

// AudioServerPlugIn.h requires at least 10,923 frames between zero timestamps.
const ZERO_TIMESTAMP_PERIOD: u32 = 16_384;

#[cfg(test)]
mod tests {
    use super::ZERO_TIMESTAMP_PERIOD;
    use channelbridge_audio::device_clock::DeviceClock;

    #[test]
    fn zero_timestamp_period_boundaries() {
        let anchor = 100;
        let frequency = 24_000_000;
        for rate in [44_100, 48_000, 768_000] {
            let clock = DeviceClock::new(anchor, frequency, rate, ZERO_TIMESTAMP_PERIOD, 7)
                .expect("fixture clock parameters are nonzero");
            let first_period_ticks =
                (u64::from(ZERO_TIMESTAMP_PERIOD) * frequency).div_ceil(u64::from(rate));
            let before = clock
                .timestamp(anchor + first_period_ticks - 1)
                .expect("fixture time follows the anchor and fits the timestamp range");
            assert_eq!(before.sample_frame, 0);
            assert_eq!(before.host_ticks, anchor);
            let at = clock
                .timestamp(anchor + first_period_ticks)
                .expect("fixture time follows the anchor and fits the timestamp range");
            assert_eq!(at.sample_frame, u64::from(ZERO_TIMESTAMP_PERIOD));
            assert_eq!(
                at.host_ticks,
                anchor + u64::from(ZERO_TIMESTAMP_PERIOD) * frequency / u64::from(rate)
            );
            assert_eq!(at.seed, before.seed);
            assert_eq!(at.seed, 7);
        }
    }
}
