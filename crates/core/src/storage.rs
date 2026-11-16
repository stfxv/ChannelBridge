//! Settings are committed before changing a live audio route.
use crate::Config;
use crate::file_publication::Publication;
use std::fs;
use std::io;
use std::path::Path;

pub fn load(path: &Path) -> io::Result<Config> {
    Ok(load_existing(path)?.unwrap_or_default())
}

/// Absence permits migration; malformed or unreadable settings never do.
pub fn load_existing(path: &Path) -> io::Result<Option<Config>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::symlink_metadata(path) {
            Err(entry) if entry.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(entry) => Err(entry),
            Ok(_) => Err(error),
        },
        Err(error) => Err(error),
    }
}

pub fn save(path: &Path, config: &Config) -> io::Result<()> {
    config
        .validate_selection()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, format!("{error:?}")))?;
    let mut data = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
    data.push(b'\n');
    Publication::Settings.write(path, &data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
        super::save(&path, &original).expect("original settings contain a device and nonzero channels in a writable fixture directory");
        let invalid = Config {
            left: 0,
            ..original.clone()
        };
        assert!(super::save(&path, &invalid).is_err());
        assert_eq!(
            super::load(&path)
                .expect("failed validation preserves the readable previously saved settings"),
            original
        );
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
        assert!(super::save(&path, &config).is_err());
        assert!(path.is_dir());
        assert_eq!(
            fs::read_dir(&dir.0)
                .expect("fixture directory remains present for cleanup inspection")
                .count(),
            1
        );
    }

    #[test]
    fn corrupted_config_is_not_silently_reset() {
        let dir = TestDirectory::new();
        let path = dir.0.join("settings.json");
        assert_eq!(
            super::load(&path).expect("a missing settings file loads the default configuration"),
            Config::default()
        );
        fs::write(&path, b"broken json").expect("fixture owns this writable settings path");
        assert_eq!(
            super::load(&path)
                .expect_err("the fixture deliberately wrote malformed JSON")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
