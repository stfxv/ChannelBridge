//! Read-only control-path diagnostic access. Never opens a device or clears totals.
use crate::devices;
use channelbridge_contract::{DIAGNOSTICS_SELECTOR, DriverCounters};

pub fn read_target_driver() -> Result<DriverCounters, Box<dyn std::error::Error>> {
    Ok(DriverCounters::decode(&read_property(
        DIAGNOSTICS_SELECTOR,
    )?)?)
}
pub fn read_target_driver_identity() -> Result<String, Box<dyn std::error::Error>> {
    decode_identity(&read_property(
        channelbridge_contract::DRIVER_IDENTITY_SELECTOR,
    )?)
}
fn decode_identity(bytes: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    if bytes.is_empty()
        || bytes.len() > channelbridge_contract::DRIVER_IDENTITY_MAX_BYTES
        || !bytes.iter().all(u8::is_ascii_graphic)
    {
        return Err("invalid live driver package identity".into());
    }
    Ok(std::str::from_utf8(bytes)?.to_owned())
}
fn read_property(selector: u32) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let inventory = devices::list()?;
    let mut selected = inventory
        .iter()
        .filter_map(|entry| entry.as_ref().ok())
        .filter(|device| device.uid == channelbridge_contract::DEPLOYMENT.device_uid);
    let device = selected
        .next()
        .ok_or("virtual microphone driver is not loaded")?;
    if selected.next().is_some() {
        return Err("duplicate virtual microphone UID".into());
    }
    if selector == DIAGNOSTICS_SELECTOR {
        Ok(devices::data(device.id, selector, 48)?)
    } else {
        Ok(devices::text(device.id, selector)?.into_bytes())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_bounded_non_nul_ascii_and_preserves_prerelease() {
        assert_eq!(
            decode_identity(b"0.5.0-alpha.1+build.2").unwrap(),
            "0.5.0-alpha.1+build.2"
        );
        for bytes in [b"".as_slice(), b"1.0\0", b"1.0\n", &[255], &[b'a'; 65]] {
            assert!(decode_identity(bytes).is_err());
        }
    }
}
