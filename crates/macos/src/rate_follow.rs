//! Nonblocking control-thread rate negotiation for the separate test device.
use crate::devices::{self, Error};
use channelbridge_hal_abi::{ObjectId, PropertyAddress, fourcc};
use std::ffi::c_void;
use std::time::Duration;
use std::time::Instant;
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectSetPropertyData(
        object: ObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
}
#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Ready,
}
pub struct RateFollow {
    device: ObjectId,
    desired: u32,
    deadline: Instant,
}
fn nominal_rate(device: ObjectId) -> Result<f64, Error> {
    let bytes: [u8; 8] = devices::property(device, fourcc(b"nsrt"), fourcc(b"glob"))?
        .try_into()
        .map_err(|_| Error::MalformedProperty)?;
    let rate = f64::from_ne_bytes(bytes);
    if !rate.is_finite() || rate <= 0.0 {
        return Err(Error::MalformedProperty);
    }
    Ok(rate)
}
impl RateFollow {
    #[must_use]
    pub fn desired_rate(&self) -> u32 {
        self.desired
    }
    pub fn request_for_source(source_uid: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::request_target(Self::source_rate(source_uid)?)
    }
    /// Read the physical source's nominal rate without changing either device.
    pub fn source_rate(source_uid: &str) -> Result<u32, Box<dyn std::error::Error>> {
        let devices = devices::list()?;
        let mut matching = devices
            .iter()
            .filter_map(|entry| entry.as_ref().ok())
            .filter(|device| device.uid == source_uid);
        let source = matching.next().ok_or("selected source is unavailable")?;
        if matching.next().is_some() {
            return Err("duplicate source UID".into());
        }
        let rate = nominal_rate(source.id)?;
        if rate.fract() != 0.0 || !(8000.0..=768000.0).contains(&rate) {
            return Err("unsupported source rate".into());
        }
        Ok(rate as u32)
    }

    /// Call before starting either output IOProc. Changing a physical source's
    /// rate is deliberately outside this API. Poll from the control worker.
    pub fn request_test(desired: u32) -> Result<Self, Box<dyn std::error::Error>> {
        Self::request_uid(desired, channelbridge_contract::TEST_DEVICE_UID)
    }
    pub fn request_target(desired: u32) -> Result<Self, Box<dyn std::error::Error>> {
        Self::request_uid(desired, channelbridge_contract::DEPLOYMENT.device_uid)
    }
    fn request_uid(desired: u32, target_uid: &str) -> Result<Self, Box<dyn std::error::Error>> {
        if !channelbridge_contract::SUPPORTED_RATES.contains(&desired) {
            return Err("unsupported virtual rate".into());
        }
        let devices = devices::list()?;
        let mut matching = devices
            .iter()
            .filter_map(|entry| entry.as_ref().ok())
            .filter(|device| device.uid == target_uid);
        let device = matching
            .next()
            .ok_or("virtual microphone driver is not loaded")?;
        if matching.next().is_some() {
            return Err("duplicate virtual microphone UID".into());
        }
        if nominal_rate(device.id)? != f64::from(desired) {
            let address = PropertyAddress {
                selector: fourcc(b"nsrt"),
                scope: fourcc(b"glob"),
                element: 0,
            };
            let rate = f64::from(desired);
            // SAFETY: exact live Float64 payload and initialized SDK property address.
            let status = unsafe {
                AudioObjectSetPropertyData(
                    device.id,
                    &address,
                    0,
                    std::ptr::null(),
                    8,
                    (&rate as *const f64).cast(),
                )
            };
            if status != 0 {
                return Err(Error::Hal(status).into());
            }
        }
        Ok(Self {
            device: device.id,
            desired,
            deadline: Instant::now() + Duration::from_secs(3),
        })
    }
    pub fn poll(&self) -> Result<Status, Box<dyn std::error::Error>> {
        let nominal = nominal_rate(self.device)?;
        let input = devices::streams(self.device, fourcc(b"inpt"))?;
        let output = devices::streams(self.device, fourcc(b"outp"))?;
        if input.len() != 1 || output.len() != 1 {
            return Err("unexpected virtual stream layout".into());
        }
        let rates = input
            .iter()
            .chain(output.iter())
            .flat_map(|stream| {
                [
                    stream.virtual_format.sample_rate,
                    stream.physical_format.sample_rate,
                ]
            })
            .collect::<Vec<_>>();
        if confirmed(self.desired, nominal, &rates) {
            return Ok(Status::Ready);
        }
        if Instant::now() >= self.deadline {
            return Err("virtual rate change timed out; output must remain stopped".into());
        }
        Ok(Status::Pending)
    }
}
#[must_use]
fn confirmed(desired: u32, nominal: f64, stream_rates: &[f64]) -> bool {
    nominal == f64::from(desired)
        && stream_rates.len() >= 4
        && stream_rates.iter().all(|rate| *rate == f64::from(desired))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nominal_change_is_not_ready_until_both_stream_formats_follow() {
        assert!(!confirmed(96000, 96000.0, &[48000.0; 4]));
        assert!(!confirmed(96000, 48000.0, &[96000.0; 4]));
        assert!(!confirmed(
            96000,
            96000.0,
            &[96000.0, 96000.0, 96000.0, f64::NAN]
        ));
        assert!(!confirmed(96000, 96000.0, &[]));
        assert!(confirmed(96000, 96000.0, &[96000.0; 4]));
    }
}
