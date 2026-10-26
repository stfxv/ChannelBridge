//! Main-thread application lifecycle, selectors and timer ownership.
use super::{power_observer, screen::Screen, smoke};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSButton, NSMenu, NSMenuDelegate, NSPopUpButton,
};
use objc2_foundation::{
    NSDate, NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer,
    ns_string,
};

pub(super) struct Runtime {
    pub(super) menu: Option<crate::status_menu::StatusMenu>,
    pub(super) screen: Screen,
    pub(super) worker: channelbridge_core::worker::Worker,
    pub(super) permission_monitor: bool,
}
pub(super) struct DelegateState {
    _power: Retained<power_observer::PowerObserver>,
    pub(super) runtime: std::cell::RefCell<Option<Runtime>>,
    shutting_down: std::cell::Cell<bool>,
    ticks: std::cell::Cell<u64>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateState]
    pub(super) struct AppDelegate;
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
        #[unsafe(method(saveDiagnostics:))]
        fn save_diagnostics(&self, _sender: &NSButton) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() {
                let main = MainThreadMarker::new().expect("save action on main thread");
                runtime.screen.export.begin(main, runtime.worker.snapshot());
            }
        }
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
        #[unsafe(method(toggleAutoSend:))]
        fn toggle_auto_send(&self, _sender: &NSButton) {
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() {
                runtime.screen.toggle_auto_send(&runtime.worker);
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
                if snapshot.finished && !runtime.screen.export.pending() && self.ivars().shutting_down.replace(false) {
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
    pub(super) fn new(main: MainThreadMarker, runtime: Runtime) -> Retained<Self> {
        let allocated = Self::alloc(main).set_ivars(DelegateState {
            _power: power_observer::PowerObserver::new(runtime.worker.system_events()),
            runtime: std::cell::RefCell::new(Some(runtime)),
            shutting_down: std::cell::Cell::new(false),
            ticks: std::cell::Cell::new(0),
        });
        // SAFETY: NSObject init after fully initializing Rust-owned ivars.
        unsafe { msg_send![super(allocated), init] }
    }
}
pub fn run() {
    let main = MainThreadMarker::new().expect("AppKit must start on the main thread");
    let app = NSApplication::sharedApplication(main);
    if std::env::args().any(|argument| argument == "--sleep-wake-smoke") {
        smoke::sleep_wake(main);
        return;
    }
    let menu = NSMenu::initWithTitle(NSMenu::alloc(main), ns_string!(""));
    let app_menu = NSMenu::initWithTitle(
        NSMenu::alloc(main),
        &NSString::from_str(channelbridge_contract::DEPLOYMENT.app_name),
    );
    // SAFETY: nil root action; quit explicitly targets NSApplication's public
    // terminate: selector, with the standard Command-Q key equivalent.
    unsafe {
        let root = menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str(channelbridge_contract::DEPLOYMENT.app_name),
            None,
            ns_string!(""),
        );
        root.setSubmenu(Some(&app_menu));
        let quit = app_menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str(&format!(
                "{}を終了",
                channelbridge_contract::DEPLOYMENT.app_name
            )),
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
            channelbridge_contract::DEPLOYMENT.app_bundle_id
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
        crate::settings::Settings::smoke()
    } else {
        let Some(home) = std::env::var_os("HOME") else {
            eprintln!("設定保存先を取得できません: HOMEがありません");
            return;
        };
        crate::settings::Settings::for_home(std::path::Path::new(&home))
    };
    let screen = Screen::new(main, settings.error);
    let worker =
        channelbridge_macos::native_backend::spawn_worker(settings.destination, settings.config)
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
            (&screen.auto_send_button, sel!(toggleAutoSend:)),
            (&screen.export.button, sel!(saveDiagnostics:)),
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
                18
            );
            assert_eq!(
                runtime.screen.export.button.title().to_string(),
                "診断を保存"
            );
            assert!(runtime.screen.export.button.isEnabled());
            assert!(runtime.screen.export.button.frame().origin.y > 35.0);
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
        smoke::device_save(main);
        smoke::sleep_wake(main);
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
