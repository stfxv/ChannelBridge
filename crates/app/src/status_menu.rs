//! Menu-bar item owns its menu; targets remain weak and main-thread only.
use channelbridge_core::{CaptureState, worker::Snapshot};
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject, sel};
use objc2_app_kit::{NSApplication, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem};
use objc2_foundation::{NSString, ns_string};

pub struct StatusMenu {
    item: Retained<NSStatusItem>,
    state: Retained<NSMenuItem>,
    send: Retained<NSMenuItem>,
    mute: Retained<NSMenuItem>,
    idle_image: Option<Retained<NSImage>>,
    send_image: Option<Retained<NSImage>>,
    mute_image: Option<Retained<NSImage>>,
}
impl StatusMenu {
    pub fn new(main: MainThreadMarker, target: &AnyObject) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(24.0);
        let menu = NSMenu::new(main);
        menu.setAutoenablesItems(false);
        // SAFETY: targets implement the matching one-argument selectors and
        // outlive this menu. NSApplication owns its terminate: implementation.
        let (state, send, mute) = unsafe {
            let state = menu.addItemWithTitle_action_keyEquivalent(
                ns_string!("入力確認待ち"),
                None,
                ns_string!(""),
            );
            state.setEnabled(false);
            let open = menu.addItemWithTitle_action_keyEquivalent(
                ns_string!("ChannelBridge Rust Testを開く"),
                Some(sel!(showWindow:)),
                ns_string!(""),
            );
            open.setTarget(Some(target));
            let send = menu.addItemWithTitle_action_keyEquivalent(
                ns_string!("送出開始"),
                Some(sel!(toggleSend:)),
                ns_string!(""),
            );
            send.setTarget(Some(target));
            let mute = menu.addItemWithTitle_action_keyEquivalent(
                ns_string!("ミュート"),
                Some(sel!(toggleMute:)),
                ns_string!(""),
            );
            mute.setTarget(Some(target));
            let quit = menu.addItemWithTitle_action_keyEquivalent(
                ns_string!("終了"),
                Some(sel!(terminate:)),
                ns_string!(""),
            );
            quit.setTarget(Some(&NSApplication::sharedApplication(main)));
            (state, send, mute)
        };
        item.setMenu(Some(&menu));
        let image = |name| {
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(name),
                Some(ns_string!("ChannelBridge Rust Test")),
            );
            if let Some(image) = image.as_ref() {
                image.setTemplate(true);
            }
            image
        };
        Self {
            item,
            state,
            send,
            mute,
            idle_image: image("mic"),
            send_image: image("mic.fill"),
            mute_image: image("mic.slash"),
        }
    }
    pub fn update(
        &self,
        main: MainThreadMarker,
        snapshot: &Snapshot,
        can_start: bool,
        can_mute: bool,
    ) {
        self.state
            .setTitle(&NSString::from_str(match snapshot.state {
                CaptureState::Idle => "入力確認待ち",
                CaptureState::Connecting => "接続中",
                CaptureState::Previewing => "入力確認中 · 送出停止",
                CaptureState::Sending => "送出中",
                CaptureState::Recovering => "再接続待ち",
                CaptureState::Blocked => "入力確認を開始できません",
            }));
        self.send.setTitle(if snapshot.wanted_send {
            ns_string!("停止")
        } else {
            ns_string!("送出開始")
        });
        self.send
            .setEnabled(!snapshot.finished && (snapshot.wanted_send || can_start));
        self.mute.setTitle(if snapshot.config.muted {
            ns_string!("ミュート解除")
        } else {
            ns_string!("ミュート")
        });
        self.mute.setEnabled(can_mute && !snapshot.finished);
        if let Some(button) = self.item.button(main) {
            let image = if snapshot.config.muted {
                &self.mute_image
            } else if snapshot.state == CaptureState::Sending {
                &self.send_image
            } else {
                &self.idle_image
            };
            button.setImage(image.as_deref());
            button.setTitle(if image.is_none() {
                ns_string!("CB")
            } else {
                ns_string!("")
            });
            button.setToolTip(Some(ns_string!("ChannelBridge Rust Test")));
        }
    }
}
impl Drop for StatusMenu {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
