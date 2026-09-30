//! Read-only inventory: no microphone capture or default-device changes.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--loopback") {
        let report = channelbridge_macos::loopback::run(std::time::Duration::from_secs(3))?;
        println!("{report:?}");
        if !report.passed() {
            return Err("synthetic loopback did not pass".into());
        }
        return Ok(());
    }
    for device in channelbridge_macos::devices::list()? {
        match device {
            Ok(device) => println!(
                "{}: {} | {} | {} Hz | in {} / out {}",
                device.id,
                device.name,
                device.uid,
                device.rate,
                device.input_channels,
                device.output_channels
            ),
            Err(error) => eprintln!("Device could not be inspected: {error}"),
        }
    }
    Ok(())
}
