//! Workspace notifications signal the worker; no UI or HAL calls in selectors.
use channelbridge_core::{SystemEvent, worker::SystemEvents};
use objc2::{AnyThread, DefinedClass, define_class, msg_send, rc::Retained, sel};
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceScreensDidWakeNotification,
    NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol};

pub(super) struct State {
    events: SystemEvents,
    center: Retained<NSNotificationCenter>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = State]
    pub(super) struct PowerObserver;
    // SAFETY: define_class inherits NSObject and preserves its object/protocol invariants.
    unsafe impl NSObjectProtocol for PowerObserver {}
    impl PowerObserver {
        #[unsafe(method(systemWillSleep:))]
        fn sleep(&self, _notification: &NSNotification) {
            self.ivars().events.notify(SystemEvent::Sleep);
        }
        #[unsafe(method(systemDidWake:))]
        fn wake(&self, _notification: &NSNotification) {
            self.ivars().events.notify(SystemEvent::Wake);
        }
        #[unsafe(method(screensDidWake:))]
        fn screens_wake(&self, _notification: &NSNotification) {
            self.ivars().events.notify(SystemEvent::ScreensWake);
        }
    }
);
impl PowerObserver {
    pub(super) fn new(events: SystemEvents) -> Retained<Self> {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let allocated = Self::alloc().set_ivars(State { events, center });
        // SAFETY: NSObject initialization with fully initialized Rust ivars.
        let observer: Retained<Self> = unsafe { msg_send![super(allocated), init] };
        // SAFETY: all selectors take one notification and access only atomics.
        // NSWorkspace uses its own center. The delegate retains this observer,
        // and Drop unregisters it before its ivars can be released.
        unsafe {
            for (name, action) in [
                (NSWorkspaceWillSleepNotification, sel!(systemWillSleep:)),
                (NSWorkspaceDidWakeNotification, sel!(systemDidWake:)),
                (NSWorkspaceScreensDidWakeNotification, sel!(screensDidWake:)),
            ] {
                observer.ivars().center.addObserver_selector_name_object(
                    &observer,
                    action,
                    Some(name),
                    None,
                );
            }
        }
        observer
    }
}
impl Drop for PowerObserver {
    fn drop(&mut self) {
        // SAFETY: remove only this observer; never retain self during deallocation.
        unsafe {
            self.ivars().center.removeObserver(self);
        }
    }
}
