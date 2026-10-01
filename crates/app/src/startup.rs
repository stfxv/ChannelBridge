//! Test builds import production preferences read-only and save independently.
use channelbridge_core::{Config, storage};
use std::{
    io,
    path::{Path, PathBuf},
};

pub struct Settings {
    pub destination: PathBuf,
    pub config: Config,
    pub error: Option<String>,
}

impl Settings {
    pub fn for_home(home: &Path) -> Self {
        let support = home.join("Library/Application Support");
        Self::read(
            support
                .join(channelbridge_contract::DEPLOYMENT.settings_directory)
                .join("settings.json"),
            &support.join("ChannelBridge/settings.json"),
        )
    }

    fn read(destination: PathBuf, legacy: &Path) -> Self {
        let loaded = storage::load_existing(&destination)
            .and_then(|saved| match saved {
                Some(config) => Ok(config),
                None => storage::load(legacy),
            })
            .and_then(|config| {
                if config.left == 0 || config.right == 0 {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "入力チャンネルは1以上を指定してください",
                    ))
                } else {
                    Ok(config)
                }
            });
        let (config, error) = match loaded {
            Ok(config) => (config, None),
            Err(error) => (
                Config::default(),
                Some(format!("設定を読み込めません: {error}")),
            ),
        };
        Self {
            destination,
            config,
            error,
        }
    }

    pub fn smoke() -> Self {
        Self {
            destination: PathBuf::from("work/read-only-settings.json"),
            config: Config::default(),
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deployment_settings_location_preserves_existing_policy_without_rewriting() {
        let home = std::env::temp_dir().join(format!(
            "channelbridge-profile-settings-{}",
            std::process::id()
        ));
        let support = home.join("Library/Application Support/ChannelBridge");
        std::fs::create_dir_all(&support).unwrap();
        let path = support.join("settings.json");
        let bytes = br#"{"device_uid":"RME","left":9,"right":16,"mode":"stereo","muted":true}"#;
        std::fs::write(&path, bytes).unwrap();
        let settings = Settings::for_home(&home);
        assert_eq!(
            settings.destination,
            home.join("Library/Application Support")
                .join(channelbridge_contract::DEPLOYMENT.settings_directory)
                .join("settings.json")
        );
        assert_eq!((settings.config.left, settings.config.right), (9, 16));
        assert!(settings.error.is_none());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn migration_preserves_source_and_never_falls_back_from_corrupt_destination() {
        let directory =
            std::env::temp_dir().join(format!("channelbridge-migration-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let legacy = directory.join("legacy.json");
        let destination = directory.join("test.json");
        let original = br#"{"device_uid":"RME","left":9,"right":16,"mode":"stereo","muted":true}"#;
        std::fs::write(&legacy, original).unwrap();
        let imported = Settings::read(destination.clone(), &legacy);
        assert!(imported.error.is_none());
        assert_eq!(imported.config.left, 9);
        assert_eq!(imported.config.right, 16);
        assert!(imported.config.muted);
        assert!(!destination.exists());
        assert_eq!(std::fs::read(&legacy).unwrap(), original);
        std::fs::write(&destination, b"broken").unwrap();
        let failed = Settings::read(destination.clone(), &legacy);
        assert!(failed.error.is_some());
        assert!(failed.config.device_uid.is_empty());
        assert_eq!(std::fs::read(&destination).unwrap(), b"broken");
        let saved = Config {
            device_uid: "other".into(),
            ..Config::default()
        };
        storage::save(&destination, &saved).unwrap();
        assert_eq!(Settings::read(destination, &legacy).config, saved);
        assert_eq!(std::fs::read(&legacy).unwrap(), original);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
