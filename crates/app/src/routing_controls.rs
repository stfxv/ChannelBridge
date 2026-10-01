//! Main-thread routing controls preserve saved channels across hot-plugging.
use channelbridge_core::{Config, Mode};
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSPopUpButton, NSTextField, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;

pub struct RoutingControls {
    pub left: Retained<NSPopUpButton>,
    pub right: Retained<NSPopUpButton>,
    pub mode: Retained<NSPopUpButton>,
    displayed: RefCell<Option<(Config, u32)>>,
}
impl RoutingControls {
    pub fn new(main: MainThreadMarker, view: &NSView) -> Self {
        let popup = |x, y, width| {
            NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(main),
                NSRect::new(NSPoint::new(x, y), NSSize::new(width, 32.0)),
                false,
            )
        };
        let left = popup(20.0, 135.0, 160.0);
        let right = popup(200.0, 135.0, 160.0);
        let mode = popup(20.0, 195.0, 340.0);
        for title in ["モノラル", "ステレオ", "ミックス（平均）"] {
            mode.addItemWithTitle(&NSString::from_str(title));
        }
        for (label, x) in [("入力 / 左", 20.0), ("右", 200.0)] {
            let text = NSTextField::labelWithString(&NSString::from_str(label), main);
            text.setFrame(NSRect::new(
                NSPoint::new(x, 170.0),
                NSSize::new(160.0, 20.0),
            ));
            view.addSubview(&text);
        }
        view.addSubview(&left);
        view.addSubview(&right);
        view.addSubview(&mode);
        Self {
            left,
            right,
            mode,
            displayed: RefCell::new(None),
        }
    }
    pub fn update(&self, config: &Config, channels: u32, enabled: bool, menu_open: bool) {
        self.left.setEnabled(enabled && channels > 0);
        self.right
            .setEnabled(enabled && channels > 0 && config.mode != Mode::Mono);
        self.mode
            .setEnabled(enabled && !config.device_uid.is_empty());
        let changed = self
            .displayed
            .borrow()
            .as_ref()
            .is_none_or(|(old, count)| old != config || *count != channels);
        if menu_open || !changed {
            return;
        }
        for (popup, selected) in [(&self.left, config.left), (&self.right, config.right)] {
            popup.removeAllItems();
            let placeholder = if selected > channels {
                format!("Ch {selected}（未接続）")
            } else {
                "入力チャンネル".into()
            };
            popup.addItemWithTitle(&NSString::from_str(&placeholder));
            // CoreAudio adapters support at most 512 channels; keep UI bounded.
            for channel in 1..=channels.min(512) {
                popup.addItemWithTitle(&NSString::from_str(&format!("Ch {channel}")));
            }
            popup.selectItemAtIndex(if selected > 0 && selected <= channels.min(512) {
                selected as isize
            } else {
                0
            });
        }
        self.mode.selectItemAtIndex(match config.mode {
            Mode::Mono => 0,
            Mode::Stereo => 1,
            Mode::Mix => 2,
        });
        *self.displayed.borrow_mut() = Some((config.clone(), channels));
    }
    /// Interpret indices using the exact list that was displayed, not a newly
    /// enumerated device. Unavailable placeholder entries never become Ch 1.
    pub fn selected_config(&self, sender: &NSPopUpButton, config: &Config) -> Option<Config> {
        let mut next = config.clone();
        if std::ptr::eq(sender, &*self.mode) {
            next.mode = match sender.indexOfSelectedItem() {
                0 => Mode::Mono,
                1 => Mode::Stereo,
                2 => Mode::Mix,
                _ => return None,
            };
        } else {
            let channel = u32::try_from(sender.indexOfSelectedItem())
                .ok()
                .filter(|channel| *channel > 0)?;
            let displayed = self.displayed.borrow();
            let (shown, count) = displayed.as_ref()?;
            if shown.device_uid != config.device_uid || channel > (*count).min(512) {
                return None;
            }
            if std::ptr::eq(sender, &*self.left) {
                next.left = channel;
            } else if std::ptr::eq(sender, &*self.right) && config.mode != Mode::Mono {
                next.right = channel;
            } else {
                return None;
            }
        }
        Some(next)
    }
    pub fn restore(&self, config: &Config) {
        // Force committed selection to be restored after a user action, even
        // when the snapshot has not changed yet.
        let channels = self
            .displayed
            .borrow()
            .as_ref()
            .map_or(0, |(_, count)| *count);
        *self.displayed.borrow_mut() = None;
        self.update(config, channels, false, false);
    }
    pub fn smoke(&self) {
        let saved = Config {
            device_uid: "fixture".into(),
            left: 9,
            right: 16,
            mode: Mode::Stereo,
            muted: false,
        };
        self.update(&saved, 2, true, false);
        assert_eq!(self.left.indexOfSelectedItem(), 0);
        assert_eq!(self.right.indexOfSelectedItem(), 0);
        assert!(self.selected_config(&self.left, &saved).is_none());
        self.update(&saved, 16, true, false);
        assert_eq!(self.left.indexOfSelectedItem(), 9);
        assert_eq!(self.right.indexOfSelectedItem(), 16);
        self.left.selectItemAtIndex(3);
        let selected = self
            .selected_config(&self.left, &saved)
            .expect("left choice");
        assert_eq!((selected.left, selected.right), (3, 16));
        self.restore(&saved);
        assert_eq!(self.left.indexOfSelectedItem(), 9);
        let mono = Config {
            mode: Mode::Mono,
            ..saved.clone()
        };
        self.update(&mono, 2, true, false);
        assert!(!self.right.isEnabled());
        assert_eq!(mono.right, 16);
        self.mode.selectItemAtIndex(2);
        let mixed = self.selected_config(&self.mode, &mono).expect("mix choice");
        assert_eq!(mixed.mode, Mode::Mix);
        assert_eq!(mixed.right, 16);
        self.update(&saved, 16, true, false);
        self.update(&saved, 2, true, true);
        assert_eq!(self.left.indexOfSelectedItem(), 9);
        self.update(&saved, 2, true, false);
        assert_eq!(self.left.indexOfSelectedItem(), 0);
        assert_eq!(saved.left, 9);
        assert_eq!(saved.right, 16);
    }
}
