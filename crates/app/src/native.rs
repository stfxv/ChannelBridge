//! AppKit objects never leave the main thread. Audio controls are wired later.
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSTextField, NSWindow, NSWindowStyleMask,
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
fn window(main: MainThreadMarker) -> Retained<NSWindow> {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(380.0, 180.0));
    // SAFETY: main-thread allocation, valid dimensions/style, no deferred backing.
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
    // Retained Rust owner manages lifetime; closing must not release it again.
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(ns_string!("ChannelBridge Rust Test"));
    let permission = channelbridge_macos::permission::status();
    let message = if permission.allowed() {
        "入力機器を選択してください"
    } else {
        "マイクの使用許可が必要です"
    };
    let label = NSTextField::labelWithString(&NSString::from_str(message), main);
    label.setFrame(NSRect::new(
        NSPoint::new(20.0, 120.0),
        NSSize::new(340.0, 30.0),
    ));
    window
        .contentView()
        .expect("window content view")
        .addSubview(&label);
    window
}
pub fn run() {
    let main = MainThreadMarker::new().expect("AppKit must start on the main thread");
    let app = NSApplication::sharedApplication(main);
    let delegate = AppDelegate::new(main);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let window = window(main);
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    if smoke {
        assert_eq!(
            window.contentView().expect("content view").subviews().len(),
            1
        );
        println!("Rust AppKit window constructed; no permission request or audio start");
        app.setDelegate(None);
        return;
    }
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    window.center();
    window.makeKeyAndOrderFront(None);
    app.run();
    app.setDelegate(None);
}
