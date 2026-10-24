//! Explicit diagnostic export on a non-audio thread. No automatic recording.
use crate::worker::Snapshot;
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const MAX_REPORT_BYTES: usize = 64 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// Caller supplies versions from its actual executable and main bundle.
/// Missing identity is preserved rather than inferred from installed files.
#[derive(Default, Serialize)]
pub struct ReportMetadata {
    pub application_package_version: Option<String>,
    pub application_bundle_version: Option<String>,
    pub application_bundle_build: Option<String>,
}

#[derive(Serialize)]
struct Report<'a> {
    schema_version: u32,
    metadata: &'a ReportMetadata,
    state: crate::CaptureState,
    wanted_send: bool,
    finished: bool,
    left_channel: u32,
    right_channel: u32,
    mode: crate::Mode,
    muted: bool,
    retained_io_contexts: u64,
    session_counters: &'a Option<crate::view::AudioDiagnostics>,
    driver: &'a Option<crate::view::DriverDiagnostics>,
    /// Active-session formats and dry levels; null without a session.
    audio: &'a Option<crate::view::AudioView>,
    /// Most recent device inventory, distinct from active-session evidence.
    selected_input_inventory: Option<InputInventory<'a>>,
    control_history: &'a [crate::view::ControlEvent],
    error: &'a Option<String>,
    command_error: &'a Option<String>,
}
#[derive(Serialize)]
struct InputInventory<'a> {
    nominal_rate: f64,
    physical_inputs: &'a [crate::view::InputFormat],
    format_error: &'a Option<String>,
}
struct LimitedBuffer(Vec<u8>);
impl Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_REPORT_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("diagnostic report exceeds size limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Publishes a complete report with private permissions. Existing destinations
/// (including symlinks) are never replaced. The parent directory must exist.
/// Missing session data is JSON null, never fabricated zero counters.
pub fn save_new(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    save_new_with_metadata(path, snapshot, &ReportMetadata::default())
}
pub fn save_new_with_metadata(
    path: &Path,
    snapshot: &Snapshot,
    metadata: &ReportMetadata,
) -> io::Result<()> {
    let report = Report {
        schema_version: 1,
        metadata,
        state: snapshot.state,
        wanted_send: snapshot.wanted_send,
        finished: snapshot.finished,
        left_channel: snapshot.config.left,
        right_channel: snapshot.config.right,
        mode: snapshot.config.mode,
        muted: snapshot.config.muted,
        retained_io_contexts: snapshot.retained_io_contexts,
        session_counters: &snapshot.diagnostics,
        driver: &snapshot.driver_diagnostics,
        audio: &snapshot.audio,
        control_history: &snapshot.control_history,
        selected_input_inventory: snapshot
            .devices
            .iter()
            .find(|device| device.uid == snapshot.config.device_uid)
            .map(|device| InputInventory {
                nominal_rate: device.rate,
                physical_inputs: &device.physical_inputs,
                format_error: &device.format_error,
            }),
        error: &snapshot.error,
        command_error: &snapshot.command_error,
    };
    let mut buffer = LimitedBuffer(Vec::new());
    serde_json::to_writer_pretty(&mut buffer, &report).map_err(io::Error::other)?;
    buffer.write_all(b"\n")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary_path = parent.join(format!(
        ".channelbridge-diagnostic-{}-{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary_path)?;
    let temporary = Temporary(temporary_path);
    file.write_all(&buffer.0)?;
    file.sync_all()?;
    drop(file);
    // A same-directory hard link is the no-replacement commit point; unlike
    // rename it cannot silently overwrite a file created by another process.
    fs::hard_link(&temporary.0, path)?;
    drop(temporary);
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, controller::Backend, worker::Worker};
    struct Empty;
    impl Backend for Empty {
        type Pending = ();
        type Session = ();
        fn save(&mut self, _: &Config) -> Result<(), String> {
            Ok(())
        }
        fn negotiate(&mut self, _: &Config) -> Result<(), String> {
            Ok(())
        }
        fn ready(&mut self, _: &()) -> Result<bool, String> {
            Ok(true)
        }
        fn open(&mut self, _: &Config) -> Result<(), String> {
            Ok(())
        }
        fn healthy(&mut self, _: &(), _: &Config) -> Result<bool, String> {
            Ok(true)
        }
        fn route(&mut self, _: &(), _: &Config, _: bool, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn close(&mut self, _: ()) -> Result<(), String> {
            Ok(())
        }
    }
    #[test]
    fn export_is_bounded_private_and_preserves_existing_files_and_unknown_data() {
        let directory = std::env::temp_dir().join(format!(
            "channelbridge-diagnostic-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let worker = Worker::spawn(
            Config {
                device_uid: "private-device-serial".into(),
                ..Default::default()
            },
            || Ok(Empty),
        )
        .unwrap();
        let mut snapshot = worker.snapshot();
        worker.join().unwrap();
        snapshot.retained_io_contexts = 3;
        snapshot.driver_diagnostics = Some(crate::view::DriverDiagnostics {
            observed_worker_ms: 17,
            reading: crate::view::DriverReading::Unavailable {
                error: "unsupported property".into(),
            },
        });
        let physical = crate::view::InputFormat {
            channels: 10,
            rate: 48000.0,
            bits: 24,
            float: false,
        };
        snapshot.devices = vec![crate::view::DeviceChoice {
            uid: snapshot.config.device_uid.clone(),
            name: "private-device-name".into(),
            channels: 10,
            rate: 48000.0,
            physical_inputs: vec![physical.clone()],
            format_error: None,
        }]
        .into();
        snapshot.audio = Some(crate::view::AudioView {
            physical_inputs: vec![physical],
            virtual_rate: 48000,
            virtual_bits: 32,
            levels: vec![crate::view::InputLevel {
                channel: 1,
                fresh: true,
                current_dbfs: None,
                held_dbfs: None,
            }],
        });
        let destination = directory.join("report.json");
        save_new(&destination, &snapshot).unwrap();
        let bytes = fs::read(&destination).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["session_counters"].is_null());
        assert_eq!(json["retained_io_contexts"], 3);
        assert!(json["metadata"]["application_bundle_version"].is_null());
        assert_eq!(json["audio"]["physical_inputs"][0]["bits"], 24);
        assert_eq!(json["audio"]["virtual_bits"], 32);
        assert_eq!(
            json["selected_input_inventory"]["physical_inputs"][0]["float"],
            false
        );
        assert_eq!(json["audio"]["levels"][0]["fresh"], true);
        assert!(json["audio"]["levels"][0]["current_dbfs"].is_null());
        assert!(!String::from_utf8_lossy(&bytes).contains("private-device-name"));
        assert_eq!(json["driver"]["reading"]["status"], "unavailable");
        assert_eq!(json["driver"]["reading"]["error"], "unsupported property");
        assert!(json["driver"]["reading"].get("written_frames").is_none());
        assert!(!String::from_utf8_lossy(&bytes).contains("private-device-serial"));
        assert_eq!(
            save_new(&destination, &snapshot).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        snapshot.audio = None;
        snapshot.devices = Vec::new().into();
        let unavailable_path = directory.join("unavailable.json");
        save_new_with_metadata(
            &unavailable_path,
            &snapshot,
            &ReportMetadata {
                application_package_version: Some("0.5.0-alpha.1".into()),
                application_bundle_version: Some("0.5.1".into()),
                application_bundle_build: Some("2".into()),
            },
        )
        .unwrap();
        let unknown: serde_json::Value =
            serde_json::from_slice(&fs::read(unavailable_path).unwrap()).unwrap();
        assert!(unknown["audio"].is_null());
        assert!(unknown["selected_input_inventory"].is_null());
        assert_eq!(
            unknown["metadata"]["application_package_version"],
            "0.5.0-alpha.1"
        );
        assert_eq!(unknown["metadata"]["application_bundle_version"], "0.5.1");
        assert_eq!(unknown["metadata"]["application_bundle_build"], "2");
        snapshot.error = Some("x".repeat(MAX_REPORT_BYTES));
        assert!(save_new(&directory.join("oversize.json"), &snapshot).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }
}
