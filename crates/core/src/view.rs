//! Owned display data, with no HAL objects or callback pointers.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceChoice {
    pub uid: String,
    pub name: String,
    pub channels: u32,
    pub rate: f64,
    pub physical_inputs: Vec<InputFormat>,
    pub format_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct InputFormat {
    pub channels: u32,
    pub rate: f64,
    pub bits: u32,
    pub float: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct InputLevel {
    pub channel: u32,
    pub fresh: bool,
    pub current_dbfs: Option<f32>,
    pub held_dbfs: Option<f32>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AudioView {
    pub physical_inputs: Vec<InputFormat>,
    pub virtual_rate: u32,
    pub virtual_bits: u32,
    pub levels: Vec<InputLevel>,
}
pub fn dbfs(amplitude: f32) -> Option<f32> {
    (amplitude.is_finite() && amplitude > 0.0).then(|| 20.0 * amplitude.log10())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silence_is_distinct_from_zero_dbfs_and_invalid_levels_are_not_displayed() {
        assert_eq!(dbfs(0.0), None);
        assert_eq!(dbfs(1.0), Some(0.0));
        assert!((dbfs(0.5).unwrap() + 6.0206).abs() < 0.0001);
        assert_eq!(dbfs(f32::NAN), None);
        assert_eq!(dbfs(f32::INFINITY), None);
    }
}
