//! Main-thread screen state and transactional control bindings.
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSPopUpButton, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};

pub(super) struct Screen {
    pub(super) export: super::export::Export,
    overview: std::cell::RefCell<Option<crate::overview::Overview>>,
    overview_sent: std::cell::Cell<bool>,
    pub(super) overview_button: Retained<NSButton>,
    pub(super) permission: channelbridge_macos::permission::MicrophonePermission,
    observed_permission: std::cell::Cell<Option<channelbridge_macos::permission::Permission>>,
    pub(super) permission_button: Retained<NSButton>,
    pub(super) send_button: Retained<NSButton>,
    pub(super) mute_button: Retained<NSButton>,
    pub(super) auto_send_button: Retained<NSButton>,
    pub(super) meters: crate::input_meters::InputMeters,
    pub(super) routing: crate::routing_controls::RoutingControls,
    pub(super) window: Retained<NSWindow>,
    pub(super) devices: Retained<NSPopUpButton>,
    format: Retained<NSTextField>,
    status: Retained<NSTextField>,
    settings_error: Option<String>,
    pub(super) device_items: std::cell::RefCell<Vec<(String, String)>>,
    displayed_uid: std::cell::RefCell<String>,
    pub(super) pending_attempt: std::cell::Cell<Option<u64>>,
    pub(super) action_error: std::cell::RefCell<Option<String>>,
    pub(super) menu_open: std::cell::Cell<bool>,
}
impl Screen {
    pub(super) fn new(main: MainThreadMarker, settings_error: Option<String>) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(380.0, 472.0));
        // SAFETY: main-thread allocation, initialized geometry and valid style.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(main),
                frame,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(&NSString::from_str(
            channelbridge_contract::DEPLOYMENT.app_name,
        ));
        let devices = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(main),
            NSRect::new(NSPoint::new(20.0, 275.0), NSSize::new(340.0, 32.0)),
            false,
        );
        // Enabled after a valid settings snapshot arrives.
        devices.setEnabled(false);
        let format = NSTextField::labelWithString(ns_string!("—"), main);
        format.setFrame(NSRect::new(
            NSPoint::new(20.0, 240.0),
            NSSize::new(340.0, 24.0),
        ));
        let status = NSTextField::labelWithString(ns_string!("入力機器を確認しています"), main);
        status.setFrame(NSRect::new(
            NSPoint::new(20.0, 10.0),
            NSSize::new(340.0, 25.0),
        ));
        let view = window.contentView().expect("window content view");
        let meters = crate::input_meters::InputMeters::new(main, &view);
        let routing = crate::routing_controls::RoutingControls::new(main, &view);
        view.addSubview(&devices);
        view.addSubview(&format);
        view.addSubview(&status);
        let children = view.subviews();
        for index in 0..children.len() {
            let child = children.objectAtIndex(index);
            let mut frame = child.frame();
            frame.origin.y += 70.0;
            child.setFrame(frame);
        }
        status.setFrame(NSRect::new(
            NSPoint::new(20.0, 10.0),
            NSSize::new(340.0, 25.0),
        ));
        let button = |title, x| {
            // SAFETY: no target/action yet; main-thread construction only.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    None,
                    None,
                    main,
                )
            };
            button.setFrame(NSRect::new(NSPoint::new(x, 55.0), NSSize::new(105.0, 32.0)));
            view.addSubview(&button);
            button
        };
        let permission_button = button("マイクを許可", 20.0);
        let send_button = button("送出開始", 137.0);
        let mute_button = button("ミュート", 255.0);
        let overview_button = button("全入力のレベルを確認", 20.0);
        overview_button.setFrame(NSRect::new(
            NSPoint::new(20.0, 90.0),
            NSSize::new(340.0, 28.0),
        ));
        for index in 0..view.subviews().len() {
            let child = view.subviews().objectAtIndex(index);
            let mut frame = child.frame();
            frame.origin.y += 72.0;
            child.setFrame(frame);
        }
        status.setFrame(NSRect::new(
            NSPoint::new(20.0, 10.0),
            NSSize::new(340.0, 25.0),
        ));
        // SAFETY: the retained main-thread delegate installs target/action later.
        let auto_send_button = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("復帰時に自動送出"),
                None,
                None,
                main,
            )
        };
        auto_send_button.setFrame(NSRect::new(
            NSPoint::new(20.0, 86.0),
            NSSize::new(340.0, 24.0),
        ));
        auto_send_button.setToolTip(Some(ns_string!(
            "有効にすると、現在停止中でも次のシステムのスリープ復帰時や入力機器の再接続時に送出を開始します。ミュート設定は維持します。有効にした直後やアプリ起動時には開始しません。"
        )));
        auto_send_button.setEnabled(false);
        view.addSubview(&auto_send_button);
        let export = super::export::Export::new(main);
        view.addSubview(&export.button);
        view.addSubview(&export.label);
        Self {
            export,
            overview: Default::default(),
            overview_sent: Default::default(),
            overview_button,
            permission: Default::default(),
            observed_permission: Default::default(),
            permission_button,
            send_button,
            mute_button,
            auto_send_button,
            meters,
            routing,
            window,
            devices,
            format,
            status,
            settings_error,
            device_items: Default::default(),
            displayed_uid: Default::default(),
            pending_attempt: Default::default(),
            action_error: Default::default(),
            menu_open: Default::default(),
        }
    }
    pub(super) fn update(&self, snapshot: &channelbridge_core::worker::Snapshot) {
        self.export.poll();
        self.meters
            .update(&snapshot.config, snapshot.audio.as_ref());
        if self
            .pending_attempt
            .get()
            .is_some_and(|attempt| snapshot.configuration_attempt > attempt)
        {
            self.pending_attempt.set(None);
        }
        self.devices.setEnabled(
            self.settings_error.is_none()
                && self.pending_attempt.get().is_none()
                && !snapshot.finished,
        );
        let items: Vec<_> = snapshot
            .devices
            .iter()
            .map(|device| (device.uid.clone(), device.name.clone()))
            .collect();
        let changed = *self.device_items.borrow() != items
            || *self.displayed_uid.borrow() != snapshot.config.device_uid;
        let selected = snapshot
            .devices
            .iter()
            .position(|device| device.uid == snapshot.config.device_uid);
        let permission = channelbridge_macos::permission::status();
        let pending = self.permission.pending();
        use channelbridge_macos::permission::Permission;
        let permission_title = match permission {
            Permission::NotDetermined if pending => "許可待ち",
            Permission::NotDetermined => "マイクを許可",
            Permission::Authorized => "マイク許可済み",
            Permission::Denied => "マイク権限なし",
            Permission::Restricted => "マイク利用制限",
            Permission::Unavailable => "権限確認不可",
        };
        self.permission_button
            .setTitle(&NSString::from_str(permission_title));
        self.permission_button
            .setEnabled(permission == Permission::NotDetermined && !pending && !snapshot.finished);
        let valid = selected.is_some_and(|index| {
            snapshot
                .config
                .validate_available(snapshot.devices[index].channels)
                .is_ok()
        });
        self.send_button.setTitle(if snapshot.wanted_send {
            ns_string!("停止")
        } else {
            ns_string!("送出開始")
        });
        self.send_button.setEnabled(
            !snapshot.finished
                && (snapshot.wanted_send
                    || (valid && permission.allowed() && self.devices.isEnabled())),
        );
        self.mute_button.setTitle(if snapshot.config.muted {
            ns_string!("ミュート解除")
        } else {
            ns_string!("ミュート")
        });
        self.mute_button
            .setEnabled(self.devices.isEnabled() && !snapshot.config.device_uid.is_empty());
        self.auto_send_button
            .setState(isize::from(snapshot.config.auto_send_on_resume));
        self.auto_send_button
            .setEnabled(self.devices.isEnabled() && !snapshot.config.device_uid.is_empty());
        self.overview_button
            .setEnabled(selected.is_some() && !snapshot.finished);
        self.routing.update(
            &snapshot.config,
            selected.map_or(0, |index| snapshot.devices[index].channels),
            self.devices.isEnabled(),
            self.menu_open.get(),
        );
        let placeholder = if snapshot.config.device_uid.is_empty() {
            "入力機器を選択してください"
        } else {
            "保存した入力機器（未接続）"
        };
        if !self.menu_open.get() && (changed || self.devices.numberOfItems() == 0) {
            self.devices.removeAllItems();
            self.devices
                .addItemWithTitle(&NSString::from_str(placeholder));
            for device in snapshot.devices.iter() {
                self.devices
                    .addItemWithTitle(&NSString::from_str(&device.name));
            }
            self.devices
                .selectItemAtIndex(selected.map_or(0, |index| index as isize + 1));
            *self.device_items.borrow_mut() = items;
            *self.displayed_uid.borrow_mut() = snapshot.config.device_uid.clone();
        }
        let format = selected
            .map(|index| &snapshot.devices[index])
            .map(|device| {
                let bits = device
                    .physical_inputs
                    .iter()
                    .map(|format| {
                        format!(
                            "{}bit{}",
                            format.bits,
                            if format.float { " float" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" / ");
                format!(
                    "{} kHz · {} · {}入力",
                    device.rate / 1000.0,
                    if bits.is_empty() {
                        "形式を取得できません"
                    } else {
                        &bits
                    },
                    device.channels
                )
            })
            .unwrap_or_else(|| "—".into());
        self.format.setStringValue(&NSString::from_str(&format));
        let status = if let Some(error) = self.settings_error.as_deref() {
            error
        } else if self.pending_attempt.get().is_some() {
            "設定を保存しています"
        } else if let Some(error) = snapshot.command_error.as_deref() {
            error
        } else if let Some(error) = snapshot.error.as_deref() {
            error
        } else if snapshot.device_error.is_some() {
            "入力機器を確認できません"
        } else if snapshot.config.device_uid.is_empty() {
            "入力機器を選択してください"
        } else if selected.is_none() {
            "選択した入力機器が接続されていません"
        } else {
            match snapshot.state {
                channelbridge_core::CaptureState::Idle if !permission.allowed() => {
                    "マイク権限を確認してください"
                }
                channelbridge_core::CaptureState::Idle => "送出停止中",
                channelbridge_core::CaptureState::Connecting => "音声を接続しています",
                channelbridge_core::CaptureState::Previewing => "入力確認中 · 送出停止",
                channelbridge_core::CaptureState::Sending => "送出中",
                channelbridge_core::CaptureState::Recovering => "音声の再接続待ち",
                channelbridge_core::CaptureState::Suspended => "スリープ中 · 復帰を待っています",
                channelbridge_core::CaptureState::Blocked if !permission.allowed() => {
                    match permission {
                        Permission::NotDetermined => "入力確認にはマイクの許可が必要です",
                        Permission::Denied => "システム設定でマイクの許可を確認してください",
                        Permission::Restricted => "マイクの利用が制限されています",
                        _ => "マイク権限を確認できません",
                    }
                }
                channelbridge_core::CaptureState::Blocked => "音声を開始できません",
            }
        };
        self.status.setStringValue(&NSString::from_str(status));
        if let Some(error) = self.action_error.borrow().as_deref() {
            self.status.setStringValue(&NSString::from_str(error));
        }
    }
    pub(super) fn show_overview(&self) {
        let main = MainThreadMarker::new().expect("overview on main thread");
        let mut overview = self.overview.borrow_mut();
        let overview = overview.get_or_insert_with(|| crate::overview::Overview::new(main));
        overview.window.center();
        overview.window.makeKeyAndOrderFront(None);
    }
    pub(super) fn sync_overview(
        &self,
        worker: &channelbridge_core::worker::Worker,
        snapshot: &channelbridge_core::worker::Snapshot,
    ) {
        let overview = self.overview.borrow();
        let visible = overview
            .as_ref()
            .is_some_and(|overview| overview.window.isVisible());
        if !snapshot.finished && visible != self.overview_sent.get() {
            match worker.send(channelbridge_core::Command::Overview(visible)) {
                Ok(()) => self.overview_sent.set(visible),
                Err(error) => *self.action_error.borrow_mut() = Some(error),
            }
        }
        if let Some(overview) = overview.as_ref().filter(|_| visible) {
            overview.update(
                MainThreadMarker::new().expect("overview refresh on main thread"),
                snapshot,
            );
        }
    }
    pub(super) fn sync_permission(&self, worker: &channelbridge_core::worker::Worker) {
        let current = channelbridge_macos::permission::status();
        if self.observed_permission.get() != Some(current) {
            match worker.send(channelbridge_core::Command::PermissionChanged(
                current.allowed(),
            )) {
                Ok(()) => self.observed_permission.set(Some(current)),
                Err(error) => *self.action_error.borrow_mut() = Some(error),
            }
        }
    }
    pub(super) fn toggle_send(&self, worker: &channelbridge_core::worker::Worker) {
        let snapshot = worker.snapshot();
        let command = if snapshot.wanted_send {
            channelbridge_core::Command::Stop
        } else {
            if !self.send_button.isEnabled() {
                return;
            }
            channelbridge_core::Command::Start
        };
        if let Err(error) = worker.send(command) {
            *self.action_error.borrow_mut() = Some(error);
        }
    }
    pub(super) fn toggle_mute(&self, worker: &channelbridge_core::worker::Worker) {
        if self.settings_error.is_some() || self.pending_attempt.get().is_some() {
            return;
        }
        let snapshot = worker.snapshot();
        let mut config = snapshot.config.clone();
        config.muted = !config.muted;
        self.submit_config(worker, &snapshot, config);
    }
    pub(super) fn toggle_auto_send(&self, worker: &channelbridge_core::worker::Worker) {
        let snapshot = worker.snapshot();
        if !self.auto_send_button.isEnabled() {
            self.update(&snapshot);
            return;
        }
        let mut config = snapshot.config.clone();
        config.auto_send_on_resume = !config.auto_send_on_resume;
        self.submit_config(worker, &snapshot, config);
    }
    pub(super) fn select_device(&self, worker: &channelbridge_core::worker::Worker) {
        if self.settings_error.is_some() || self.pending_attempt.get().is_some() {
            return;
        }
        let snapshot = worker.snapshot();
        let selected = self.devices.indexOfSelectedItem();
        let committed_index = self
            .device_items
            .borrow()
            .iter()
            .position(|item| item.0 == snapshot.config.device_uid)
            .map_or(0, |index| index as isize + 1);
        let uid = selected
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| {
                self.device_items
                    .borrow()
                    .get(index)
                    .map(|item| item.0.clone())
            });
        // Map the displayed UID, never a fresh enumeration index. Hot-plugging
        // while the menu is open cannot silently select another device.
        let Some(uid) = uid else {
            self.devices.selectItemAtIndex(committed_index);
            self.update(&snapshot);
            return;
        };
        if uid == snapshot.config.device_uid {
            return;
        }
        let mut config = snapshot.config.clone();
        config.device_uid = uid;
        // Restore the committed selection until Configure has completed.
        self.devices.selectItemAtIndex(committed_index);
        self.submit_config(worker, &snapshot, config);
    }
    pub(super) fn select_routing(
        &self,
        sender: &NSPopUpButton,
        worker: &channelbridge_core::worker::Worker,
    ) {
        if self.settings_error.is_some() || self.pending_attempt.get().is_some() {
            return;
        }
        let snapshot = worker.snapshot();
        let config = self.routing.selected_config(sender, &snapshot.config);
        self.routing.restore(&snapshot.config);
        if let Some(config) = config.filter(|config| config != &snapshot.config) {
            self.submit_config(worker, &snapshot, config);
        } else {
            self.update(&snapshot);
        }
    }
    fn submit_config(
        &self,
        worker: &channelbridge_core::worker::Worker,
        snapshot: &channelbridge_core::worker::Snapshot,
        config: channelbridge_core::Config,
    ) {
        match worker.send(channelbridge_core::Command::Configure(config)) {
            Ok(()) => {
                self.pending_attempt
                    .set(Some(snapshot.configuration_attempt));
                *self.action_error.borrow_mut() = None;
            }
            Err(error) => *self.action_error.borrow_mut() = Some(error),
        }
        self.update(snapshot);
    }
}
