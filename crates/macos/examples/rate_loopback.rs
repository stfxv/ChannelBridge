//! Real HAL rate transitions, restricted to the separately installed test UID.
//! No physical input or default-device changes. Restore the initial test rate.
use channelbridge_contract::TEST_DEVICE_UID;
use channelbridge_macos::{
    devices, loopback,
    rate_follow::{RateFollow, Status},
};
use std::{error::Error, thread, time::Duration};

fn change(rate: u32) -> Result<(), Box<dyn Error>> {
    let pending = RateFollow::request_test(rate)?;
    while pending.poll()? == Status::Pending {
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let inventory = devices::list()?;
    let initial = inventory
        .iter()
        .filter_map(|entry| entry.as_ref().ok())
        .find(|device| device.uid == TEST_DEVICE_UID)
        .ok_or("Rust test driver is not loaded")?
        .rate;
    if !initial.is_finite()
        || initial.fract() != 0.0
        || !channelbridge_contract::SUPPORTED_RATES.contains(&(initial as u32))
    {
        return Err("cannot safely restore the initial test rate".into());
    }
    let result = (|| -> Result<(), Box<dyn Error>> {
        for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 48_000] {
            change(rate)?;
            let report = loopback::run(Duration::from_secs(3))?;
            println!("{rate} Hz: {report:?}");
            if !report.passed() {
                return Err(format!("loopback failed at {rate} Hz").into());
            }
        }
        Ok(())
    })();
    // Attempt restoration even after a failed negotiation or transfer.
    let restored = change(initial as u32);
    match (result, restored) {
        (Ok(()), Ok(())) => {
            println!("Restored {initial} Hz");
            Ok(())
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(format!("initial rate restoration failed: {error}").into()),
        (Err(error), Err(restore)) => {
            Err(format!("{error}; initial rate restoration failed: {restore}").into())
        }
    }
}
