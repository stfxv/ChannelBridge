/// A complete stereo frame can be published using one atomic u64 operation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StereoFrame {
    pub left: f32,
    pub right: f32,
}

impl StereoFrame {
    pub const SILENCE: Self = Self {
        left: 0.0,
        right: 0.0,
    };

    pub fn new(left: f32, right: f32) -> Self {
        Self {
            left: finite(left),
            right: finite(right),
        }
    }

    pub(crate) fn pack(self) -> u64 {
        u64::from(self.left.to_bits()) | (u64::from(self.right.to_bits()) << 32)
    }

    pub(crate) fn unpack(bits: u64) -> Self {
        Self {
            left: f32::from_bits(bits as u32),
            right: f32::from_bits((bits >> 32) as u32),
        }
    }
}

pub fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// One normalization policy for ring addressing AND availability checking.
/// Integer sample numbers must be exactly representable by the HAL's f64.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramePosition(u64);

impl FramePosition {
    pub const MAX_EXACT: u64 = (1_u64 << 53) - 1;

    pub fn from_sample_time(time: f64) -> Option<Self> {
        if !time.is_finite() || time < 0.0 || time > Self::MAX_EXACT as f64 {
            return None;
        }
        Some(Self(time.floor() as u64))
    }

    pub fn from_integer(time: u64) -> Option<Self> {
        (time <= Self::MAX_EXACT).then_some(Self(time))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_positions_use_the_same_integer_policy() {
        assert_eq!(
            FramePosition::from_sample_time(128.25),
            FramePosition::from_sample_time(128.5)
        );
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.1,
            (1_u64 << 53) as f64,
        ] {
            assert!(FramePosition::from_sample_time(invalid).is_none());
        }
    }

    #[test]
    fn stereo_pair_round_trips_without_loss() {
        let frame = StereoFrame::new(-0.5, 1.0 / 16_777_216.0);
        assert_eq!(StereoFrame::unpack(frame.pack()), frame);
        assert_eq!(
            StereoFrame::new(f32::NAN, f32::INFINITY),
            StereoFrame::SILENCE
        );
    }
}
