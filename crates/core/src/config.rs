use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Mono,
    Stereo,
    Mix,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub device_uid: String,
    pub left: u32,
    pub right: u32,
    pub mode: Mode,
    pub muted: bool,
    pub auto_send_on_resume: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_uid: String::new(),
            left: 1,
            right: 2,
            mode: Mode::Mono,
            muted: false,
            auto_send_on_resume: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionError {
    NoDevice,
    InvalidChannel,
    UnavailableChannel,
}

impl Config {
    pub fn validate_selection(&self) -> Result<(), SelectionError> {
        if self.device_uid.is_empty() {
            return Err(SelectionError::NoDevice);
        }
        if self.left == 0 || self.right == 0 {
            return Err(SelectionError::InvalidChannel);
        }
        Ok(())
    }

    pub fn validate_available(&self, channels: u32) -> Result<(), SelectionError> {
        self.validate_selection()?;
        if self.left > channels || (self.mode != Mode::Mono && self.right > channels) {
            return Err(SelectionError::UnavailableChannel);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_json_migrates_without_changing_selected_channels() {
        let json =
            r#"{"device_uid":"RME","left":9,"right":16,"mode":"stereo","muted":false,"gain_db":3}"#;
        let config: Config =
            serde_json::from_str(json).expect("fixture JSON conforms to the Config schema");
        assert!(!config.auto_send_on_resume);
        assert_eq!(config.left, 9);
        assert_eq!(config.right, 16);
        assert_eq!(
            config.validate_available(8),
            Err(SelectionError::UnavailableChannel)
        );
        let restored: Config = serde_json::from_str(
            &serde_json::to_string(&config).expect("Config fields have a JSON representation"),
        )
        .expect("JSON serialized from Config conforms to the same schema");
        assert_eq!(restored, config);
    }

    #[test]
    fn automatic_send_is_opt_in_and_round_trips_as_a_boolean() {
        assert!(!Config::default().auto_send_on_resume);
        let config = Config {
            auto_send_on_resume: true,
            ..Config::default()
        };
        assert_eq!(
            serde_json::from_str::<Config>(
                &serde_json::to_string(&config).expect("Config fields have a JSON representation")
            )
            .expect("JSON serialized from Config conforms to the same schema"),
            config
        );
        assert!(serde_json::from_str::<Config>(r#"{"auto_send_on_resume":"true"}"#).is_err());
    }

    #[test]
    fn mono_does_not_require_saved_right_channel_to_be_present() {
        let config = Config {
            device_uid: "RME".into(),
            left: 1,
            right: 16,
            ..Config::default()
        };
        assert_eq!(config.validate_available(2), Ok(()));
        assert_eq!(config.right, 16);
    }

    #[test]
    fn malformed_policy_is_rejected() {
        assert!(serde_json::from_str::<Config>(r#"{"mode":"other"}"#).is_err());
        assert!(serde_json::from_str::<Config>(r#"{"left":-1}"#).is_err());
        assert_eq!(
            Config::default().validate_selection(),
            Err(SelectionError::NoDevice)
        );
    }
}
