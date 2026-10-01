//! Read-only inventory: no microphone capture or default-device changes.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--ready") {
        let identity = channelbridge_macos::diagnostics::read_target_driver_identity()?;
        let counters = channelbridge_macos::diagnostics::read_target_driver()?;
        if identity != env!("CARGO_PKG_VERSION") {
            return Err(format!(
                "loaded driver package {identity} differs from probe package {}",
                env!("CARGO_PKG_VERSION")
            )
            .into());
        }
        if counters.generation == 0 {
            return Err("loaded driver has no initialized timeline generation".into());
        }
        println!(
            "Loaded driver package {identity}; diagnostic protocol 1; generation {}. Read-only readiness passed; audio quality and exact source revision are not verified.",
            counters.generation
        );
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--diagnostics") {
        println!(
            "{:?}",
            channelbridge_macos::diagnostics::read_target_driver()?
        );
        return Ok(());
    }
    let show_formats = std::env::args().nth(1).as_deref() == Some("--formats");
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
            Ok(device) => {
                if show_formats && device.input_channels > 0 {
                    for stream in channelbridge_macos::devices::streams(
                        device.id,
                        channelbridge_hal_abi::fourcc(b"inpt"),
                    )? {
                        println!(
                            "{} input stream {}: virtual {:?}, physical {:?}",
                            device.name, stream.id, stream.virtual_format, stream.physical_format
                        );
                    }
                }
                println!(
                    "{}: {} | {} | {} Hz | in {} / out {}",
                    device.id,
                    device.name,
                    device.uid,
                    device.rate,
                    device.input_channels,
                    device.output_channels
                );
            }
            Err(error) => eprintln!("Device could not be inspected: {error}"),
        }
    }
    Ok(())
}
