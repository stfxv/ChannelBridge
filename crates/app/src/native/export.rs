//! Save dialog on the main thread; bounded report writing on a separate thread.
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSModalResponseOK, NSSavePanel, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use std::cell::RefCell;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;

pub(super) struct Export {
    pub button: Retained<NSButton>,
    pub label: Retained<NSTextField>,
    completion: RefCell<Option<Receiver<Result<(), String>>>>,
}
impl Export {
    pub fn new(main: MainThreadMarker) -> Self {
        // SAFETY: target/action attached later to the retained app delegate.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!("診断を保存"), None, None, main)
        };
        button.setFrame(NSRect::new(
            NSPoint::new(20.0, 42.0),
            NSSize::new(120.0, 28.0),
        ));
        let label = NSTextField::labelWithString(ns_string!(""), main);
        label.setFrame(NSRect::new(
            NSPoint::new(150.0, 44.0),
            NSSize::new(210.0, 24.0),
        ));
        Self {
            button,
            label,
            completion: RefCell::new(None),
        }
    }
    pub fn begin(&self, main: MainThreadMarker, snapshot: channelbridge_core::worker::Snapshot) {
        if !self.button.isEnabled() {
            return;
        }
        self.button.setEnabled(false);
        let panel = NSSavePanel::savePanel(main);
        panel.setNameFieldStringValue(ns_string!("ChannelBridge-diagnostics.json"));
        panel.setMessage(Some(ns_string!(
            "音声を含まない診断情報を保存します。新しいファイル名を指定してください。"
        )));
        panel.setCanCreateDirectories(true);
        if panel.runModal() != NSModalResponseOK {
            self.button.setEnabled(true);
            return;
        }
        let path = panel
            .URL()
            .filter(|url| url.isFileURL())
            .and_then(|url| url.path())
            .map(|path| std::path::PathBuf::from(path.to_string()));
        let Some(path) = path else {
            self.label
                .setStringValue(ns_string!("保存先を取得できません"));
            self.button.setEnabled(true);
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let bundle = objc2_foundation::NSBundle::mainBundle();
        let version = |key: &NSString| {
            bundle.objectForInfoDictionaryKey(key).and_then(|value| {
                value
                    .downcast_ref::<NSString>()
                    .map(|text| text.to_string())
            })
        };
        let metadata = channelbridge_core::diagnostics::ReportMetadata {
            application_package_version: Some(env!("CARGO_PKG_VERSION").into()),
            application_bundle_version: version(ns_string!("CFBundleShortVersionString")),
            application_bundle_build: version(ns_string!("CFBundleVersion")),
        };
        match std::thread::Builder::new()
            .name("channelbridge-export".into())
            .spawn(move || {
                let result = channelbridge_core::diagnostics::save_new_with_metadata(
                    &path, &snapshot, &metadata,
                )
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        "同名のファイルがあります".into()
                    } else {
                        format!("診断の保存に失敗しました: {error}")
                    }
                });
                let _ = sender.send(result);
            }) {
            Ok(_) => {
                *self.completion.borrow_mut() = Some(receiver);
                self.label.setStringValue(ns_string!("保存しています…"));
            }
            Err(_) => {
                self.label
                    .setStringValue(ns_string!("保存処理を開始できません"));
                self.button.setEnabled(true);
            }
        }
    }
    pub fn poll(&self) {
        let mut completion = self.completion.borrow_mut();
        let Some(receiver) = completion.as_ref() else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("保存処理が終了しました".into()),
        };
        self.label
            .setStringValue(&NSString::from_str(match &result {
                Ok(()) => "保存しました",
                Err(error) => error,
            }));
        *completion = None;
        self.button.setEnabled(true);
    }
    #[must_use]
    pub fn pending(&self) -> bool {
        self.completion.borrow().is_some()
    }
}
