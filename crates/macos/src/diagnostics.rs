//! Read-only control-path diagnostic access. Never opens a device or clears totals.
use crate::devices;
use channelbridge_contract::{DIAGNOSTICS_SELECTOR, DriverCounters, TEST_DEVICE_UID};
use channelbridge_hal_abi::fourcc;

pub fn read_test_driver() -> Result<DriverCounters, Box<dyn std::error::Error>> {
    let inventory = devices::list()?;
    let mut selected = inventory
        .iter()
        .filter_map(|entry| entry.as_ref().ok())
        .filter(|device| device.uid == TEST_DEVICE_UID);
    let device = selected.next().ok_or("Rust test driver is not loaded")?;
    if selected.next().is_some() {
        return Err("duplicate Rust test UID".into());
    }
    let bytes = devices::property(device.id, DIAGNOSTICS_SELECTOR, fourcc(b"glob"))?;
    Ok(DriverCounters::decode(&bytes)?)
}
