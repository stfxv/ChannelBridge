//! Read-only native worker smoke test. No permission grant, source or audio start.
use channelbridge_core::Config;
use std::error::Error;
use std::path::Path;
#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn Error>> {
    use std::path::PathBuf;
    use std::time::Duration;
    use std::time::Instant;
    let worker = channelbridge_macos::native_backend::spawn_worker(
        PathBuf::from("work/read-only-settings.json"),
        Config::default(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = worker.snapshot();
        if let Some(error) = snapshot.device_error.as_ref().or(snapshot.error.as_ref()) {
            return Err(error.clone().into());
        }
        if !snapshot.devices.is_empty() {
            assert!(!snapshot.wanted_send);
            assert!(snapshot.audio.is_none());
            assert!(snapshot.driver_diagnostics.is_some());
            println!("Driver diagnostics: {:?}", snapshot.driver_diagnostics);
            if std::env::args().nth(1).as_deref() == Some("--save-diagnostics") {
                let path = std::env::args()
                    .nth(2)
                    .ok_or("diagnostic output path required")?;
                channelbridge_core::diagnostics::save_new(Path::new(&path), &snapshot)?;
            }
            for device in snapshot.devices.iter() {
                println!(
                    "{} | {} input channels | {} Hz | physical {:?}",
                    device.name, device.channels, device.rate, device.physical_inputs
                );
            }
            break;
        }
        if snapshot.finished || Instant::now() >= deadline {
            return Err("native inventory worker did not publish devices".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    worker.join().map_err(|_| "inventory worker panicked")?;
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("native inventory worker requires macOS");
}
