//! Clock estimation and slow queue feedback, independent of macOS APIs.
//! Each instance belongs to one callback. Observations are published separately
//! by the future HAL adapter, rather than sharing mutable estimator state.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Observation {
    Accumulating,
    Updated { speed: f64 },
    Discontinuity,
}

pub struct ClockEstimator {
    nominal_rate: f64,
    ticks_per_second: u64,
    anchor: Option<(u64, u64)>,
    last: Option<(u64, u64)>,
    speed: f64,
}
impl ClockEstimator {
    pub fn new(nominal_rate: u32, ticks_per_second: u64) -> Option<Self> {
        if nominal_rate == 0 || ticks_per_second == 0 {
            return None;
        }
        Some(Self {
            nominal_rate: f64::from(nominal_rate),
            ticks_per_second,
            anchor: None,
            last: None,
            speed: 1.0,
        })
    }
    pub fn speed(&self) -> f64 {
        self.speed
    }
    pub fn reset(&mut self) {
        self.anchor = None;
        self.last = None;
        self.speed = 1.0;
    }

    fn reanchor(&mut self, frame: u64, tick: u64) {
        self.anchor = Some((frame, tick));
        self.last = Some((frame, tick));
        self.speed = 1.0;
    }

    /// frame and tick describe the same device sample, not callback arrival.
    /// Non-monotonic time, excessive gaps and gross rate changes are reported
    /// as discontinuities; they must not become a tiny drift correction.
    pub fn observe(&mut self, frame: u64, tick: u64) -> Observation {
        let Some((last_frame, last_tick)) = self.last else {
            self.reanchor(frame, tick);
            return Observation::Accumulating;
        };
        if frame == last_frame && tick == last_tick {
            return Observation::Accumulating;
        }
        if frame <= last_frame || tick <= last_tick {
            self.reanchor(frame, tick);
            return Observation::Discontinuity;
        }
        let (anchor_frame, anchor_tick) = self.anchor.unwrap_or((last_frame, last_tick));
        let ticks = tick - anchor_tick;
        if ticks < self.ticks_per_second {
            self.last = Some((frame, tick));
            return Observation::Accumulating;
        }
        let seconds = ticks as f64 / self.ticks_per_second as f64;
        let observed_speed = (frame - anchor_frame) as f64 / (seconds * self.nominal_rate);
        if seconds > 3.0 || !(0.99..=1.01).contains(&observed_speed) {
            self.reanchor(frame, tick);
            return Observation::Discontinuity;
        }
        let alpha = seconds / (2.0 + seconds);
        self.speed += (observed_speed - self.speed) * alpha;
        self.anchor = Some((frame, tick));
        self.last = Some((frame, tick));
        Observation::Updated { speed: self.speed }
    }
}

pub struct DriftController {
    input_rate: f64,
    output_rate: f64,
    nominal_step: f64,
    target: f64,
    filtered_queue: f64,
    integrator: f64,
    correction: f64,
    measured_speed: f64,
}

impl DriftController {
    /// target is total pending input in FIFO + resampler working history.
    /// Caller sizes it for its block size, filter lookahead and arrival jitter.
    pub fn new(input_rate: u32, output_rate: u32, target: usize) -> Option<Self> {
        if input_rate == 0 || output_rate == 0 || !(1..=1_048_576).contains(&target) {
            return None;
        }
        Some(Self {
            input_rate: f64::from(input_rate),
            output_rate: f64::from(output_rate),
            nominal_step: f64::from(input_rate) / f64::from(output_rate),
            target: target as f64,
            filtered_queue: target as f64,
            integrator: 0.0,
            correction: 0.0,
            measured_speed: 1.0,
        })
    }

    pub fn correction_ppm(&self) -> f64 {
        self.correction * 1_000_000.0
    }
    pub fn reset(&mut self) {
        self.filtered_queue = self.target;
        self.integrator = 0.0;
        self.correction = 0.0;
        self.measured_speed = 1.0;
    }
    pub fn step(&self) -> f64 {
        self.nominal_step * (1.0 + self.correction)
    }

    /// Call after output consumption, passing that block's frame count.
    /// Queue feedback is low-pass filtered; correction has both absolute and
    /// slew limits, so a scheduling fluctuation cannot jump the sample position.
    pub fn update(&mut self, queued_input: usize, output_frames: usize, speed: Option<f64>) -> f64 {
        if output_frames == 0 || output_frames > 4096 || queued_input > 1_048_576 {
            return self.step();
        }
        if let Some(speed) = speed.filter(|s| s.is_finite() && (0.998..=1.002).contains(s)) {
            self.measured_speed = speed;
        }
        let seconds = output_frames as f64 / self.output_rate;
        let alpha = seconds / (0.5 + seconds);
        self.filtered_queue += (queued_input as f64 - self.filtered_queue) * alpha;
        let error_seconds = (self.filtered_queue - self.target) / self.input_rate;
        let proposed_integrator =
            (self.integrator + 0.02 * error_seconds * seconds).clamp(-0.0015, 0.0015);
        let requested = self.measured_speed - 1.0 + 0.1 * error_seconds + proposed_integrator;
        // Anti-windup: do not integrate further into the saturated direction.
        if requested.abs() <= 0.002 || requested * error_seconds <= 0.0 {
            self.integrator = proposed_integrator;
        }
        let requested = (self.measured_speed - 1.0 + 0.1 * error_seconds + self.integrator)
            .clamp(-0.002, 0.002);
        let maximum_change = 0.0002 * seconds;
        self.correction += (requested - self.correction).clamp(-maximum_change, maximum_change);
        self.step()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_estimate_speed_and_discontinuities_are_not_drift() {
        let mut estimator = ClockEstimator::new(48_000, 24_000_000)
            .expect("fixture sample rate and host clock frequency are positive");
        for second in 0..30 {
            let frame = (second as f64 * 48_000.0 * 1.001).round() as u64;
            estimator.observe(frame, second * 24_000_000);
        }
        assert!((estimator.speed() - 1.001).abs() < 0.00001);
        assert_eq!(estimator.observe(0, 0), Observation::Discontinuity);
        assert_eq!(estimator.speed(), 1.0);
        assert_eq!(estimator.observe(0, 0), Observation::Accumulating);
        assert_eq!(
            estimator.observe(96_000, 24_000_000),
            Observation::Discontinuity
        );
    }

    #[test]
    fn correction_is_bounded_and_invalid_observations_do_not_poison_it() {
        let mut controller = DriftController::new(48_000, 48_000, 704)
            .expect("fixture rates and target buffer size are positive");
        let step = controller.update(4000, 512, Some(f64::NAN));
        assert!((step - 1.0).abs() <= 0.0002 * 512.0 / 48_000.0 + f64::EPSILON);
        for _ in 0..10_000 {
            controller.update(4000, 512, Some(1.001));
        }
        assert!(controller.correction_ppm() <= 2000.0);
        let previous = controller.step();
        assert_eq!(controller.update(usize::MAX, 512, Some(1.0)), previous);
        assert_eq!(controller.update(704, 0, Some(1.0)), previous);
    }

    #[test]
    fn eight_hour_clock_and_queue_model_stays_bounded() {
        // Controller-only virtual-time model: no HAL, PCM or real-time wait.
        // ±2 ms here perturbs the observed queue, not actual callback deadlines.
        for ppm in [-1000.0, -500.0, -100.0, 100.0, 500.0, 1000.0] {
            let target = 4608.0;
            let actual_speed = 1.0 + ppm / 1_000_000.0;
            let mut controller = DriftController::new(48_000, 48_000, target as usize)
                .expect("fixture rates and target buffer size are positive");
            let mut queue = target;
            let mut elapsed = 0.0;
            let mut iteration = 0;
            let mut maximum_deviation = 0_f64;
            while elapsed < 8.0 * 3600.0 {
                let frames = [64, 128, 256, 512, 1024, 4096][iteration % 6];
                queue += frames as f64 * (actual_speed - controller.step());
                maximum_deviation = maximum_deviation.max((queue - target).abs());
                let jitter = if iteration % 2 == 0 { 96.0 } else { -96.0 };
                controller.update(
                    (queue + jitter).round().max(0.0) as usize,
                    frames,
                    (elapsed >= 1.0).then_some(actual_speed),
                );
                elapsed += frames as f64 / 48_000.0;
                iteration += 1;
            }
            assert!(
                maximum_deviation < 512.0,
                "ppm={ppm} deviation={maximum_deviation}"
            );
            assert!(
                (controller.correction_ppm() - ppm).abs() < 25.0,
                "ppm={ppm} correction={}",
                controller.correction_ppm()
            );
        }
    }
}
