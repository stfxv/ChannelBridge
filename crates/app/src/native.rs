//! AppKit objects never leave the main thread. Audio controls are wired later.
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSPopUpButton, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, ns_string};

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct AppDelegate;
    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn close_last_window(&self, _application: &NSApplication) -> bool {
            true
        }
    }
);
impl AppDelegate {
    fn new(main: MainThreadMarker) -> Retained<Self> {
        // SAFETY: NSObject's init initializes this no-ivar main-thread subclass.
        unsafe { msg_send![Self::alloc(main), init] }
    }
}
struct Screen {
    window: Retained<NSWindow>,
    devices: Retained<NSPopUpButton>,
    format: Retained<NSTextField>,
    status: Retained<NSTextField>,
}
impl Screen {
    fn new(main: MainThreadMarker) -> Self {
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
        let status = if snapshot.device_error.is_some() {
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
    let delegate = AppDelegate::new(main);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let screen = Screen::new(main);
    let worker = channelbridge_macos::backend::spawn_worker(
        std::path::PathBuf::from("work/read-only-settings.json"),
        channelbridge_core::Config::default(),
    )
    .expect("control worker");
    // First native inventory snapshot, still without granting permission or I/O.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while worker.snapshot().devices.is_empty() && std::time::Instant::now() < deadline {
        if worker.snapshot().finished {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let snapshot = worker.snapshot();
    screen.update(&snapshot);
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    if smoke {
        assert_eq!(
            screen
                .window
                .contentView()
                .expect("content view")
                .subviews()
                .len(),
            3
        );
        println!("Rust AppKit window constructed; no permission request or audio start");
        assert_eq!(screen.devices.indexOfSelectedItem(), 0);
        assert_eq!(
            screen.devices.numberOfItems() as usize,
            snapshot.devices.len() + 1
        );
        if let Some(device) = snapshot.devices.first() {
            let mut selected = snapshot.clone();
            selected.config.device_uid = device.uid.clone();
            screen.update(&selected);
            assert_eq!(screen.devices.indexOfSelectedItem(), 1);
            assert!(screen.format.stringValue().to_string().contains("kHz"));
            selected.config.device_uid = "missing-device-for-smoke-test".into();
            screen.update(&selected);
            assert_eq!(screen.devices.indexOfSelectedItem(), 0);
            assert_eq!(selected.config.device_uid, "missing-device-for-smoke-test");
        }
        worker.join().expect("worker shutdown");
        app.setDelegate(None);
        return;
    }
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    screen.window.center();
    screen.window.makeKeyAndOrderFront(None);
    app.run();
    app.setDelegate(None);
}
