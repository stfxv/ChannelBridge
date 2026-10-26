//! Settings are committed before changing a live audio route.
use crate::Config;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn load(path: &Path) -> io::Result<Config> {
    Ok(load_existing(path)?.unwrap_or_default())
}

/// Absence permits migration; malformed or unreadable settings never do.
pub fn load_existing(path: &Path) -> io::Result<Option<Config>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn open_temporary(path: PathBuf) -> io::Result<(File, TemporaryFile)> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    // Own cleanup only after create_new succeeds. A colliding file is not ours.
    Ok((file, TemporaryFile(path)))
}

pub fn save(path: &Path, config: &Config) -> io::Result<()> {
    config
        .validate_selection()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, format!("{error:?}")))?;
    let data = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary_path = parent.join(format!(
        ".channelbridge-settings-{}-{sequence}.tmp",
        std::process::id()
    ));
    let (mut file, temporary) = open_temporary(temporary_path)?;
    file.write_all(&data)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    // Rename is the commit point. No subsequent error may claim the previous
    // config remains in place after it has already been replaced.
    fs::rename(&temporary.0, path)?;
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "channelbridge-test-{}-{}",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("fixture owns a writable unique temporary directory");
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn invalid_save_does_not_replace_existing_config() {
        let dir = TestDirectory::new();
        let path = dir.0.join("settings.json");
        let original = Config {
            device_uid: "RME".into(),
            left: 9,
            right: 16,
            mode: Mode::Stereo,
            muted: false,
            auto_send_on_resume: false,
        };
        save(&path, &original).expect("original settings contain a device and nonzero channels in a writable fixture directory");
        let invalid = Config {
            left: 0,
            ..original.clone()
        };
        assert!(save(&path, &invalid).is_err());
        assert_eq!(load(&path).expect("failed validation preserves the readable previously saved settings"), original);
    }

    #[test]
    fn failed_commit_cleans_up_the_temporary_file() {
        let dir = TestDirectory::new();
        let path = dir.0.join("settings.json");
        fs::create_dir(&path).expect("fixture owns a writable unique temporary directory");
        let config = Config {
            device_uid: "RME".into(),
            ..Config::default()
        };
        assert!(save(&path, &config).is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(&dir.0).expect("fixture directory remains present for cleanup inspection").count(), 1);
    }

    #[test]
    fn corrupted_config_is_not_silently_reset() {
        let dir = TestDirectory::new();
        let path = dir.0.join("settings.json");
        assert_eq!(load(&path).expect("a missing settings file loads the default configuration"), Config::default());
        fs::write(&path, b"broken json").expect("fixture owns this writable settings path");
        assert_eq!(load(&path).expect_err("the fixture deliberately wrote malformed JSON").kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_colliding_temporary_file_is_never_removed() {
        let dir = TestDirectory::new();
        let path = dir.0.join("existing.tmp");
        fs::write(&path, b"owned by another writer").expect("fixture owns this writable settings path");
        assert!(open_temporary(path.clone()).is_err());
        assert_eq!(fs::read(&path).expect("fixture file was written and remains present for inspection"), b"owned by another writer");
    }
}
