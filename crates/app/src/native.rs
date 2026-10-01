//! AppKit objects never leave the main thread. Audio controls are wired later.
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSBackingStoreType, NSButton, NSMenu, NSMenuDelegate,
    NSPopUpButton, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSDate, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize,
    NSString, NSTimer, ns_string,
};

struct Runtime {
    menu: Option<crate::status_menu::StatusMenu>,
    screen: Screen,
    worker: channelbridge_core::worker::Worker,
    permission_monitor: bool,
}
struct DelegateState {
    runtime: std::cell::RefCell<Option<Runtime>>,
    shutting_down: std::cell::Cell<bool>,
    ticks: std::cell::Cell<u64>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateState]
    struct AppDelegate;
    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl NSMenuDelegate for AppDelegate {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() { runtime.screen.menu_open.set(true); }
        }
        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() { runtime.screen.menu_open.set(false); }
        }
    }
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn close_last_window(&self, _application: &NSApplication) -> bool { false }
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _application: &NSApplication, _visible: bool) -> bool {
            self.reveal_window();
            true
        }
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _application: &NSApplication) -> NSApplicationTerminateReply {
            self.ivars().shutting_down.set(true);
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() { runtime.worker.request_shutdown(); }
            NSApplicationTerminateReply::TerminateLater
        }
    }
    impl AppDelegate {
        #[unsafe(method(showOverview:))]
        fn show_overview(&self, _sender: &NSButton) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() {
                runtime.screen.show_overview();
            }
        }
        #[unsafe(method(showWindow:))]
        fn show_window(&self, _sender: &objc2::runtime::AnyObject) {
            self.reveal_window();
        }
        #[unsafe(method(requestMicrophone:))]
        fn request_microphone(&self, _sender: &NSButton) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() {
                let main = MainThreadMarker::new().expect("permission action on main thread");
                if let Err(error) = runtime.screen.permission.request(main) {
                    *runtime.screen.action_error.borrow_mut() = Some(error.into());
                }
            }
        }
        #[unsafe(method(toggleSend:))]
        fn toggle_send(&self, _sender: &objc2::runtime::AnyObject) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() { runtime.screen.toggle_send(&runtime.worker); }
        }
        #[unsafe(method(toggleMute:))]
        fn toggle_mute(&self, _sender: &objc2::runtime::AnyObject) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() {
                runtime.screen.toggle_mute(&runtime.worker);
            }
        }
        #[unsafe(method(selectRouting:))]
        fn select_routing(&self, sender: &NSPopUpButton) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() {
                runtime.screen.select_routing(sender, &runtime.worker);
            }
        }
        #[unsafe(method(selectDevice:))]
        fn select_device(&self, _sender: &NSPopUpButton) {
            let state = self.ivars().runtime.borrow();
            if let Some(runtime) = state.as_ref() {
                runtime.screen.select_device(&runtime.worker);
            }
        }
        #[unsafe(method(refresh:))]
        fn refresh(&self, _timer: &NSTimer) {
            let main = MainThreadMarker::new().expect("UI timer must run on main thread");
            self.ivars().ticks.set(self.ivars().ticks.get().saturating_add(1));
            let state = self.ivars().runtime.borrow();
            if let Some(runtime) = state.as_ref() {
                let snapshot = runtime.worker.snapshot();
                if runtime.permission_monitor && !snapshot.finished && self.ivars().ticks.get() % 20 == 1 {
                    runtime.screen.sync_permission(&runtime.worker);
                }
                runtime.screen.update(&snapshot);
                runtime.screen.sync_overview(&runtime.worker, &snapshot);
                if let Some(menu) = runtime.menu.as_ref() {
                    menu.update(main, &snapshot, runtime.screen.send_button.isEnabled(), runtime.screen.mute_button.isEnabled());
                }
                if snapshot.finished && self.ivars().shutting_down.replace(false) {
                    NSApplication::sharedApplication(main).replyToApplicationShouldTerminate(true);
                }
            }
        }
    }
);
impl AppDelegate {
    fn reveal_window(&self) {
        if let Some(runtime) = self.ivars().runtime.borrow().as_ref() {
            runtime.screen.window.makeKeyAndOrderFront(None);
            let main = MainThreadMarker::new().expect("window action on main thread");
            // NSApplication.activate is newer than the macOS 13 deployment target.
            #[allow(deprecated)]
            NSApplication::sharedApplication(main).activateIgnoringOtherApps(true);
        }
    }
    fn new(main: MainThreadMarker, runtime: Runtime) -> Retained<Self> {
        let allocated = Self::alloc(main).set_ivars(DelegateState {
            runtime: std::cell::RefCell::new(Some(runtime)),
            shutting_down: std::cell::Cell::new(false),
            ticks: std::cell::Cell::new(0),
        });
        // SAFETY: NSObject init after fully initializing Rust-owned ivars.
        unsafe { msg_send![super(allocated), init] }
    }
}
struct Screen {
    overview: std::cell::RefCell<Option<crate::overview::Overview>>,
    overview_sent: std::cell::Cell<bool>,
    overview_button: Retained<NSButton>,
    permission: channelbridge_macos::permission::MicrophonePermission,
    observed_permission: std::cell::Cell<Option<channelbridge_macos::permission::Permission>>,
    permission_button: Retained<NSButton>,
    send_button: Retained<NSButton>,
    mute_button: Retained<NSButton>,
    meters: crate::input_meters::InputMeters,
    routing: crate::routing_controls::RoutingControls,
    window: Retained<NSWindow>,
    devices: Retained<NSPopUpButton>,
    format: Retained<NSTextField>,
    status: Retained<NSTextField>,
    settings_error: Option<String>,
    device_items: std::cell::RefCell<Vec<(String, String)>>,
    displayed_uid: std::cell::RefCell<String>,
    pending_attempt: std::cell::Cell<Option<u64>>,
    action_error: std::cell::RefCell<Option<String>>,
    menu_open: std::cell::Cell<bool>,
}
impl Screen {
    fn new(main: MainThreadMarker, settings_error: Option<String>) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(380.0, 400.0));
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
        window.setTitle(ns_string!("ChannelBridge Rust Test"));
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
        Self {
            overview: Default::default(),
            overview_sent: Default::default(),
            overview_button,
            permission: Default::default(),
            observed_permission: Default::default(),
            permission_button,
            send_button,
            mute_button,
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
    fn update(&self, snapshot: &channelbridge_core::worker::Snapshot) {
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
    fn show_overview(&self) {
        let main = MainThreadMarker::new().expect("overview on main thread");
        let mut overview = self.overview.borrow_mut();
        let overview = overview.get_or_insert_with(|| crate::overview::Overview::new(main));
        overview.window.center();
        overview.window.makeKeyAndOrderFront(None);
    }
    fn sync_overview(
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
    fn sync_permission(&self, worker: &channelbridge_core::worker::Worker) {
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
    fn toggle_send(&self, worker: &channelbridge_core::worker::Worker) {
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
    fn toggle_mute(&self, worker: &channelbridge_core::worker::Worker) {
        if self.settings_error.is_some() || self.pending_attempt.get().is_some() {
            return;
        }
        let snapshot = worker.snapshot();
        let mut config = snapshot.config.clone();
        config.muted = !config.muted;
        self.submit_config(worker, &snapshot, config);
    }
    fn select_device(&self, worker: &channelbridge_core::worker::Worker) {
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
    fn select_routing(&self, sender: &NSPopUpButton, worker: &channelbridge_core::worker::Worker) {
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
pub fn run() {
    let main = MainThreadMarker::new().expect("AppKit must start on the main thread");
    let app = NSApplication::sharedApplication(main);
    let menu = NSMenu::initWithTitle(NSMenu::alloc(main), ns_string!(""));
    let app_menu =
        NSMenu::initWithTitle(NSMenu::alloc(main), ns_string!("ChannelBridge Rust Test"));
    // SAFETY: nil root action; quit explicitly targets NSApplication's public
    // terminate: selector, with the standard Command-Q key equivalent.
    unsafe {
        let root = menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("ChannelBridge Rust Test"),
            None,
            ns_string!(""),
        );
        root.setSubmenu(Some(&app_menu));
        let quit = app_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("ChannelBridge Rust Testを終了"),
            Some(sel!(terminate:)),
            ns_string!("q"),
        );
        quit.setTarget(Some(&app));
    }
    app.setMainMenu(Some(&menu));
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    let check_bundle = std::env::args().any(|argument| argument == "--check-bundle");
    if check_bundle || std::env::args().any(|argument| argument == "--require-bundle") {
        let bundle = objc2_foundation::NSBundle::mainBundle();
        assert_eq!(
            bundle
                .bundleIdentifier()
                .expect("bundle identifier")
                .to_string(),
            "local.channelbridge.rust.app.test"
        );
        let description = bundle
            .objectForInfoDictionaryKey(ns_string!("NSMicrophoneUsageDescription"))
            .expect("microphone usage description");
        assert!(
            !description
                .downcast_ref::<NSString>()
                .expect("usage NSString")
                .is_empty()
        );
        if check_bundle {
            println!("Rust test bundle identifier and microphone description verified");
            return;
        }
    }
    let settings = if smoke {
        crate::startup::Settings::smoke()
    } else {
        let Some(home) = std::env::var_os("HOME") else {
            eprintln!("設定保存先を取得できません: HOMEがありません");
            return;
        };
        crate::startup::Settings::for_home(std::path::Path::new(&home))
    };
    let screen = Screen::new(main, settings.error);
    let worker = channelbridge_macos::backend::spawn_worker(settings.destination, settings.config)
        .expect("control worker");
    let delegate = AppDelegate::new(
        main,
        Runtime {
            menu: None,
            screen,
            worker,
            permission_monitor: !smoke,
        },
    );
    if !smoke {
        let menu = crate::status_menu::StatusMenu::new(main, &delegate);
        delegate
            .ivars()
            .runtime
            .borrow_mut()
            .as_mut()
            .expect("runtime")
            .menu = Some(menu);
    }
    {
        let state = delegate.ivars().runtime.borrow();
        let screen = &state.as_ref().expect("runtime").screen;
        // SAFETY: retained main-thread delegate implements showOverview:.
        unsafe {
            screen.overview_button.setTarget(Some(&delegate));
            screen.overview_button.setAction(Some(sel!(showOverview:)));
        }
        for (button, action) in [
            (&screen.permission_button, sel!(requestMicrophone:)),
            (&screen.send_button, sel!(toggleSend:)),
            (&screen.mute_button, sel!(toggleMute:)),
        ] {
            // SAFETY: matching main-thread delegate selectors; delegate retained
            // through the event loop, targets are weak.
            unsafe {
                button.setTarget(Some(&delegate));
                button.setAction(Some(action));
            }
        }
        // SAFETY: main-thread delegate outlives the control and implements the
        // one-argument selector. AppKit target is weak, retained locally below.
        unsafe {
            screen.devices.setTarget(Some(&delegate));
            screen.devices.setAction(Some(sel!(selectDevice:)));
        }
        screen
            .devices
            .menu()
            .expect("device menu")
            .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        for popup in [
            &screen.routing.left,
            &screen.routing.right,
            &screen.routing.mode,
        ] {
            // SAFETY: same main-thread retained delegate and matching selector.
            unsafe {
                popup.setTarget(Some(&delegate));
                popup.setAction(Some(sel!(selectRouting:)));
            }
            popup
                .menu()
                .expect("routing menu")
                .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
    }
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // SAFETY: matching Objective-C selector implemented above, main-thread
    // target retained by timer; no userInfo. Only the main run loop owns it.
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            0.05,
            &delegate,
            sel!(refresh:),
            None,
            true,
        )
    };
    let mode = unsafe { NSRunLoopCommonModes };
    // SAFETY: MainThreadMarker above proves this is the timer owning run loop.
    unsafe {
        NSRunLoop::mainRunLoop().addTimer_forMode(&timer, mode);
    }
    if smoke {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let ready = !delegate
                .ivars()
                .runtime
                .borrow()
                .as_ref()
                .expect("runtime")
                .worker
                .snapshot()
                .devices
                .is_empty();
            if ready && delegate.ivars().ticks.get() > 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "inventory timer timed out"
            );
            NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
        }
        {
            let state = delegate.ivars().runtime.borrow();
            let runtime = state.as_ref().expect("runtime");
            let snapshot = runtime.worker.snapshot();
            assert_eq!(
                runtime
                    .screen
                    .window
                    .contentView()
                    .expect("content view")
                    .subviews()
                    .len(),
                15
            );
            assert_eq!(runtime.screen.devices.indexOfSelectedItem(), 0);
            if let Some(device) = snapshot.devices.first() {
                let mut selected = snapshot.clone();
                selected.config.device_uid = device.uid.clone();
                runtime.screen.update(&selected);
                assert_eq!(runtime.screen.devices.indexOfSelectedItem(), 1);
                selected.config.device_uid = "missing-device-for-smoke-test".into();
                runtime.screen.update(&selected);
                assert_eq!(runtime.screen.devices.indexOfSelectedItem(), 0);
                assert_eq!(selected.config.device_uid, "missing-device-for-smoke-test");
            }
            runtime.worker.request_shutdown();
        }
        while !delegate
            .ivars()
            .runtime
            .borrow()
            .as_ref()
            .expect("runtime")
            .worker
            .snapshot()
            .finished
        {
            assert!(
                std::time::Instant::now() < deadline,
                "worker shutdown timed out"
            );
            NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
        }
        timer.invalidate();
        let runtime = delegate
            .ivars()
            .runtime
            .borrow_mut()
            .take()
            .expect("runtime");
        runtime.worker.join().expect("worker shutdown");
        app.setDelegate(None);
        smoke_device_save(main);
        println!(
            "Rust AppKit timer refreshed inventory and observed worker shutdown; no audio or permission request"
        );
        return;
    }
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    {
        let state = delegate.ivars().runtime.borrow();
        let screen = &state.as_ref().expect("runtime").screen;
        screen.window.center();
        screen.window.makeKeyAndOrderFront(None);
    }
    app.run();
    timer.invalidate();
    app.setDelegate(None);
}

/// Exercise real settings I/O and native control inventory, without granting
/// permission or opening audio. Uses an isolated, disposable destination.
fn smoke_device_save(main: MainThreadMarker) {
    let directory = std::env::temp_dir().join(format!(
        "channelbridge-ui-save-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let destination = directory.join("settings.json");
    // A directory at the destination deliberately makes the commit fail.
    std::fs::create_dir_all(&destination).expect("fixture directory");
    let worker = channelbridge_macos::backend::spawn_worker(
        destination.clone(),
        channelbridge_core::Config::default(),
    )
    .expect("fixture worker");
    let screen = Screen::new(main, None);
    screen.meters.smoke();
    screen.routing.smoke();
    let wait = |predicate: &dyn Fn(&channelbridge_core::worker::Snapshot) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let snapshot = worker.snapshot();
            if predicate(&snapshot) {
                return snapshot;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "device action timed out"
            );
            NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
        }
    };
    let initial = wait(&|snapshot| !snapshot.devices.is_empty());
    let overview = crate::overview::Overview::new(main);
    let mut overview_fixture = initial.clone();
    overview_fixture.config.device_uid = initial.devices[0].uid.clone();
    overview.smoke(main, &overview_fixture);
    screen.update(&initial);
    let uid = initial.devices[0].uid.clone();
    // Inventory changes while a menu is open are deferred until it closes.
    screen.menu_open.set(true);
    let mut absent = initial.clone();
    absent.devices = std::sync::Arc::from([]);
    screen.update(&absent);
    assert_eq!(screen.device_items.borrow()[0].0, uid);
    screen.menu_open.set(false);
    screen.devices.selectItemAtIndex(1);
    screen.select_device(&worker);
    assert!(!screen.devices.isEnabled());
    let failed = wait(&|snapshot| snapshot.configuration_attempt == 1);
    screen.update(&failed);
    assert!(failed.command_error.is_some());
    assert!(failed.config.device_uid.is_empty());
    assert_eq!(screen.devices.indexOfSelectedItem(), 0);
    assert!(destination.is_dir());
    std::fs::remove_dir(&destination).expect("remove failure fixture");
    screen.devices.selectItemAtIndex(1);
    screen.select_device(&worker);
    let saved = wait(&|snapshot| snapshot.configuration_attempt == 2);
    screen.update(&saved);
    assert!(saved.command_error.is_none());
    assert_eq!(saved.config.device_uid, uid);
    assert_eq!(
        channelbridge_core::storage::load(&destination).expect("saved config"),
        saved.config
    );
    assert_eq!(screen.devices.indexOfSelectedItem(), 1);
    assert!(screen.devices.isEnabled());
    // The placeholder is informational and must not clear a saved selection.
    screen.devices.selectItemAtIndex(0);
    screen.select_device(&worker);
    assert_eq!(screen.devices.indexOfSelectedItem(), 1);
    screen.routing.mode.selectItemAtIndex(1);
    screen.select_routing(&screen.routing.mode, &worker);
    let stereo = wait(&|snapshot| snapshot.configuration_attempt == 3);
    screen.update(&stereo);
    assert_eq!(stereo.config.mode, channelbridge_core::Mode::Stereo);
    assert!(screen.routing.right.isEnabled());
    if stereo.devices[0].channels >= 2 {
        screen.routing.left.selectItemAtIndex(2);
        screen.select_routing(&screen.routing.left, &worker);
        let selected = wait(&|snapshot| snapshot.configuration_attempt == 4);
        screen.update(&selected);
        assert_eq!((selected.config.left, selected.config.right), (2, 2));
        assert_eq!(
            channelbridge_core::storage::load(&destination).expect("channel config"),
            selected.config
        );
    }
    let before_mute = worker.snapshot();
    screen.toggle_mute(&worker);
    let muted =
        wait(&|snapshot| snapshot.configuration_attempt > before_mute.configuration_attempt);
    screen.update(&muted);
    assert!(muted.config.muted);
    assert_eq!(
        channelbridge_core::storage::load(&destination).expect("mute config"),
        muted.config
    );
    assert_eq!(screen.mute_button.title().to_string(), "ミュート解除");
    let mut recovery = muted.clone();
    recovery.wanted_send = true;
    recovery.state = channelbridge_core::CaptureState::Recovering;
    screen
        .pending_attempt
        .set(Some(recovery.configuration_attempt));
    screen.update(&recovery);
    assert!(screen.send_button.isEnabled());
    assert_eq!(screen.send_button.title().to_string(), "停止");
    recovery.finished = true;
    screen.update(&recovery);
    assert!(!screen.send_button.isEnabled());
    worker.request_shutdown();
    wait(&|snapshot| snapshot.finished);
    worker.join().expect("fixture worker shutdown");
    std::fs::remove_dir_all(directory).expect("remove fixture");
    println!(
        "Rust AppKit device selection preserved failed saves and acknowledged successful saves; no audio opened"
    );
}
