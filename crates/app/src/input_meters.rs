//! Read-only input meters: dry input levels do not depend on sending or mute.
use channelbridge_core::Config;
use channelbridge_core::Mode;
use channelbridge_core::view::AudioView;
use channelbridge_core::view::InputLevel;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSTextField, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

pub struct InputMeters {
    left: Retained<NSTextField>,
    right: Retained<NSTextField>,
    virtual_format: Retained<NSTextField>,
}
impl InputMeters {
    pub fn new(main: MainThreadMarker, view: &NSView) -> Self {
        let label = |y| {
            let label = NSTextField::labelWithString(&NSString::from_str("—"), main);
            label.setFrame(NSRect::new(NSPoint::new(20.0, y), NSSize::new(340.0, 20.0)));
            view.addSubview(&label);
            label
        };
        Self {
            left: label(105.0),
            right: label(80.0),
            virtual_format: label(55.0),
        }
    }
    pub fn update(&self, config: &Config, audio: Option<&AudioView>) {
        self.left
            .setStringValue(&NSString::from_str(&channel_text(config.left, audio)));
        self.right
            .setHidden(config.mode == Mode::Mono || config.left == config.right);
        self.right
            .setStringValue(&NSString::from_str(&channel_text(config.right, audio)));
        let format = audio.map_or_else(
            || "仮想マイク: 未接続".into(),
            |audio| {
                format!(
                    "仮想マイク: {} kHz · {}bit float",
                    audio.virtual_rate as f64 / 1000.0,
                    audio.virtual_bits
                )
            },
        );
        self.virtual_format
            .setStringValue(&NSString::from_str(&format));
    }
    pub fn smoke(&self) {
        let config = Config {
            left: 9,
            right: 16,
            mode: Mode::Stereo,
            muted: true,
            ..Config::default()
        };
        let mut audio = AudioView {
            physical_inputs: vec![],
            virtual_rate: 48000,
            virtual_bits: 32,
            levels: vec![InputLevel {
                channel: 9,
                fresh: true,
                current_dbfs: Some(-12.0),
                held_dbfs: Some(-6.0),
            }],
        };
        self.update(&config, Some(&audio));
        assert!(self.left.stringValue().to_string().contains("-12.0 dBFS"));
        assert!(self.right.stringValue().to_string().contains("未取得"));
        assert!(!self.right.isHidden());
        assert!(
            self.virtual_format
                .stringValue()
                .to_string()
                .contains("48 kHz · 32bit float")
        );
        audio.levels[0].fresh = false;
        self.update(
            &Config {
                mode: Mode::Mono,
                ..config
            },
            Some(&audio),
        );
        assert!(self.left.stringValue().to_string().contains("未取得"));
        assert!(self.right.isHidden());
    }
}
#[must_use]
fn decibels(value: Option<f32>) -> String {
    value
        .filter(|value| value.is_finite())
        .map_or_else(|| "−∞".into(), |value| format!("{value:.1}"))
}
#[must_use]
pub(crate) fn channel_text(channel: u32, audio: Option<&AudioView>) -> String {
    match audio.and_then(|audio| audio.levels.iter().find(|level| level.channel == channel)) {
        Some(InputLevel {
            fresh: true,
            current_dbfs,
            held_dbfs,
            ..
        }) => format!(
            "Ch {channel}: {} dBFS · ピーク {} dBFS",
            decibels(*current_dbfs),
            decibels(*held_dbfs)
        ),
        _ => format!("Ch {channel}: 入力レベル未取得"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_silent_stale_and_zero_dbfs_levels_are_distinct() {
        let mut audio = AudioView {
            physical_inputs: vec![],
            virtual_rate: 48000,
            virtual_bits: 32,
            levels: vec![InputLevel {
                channel: 9,
                fresh: true,
                current_dbfs: None,
                held_dbfs: Some(-6.0),
            }],
        };
        assert!(super::channel_text(9, Some(&audio)).contains("−∞ dBFS"));
        assert!(super::channel_text(9, Some(&audio)).contains("ピーク -6.0"));
        assert!(super::channel_text(16, Some(&audio)).contains("未取得"));
        assert!(super::channel_text(9, None).contains("未取得"));
        audio.levels[0].current_dbfs = Some(0.0);
        assert!(super::channel_text(9, Some(&audio)).contains("0.0 dBFS"));
        audio.levels[0].fresh = false;
        assert!(super::channel_text(9, Some(&audio)).contains("未取得"));
    }
}
