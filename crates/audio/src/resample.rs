//! Own bandlimited interpolation, prepared before I/O starts.
//! Windowed sinc construction: Julius O. Smith, Physical Audio Signal Processing,
//! https://dsprelated.com/freebooks/pasp/Windowed_Sinc_Interpolation.html
//! The table is shared across fractional phases; callback work uses only table
//! interpolation and finite sums, with no allocation or trigonometric calls.
use crate::{StereoFrame, frame::finite};

const HALF_WIDTH: usize = 96;
const DENSITY: usize = 512;
const BETA: f64 = 12.0;

pub struct SincKernel {
    table: Box<[f64]>,
    scale: f64,
    radius: usize,
    maximum_step: f64,
}

impl SincKernel {
    /// step = input frames consumed per output frame. Include the permitted
    /// clock correction in maximum_step; changing the filter requires a restart.
    pub fn new(maximum_step: f64) -> Option<Self> {
        if !maximum_step.is_finite() || !(1.0 / 128.0..=128.0).contains(&maximum_step) {
            return None;
        }
        let scale = 1.0 / maximum_step.max(1.0);
        let radius = (HALF_WIDTH as f64 / scale).ceil() as usize;
        let length = HALF_WIDTH * DENSITY + 2;
        let mut table = Vec::new();
        table.try_reserve_exact(length).ok()?;
        let normalizer = bessel_i0(BETA);
        for index in 0..length {
            let x = index as f64 / DENSITY as f64;
            let coefficient = if x > HALF_WIDTH as f64 {
                0.0
            } else {
                let window =
                    bessel_i0(BETA * (1.0 - (x / HALF_WIDTH as f64).powi(2)).max(0.0).sqrt())
                        / normalizer;
                let argument = std::f64::consts::PI * 0.9 * x;
                let sinc = if argument == 0.0 {
                    1.0
                } else {
                    argument.sin() / argument
                };
                0.9 * sinc * window
            };
            table.push(coefficient);
        }
        Some(Self {
            table: table.into_boxed_slice(),
            scale,
            radius,
            maximum_step,
        })
    }

    pub fn radius(&self) -> usize {
        self.radius
    }

    fn weight(&self, distance: f64) -> f64 {
        let position = distance.abs() * self.scale * DENSITY as f64;
        if position > (HALF_WIDTH * DENSITY) as f64 {
            return 0.0;
        }
        let index = position as usize;
        let fraction = position - index as f64;
        match (self.table.get(index), self.table.get(index + 1)) {
            (Some(left), Some(right)) => left + (right - left) * fraction,
            _ => 0.0,
        }
    }

    /// sample receives an index in the 2*radius+1 input window. All phases have
    /// unity DC gain; left and right use exactly the same weights and position.
    fn interpolate(
        &self,
        fraction: f64,
        mut sample: impl FnMut(usize) -> StereoFrame,
    ) -> StereoFrame {
        let mut left = 0.0;
        let mut right = 0.0;
        let mut total = 0.0;
        for index in 0..=2 * self.radius {
            let weight = self.weight(index as f64 - self.radius as f64 - fraction);
            let frame = sample(index);
            left += f64::from(finite(frame.left)) * weight;
            right += f64::from(finite(frame.right)) * weight;
            total += weight;
        }
        if total.abs() < 1e-12 {
            return StereoFrame::SILENCE;
        }
        StereoFrame::new((left / total) as f32, (right / total) as f32)
    }
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for index in 1..=64 {
        term *= x * x / (4.0 * f64::from(index * index));
        sum += term;
        if term < sum * 1e-16 {
            break;
        }
    }
    sum
}

/// Owned only by the virtual-output callback. Input FIFO samples are copied in
/// using push; capture never accesses this non-atomic working history.
pub struct StreamingResampler {
    kernel: SincKernel,
    history: Box<[StereoFrame]>,
    mask: usize,
    oldest: u64,
    written: u64,
    cursor: u64,
    fraction: f64,
}

impl StreamingResampler {
    pub fn new(maximum_step: f64, block_capacity: usize) -> Option<Self> {
        if !(1..=65_536).contains(&block_capacity) {
            return None;
        }
        let kernel = SincKernel::new(maximum_step)?;
        let capacity = (2 * kernel.radius + block_capacity + 2).checked_next_power_of_two()?;
        let mut history = Vec::new();
        history.try_reserve_exact(capacity).ok()?;
        history.resize(capacity, StereoFrame::SILENCE);
        let radius = kernel.radius as u64;
        Some(Self {
            kernel,
            history: history.into_boxed_slice(),
            mask: capacity - 1,
            oldest: 0,
            written: radius,
            cursor: radius,
            fraction: 0.0,
        })
    }

    pub fn lookahead_frames(&self) -> usize {
        self.kernel.radius
    }

    pub fn writable_frames(&self) -> usize {
        self.history
            .len()
            .saturating_sub(self.written.saturating_sub(self.oldest) as usize)
    }
    /// Include this when measuring queued input; moving FIFO data into working
    /// history must not make clock control believe the samples were consumed.
    pub fn queued_input_frames(&self) -> usize {
        self.written.saturating_sub(self.cursor) as usize
    }

    /// Bounded reset after an input discontinuity. Never reuse old samples or
    /// coefficients from a different route; the immutable kernel remains valid.
    pub fn reset(&mut self) {
        self.history.fill(StereoFrame::SILENCE);
        let radius = self.kernel.radius as u64;
        self.oldest = 0;
        self.written = radius;
        self.cursor = radius;
        self.fraction = 0.0;
    }

    pub fn push(&mut self, input: &[StereoFrame]) -> usize {
        let count = input.len().min(self.writable_frames());
        let mut accepted = 0;
        for frame in input.iter().take(count) {
            let Some(next) = self.written.checked_add(1) else {
                break;
            };
            let Some(slot) = self.history.get_mut(self.written as usize & self.mask) else {
                break;
            };
            *slot = StereoFrame::new(frame.left, frame.right);
            self.written = next;
            accepted += 1;
        }
        accepted
    }

    /// Return the produced prefix. A shortage leaves the unwritten suffix alone;
    /// caller owns silence/fade/diagnostic policy and must not repeat old output.
    pub fn render(&mut self, output: &mut [StereoFrame], step: f64) -> usize {
        if !step.is_finite() || step <= 0.0 || step > self.kernel.maximum_step {
            return 0;
        }
        let radius = self.kernel.radius as u64;
        let mut produced = 0;
        for frame in output {
            let Some(end) = self.cursor.checked_add(radius) else {
                break;
            };
            if end >= self.written {
                break;
            }
            let Some(start) = self.cursor.checked_sub(radius) else {
                break;
            };
            let phase = self.fraction + step;
            let advance = phase.floor() as u64;
            let Some(next_cursor) = self.cursor.checked_add(advance) else {
                break;
            };
            *frame = self.kernel.interpolate(self.fraction, |index| {
                self.history
                    .get(start.wrapping_add(index as u64) as usize & self.mask)
                    .copied()
                    .unwrap_or(StereoFrame::SILENCE)
            });
            self.cursor = next_cursor;
            self.fraction = phase - advance as f64;
            self.oldest = self.cursor.saturating_sub(radius);
            produced += 1;
        }
        produced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A reproducible dense diagnostic; intentionally excluded from fast tests.
    /// Run in release mode and retain the printed extrema and grid definition.
    #[test]
    #[ignore = "dense response sweep; use release mode explicitly"]
    fn response_grid_meets_declared_passband_and_stopband_targets() {
        println!("maximum_step\tmax_gain_error_db\tmax_phase_component_error\tmax_stopband_db");
        for step in [0.501, 1.002, 2.004, 4.008, 16.032, 96.192] {
            let kernel = SincKernel::new(step).expect("fixture resampling step is finite and within the supported range");
            let mut gain_error = 0_f64;
            let mut phase_error = 0_f64;
            let mut stop_amplitude = 0_f64;
            for phase_index in 0..16 {
                let fraction = f64::from(phase_index) / 16.0;
                for frequency_index in 1..=32 {
                    let frequency =
                        f64::from(frequency_index) / 32.0 * (5.0 / 12.0) / step.max(1.0);
                    let omega = frequency * std::f64::consts::TAU;
                    let tone = kernel.interpolate(fraction, |index| {
                        let phase = omega * (index as f64 - kernel.radius as f64);
                        StereoFrame::new(phase.cos() as f32, phase.sin() as f32)
                    });
                    let amplitude = f64::from(tone.left).hypot(f64::from(tone.right));
                    gain_error = gain_error.max((20.0 * amplitude.log10()).abs());
                    phase_error = phase_error.max(
                        (f64::from(tone.left) - (omega * fraction).cos())
                            .abs()
                            .max((f64::from(tone.right) - (omega * fraction).sin()).abs()),
                    );
                }
                if step > 1.0 {
                    for frequency_index in 0..=32 {
                        let lower = 0.5 / step;
                        let frequency = lower + (0.5 - lower) * f64::from(frequency_index) / 32.0;
                        let tone = kernel.interpolate(fraction, |index| {
                            let phase = std::f64::consts::TAU
                                * frequency
                                * (index as f64 - kernel.radius as f64);
                            StereoFrame::new(phase.cos() as f32, phase.sin() as f32)
                        });
                        stop_amplitude =
                            stop_amplitude.max(f64::from(tone.left).hypot(f64::from(tone.right)));
                    }
                }
            }
            let stop_db = if step > 1.0 {
                20.0 * stop_amplitude.log10()
            } else {
                f64::NEG_INFINITY
            };
            println!("{step}\t{gain_error:.8}\t{phase_error:.8}\t{stop_db:.4}");
            assert!(gain_error <= 0.1, "step={step} gain={gain_error} dB");
            assert!(
                phase_error <= 0.001,
                "step={step} phase component error={phase_error}"
            );
            if step > 1.0 {
                assert!(stop_db <= -90.0, "step={step} stop={stop_db} dB");
            }
        }
    }
    #[test]
    fn phase_table_preserves_dc_and_passband() {
        for step in [0.501, 1.002, 2.004, 4.008, 96.192] {
            let kernel = SincKernel::new(step).expect("fixture resampling step is finite and within the supported range");
            for fraction in [0.0, 0.125, 0.371, 0.999] {
                let dc = kernel.interpolate(fraction, |_| StereoFrame::new(0.3, -0.7));
                assert!((dc.left - 0.3).abs() < 1e-7 && (dc.right + 0.7).abs() < 1e-7);
                for frequency in [0.01, 0.1, 0.4, 5.0 / 12.0].map(|f| f / step.max(1.0)) {
                    let omega = frequency * std::f64::consts::TAU;
                    let tone = kernel.interpolate(fraction, |index| {
                        let phase = omega * (index as f64 - kernel.radius as f64);
                        StereoFrame::new(phase.cos() as f32, phase.sin() as f32)
                    });
                    let amplitude = f64::from(tone.left).hypot(f64::from(tone.right));
                    assert!(
                        (20.0 * amplitude.log10()).abs() < 0.1,
                        "step={step} f={frequency} amplitude={amplitude}"
                    );
                    assert!((f64::from(tone.left) - (omega * fraction).cos()).abs() < 0.001);
                    assert!((f64::from(tone.right) - (omega * fraction).sin()).abs() < 0.001);
                }
            }
        }
    }

    #[test]
    fn downsampling_suppresses_out_of_band_tones() {
        for step in [1.002, 2.004, 4.008, 96.192] {
            let kernel = SincKernel::new(step).expect("fixture resampling step is finite and within the supported range");
            for frequency in [0.5, 0.55, 0.7]
                .map(|f| f / step)
                .into_iter()
                .filter(|f| *f < 0.5)
            {
                for fraction in [0.125, 0.371, 0.999] {
                    let tone = kernel.interpolate(fraction, |index| {
                        let phase = std::f64::consts::TAU
                            * frequency
                            * (index as f64 - kernel.radius as f64);
                        StereoFrame::new(phase.cos() as f32, phase.sin() as f32)
                    });
                    let amplitude = f64::from(tone.left).hypot(f64::from(tone.right));
                    assert!(
                        amplitude < 10_f64.powf(-90.0 / 20.0),
                        "step={step} f={frequency} amplitude={amplitude}"
                    );
                }
            }
        }
    }

    #[test]
    fn stream_history_wraps_without_losing_phase_or_repeating_shortages() {
        let mut resampler = StreamingResampler::new(1.002, 256).expect("fixture resampling step is supported and scratch capacity is nonzero");
        let mut incoming = [StereoFrame::SILENCE; 127];
        let mut outgoing = [StereoFrame::new(9.0, 9.0); 113];
        let mut written = 0;
        let mut position = 0.0;
        let mut compared = 0;
        for _ in 0..100 {
            for (index, frame) in incoming.iter_mut().enumerate() {
                let phase = (written + index) as f64 * 0.02 * std::f64::consts::TAU;
                *frame = StereoFrame::new(phase.sin() as f32, phase.cos() as f32);
            }
            let count = resampler.push(&incoming);
            written += count;
            loop {
                outgoing.fill(StereoFrame::new(9.0, 9.0));
                let count = resampler.render(&mut outgoing, 1.001);
                for frame in outgoing.iter().take(count) {
                    if position > 2.0 * resampler.lookahead_frames() as f64 {
                        let phase = position * 0.02 * std::f64::consts::TAU;
                        assert!((f64::from(frame.left) - phase.sin()).abs() < 0.00001);
                        assert!((f64::from(frame.right) - phase.cos()).abs() < 0.00001);
                        compared += 1;
                    }
                    position += 1.001;
                }
                assert!(
                    outgoing
                        .iter()
                        .skip(count)
                        .all(|f| *f == StereoFrame::new(9.0, 9.0))
                );
                if count < outgoing.len() {
                    break;
                }
            }
        }
        assert!(compared > 10_000);
        assert_eq!(resampler.render(&mut outgoing, f64::NAN), 0);
        assert_eq!(resampler.render(&mut outgoing, 1.003), 0);
    }
}
