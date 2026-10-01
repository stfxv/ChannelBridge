//! AppKit objects never leave the main thread. Audio controls are wired later.
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSBackingStoreType, NSPopUpButton, NSTextField, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSDate, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize,
    NSString, NSTimer, ns_string,
};

struct Runtime {
    screen: Screen,
    worker: channelbridge_core::worker::Worker,
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
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn close_last_window(&self, _application: &NSApplication) -> bool { true }
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _application: &NSApplication) -> NSApplicationTerminateReply {
            self.ivars().shutting_down.set(true);
            if let Some(runtime) = self.ivars().runtime.borrow().as_ref() { runtime.worker.request_shutdown(); }
            NSApplicationTerminateReply::TerminateLater
        }
    }
    impl AppDelegate {
        #[unsafe(method(refresh:))]
        fn refresh(&self, _timer: &NSTimer) {
            let main = MainThreadMarker::new().expect("UI timer must run on main thread");
            self.ivars().ticks.set(self.ivars().ticks.get().saturating_add(1));
            let state = self.ivars().runtime.borrow();
            if let Some(runtime) = state.as_ref() {
                let snapshot = runtime.worker.snapshot();
                runtime.screen.update(&snapshot);
                if snapshot.finished && self.ivars().shutting_down.replace(false) {
                    NSApplication::sharedApplication(main).replyToApplicationShouldTerminate(true);
                }
            }
        }
    }
);
impl AppDelegate {
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
    window: Retained<NSWindow>,
    devices: Retained<NSPopUpButton>,
    format: Retained<NSTextField>,
    status: Retained<NSTextField>,
    settings_error: Option<String>,
}
impl Screen {
    fn new(main: MainThreadMarker, settings_error: Option<String>) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(380.0, 180.0));
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
            NSRect::new(NSPoint::new(20.0, 110.0), NSSize::new(340.0, 32.0)),
            false,
        );
        // Display-only until actions and transactional config acknowledgement are wired.
        devices.setEnabled(false);
        let format = NSTextField::labelWithString(ns_string!("—"), main);
        format.setFrame(NSRect::new(
            NSPoint::new(20.0, 75.0),
            NSSize::new(340.0, 24.0),
        ));
        let status = NSTextField::labelWithString(ns_string!("入力機器を確認しています"), main);
        status.setFrame(NSRect::new(
            NSPoint::new(20.0, 30.0),
            NSSize::new(340.0, 30.0),
        ));
        let view = window.contentView().expect("window content view");
        view.addSubview(&devices);
        view.addSubview(&format);
        view.addSubview(&status);
        Self {
            window,
            devices,
            format,
            status,
            settings_error,
        }
    }
    fn update(&self, snapshot: &channelbridge_core::worker::Snapshot) {
        self.devices.removeAllItems();
        let selected = snapshot
            .devices
            .iter()
            .position(|device| device.uid == snapshot.config.device_uid);
        let placeholder = if snapshot.config.device_uid.is_empty() {
            "入力機器を選択してください"
        } else {
            "保存した入力機器（未接続）"
        };
        self.devices
            .addItemWithTitle(&NSString::from_str(placeholder));
        for device in snapshot.devices.iter() {
            self.devices
                .addItemWithTitle(&NSString::from_str(&device.name));
        }
        self.devices
            .selectItemAtIndex(selected.map_or(0, |index| index as isize + 1));
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
        } else if snapshot.device_error.is_some() {
            "入力機器を確認できません"
        } else if snapshot.config.device_uid.is_empty() {
            "入力機器を選択してください"
        } else if selected.is_none() {
            "選択した入力機器が接続されていません"
        } else {
            "送出停止中"
        };
        self.status.setStringValue(&NSString::from_str(status));
    }
}
pub fn run() {
    let main = MainThreadMarker::new().expect("AppKit must start on the main thread");
    let app = NSApplication::sharedApplication(main);
    let smoke = std::env::args().any(|argument| argument == "--smoke");
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
    let delegate = AppDelegate::new(main, Runtime { screen, worker });
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
                3
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
