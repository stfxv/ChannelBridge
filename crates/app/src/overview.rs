//! A bounded, scrollable dry-input meter list, updated only while visible.
use channelbridge_core::worker::Snapshot;
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSBackingStoreType, NSScrollView, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use std::cell::RefCell;

pub struct Overview {
    pub window: Retained<NSWindow>,
    document: Retained<NSView>,
    rows: RefCell<Vec<Retained<NSTextField>>>,
}
impl Overview {
    pub fn new(main: MainThreadMarker) -> Self {
        // SAFETY: main thread, valid native geometry and window style.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(main),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 440.0)),
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
        window.setTitle(ns_string!("入力レベル一覧"));
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(main),
            NSRect::new(NSPoint::new(20.0, 20.0), NSSize::new(360.0, 400.0)),
        );
        scroll.setHasVerticalScroller(true);
        let document = NSView::initWithFrame(
            NSView::alloc(main),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 400.0)),
        );
        scroll.setDocumentView(Some(&document));
        window
            .contentView()
            .expect("overview content")
            .addSubview(&scroll);
        Self {
            window,
            document,
            rows: Default::default(),
        }
    }
    pub fn update(&self, main: MainThreadMarker, snapshot: &Snapshot) {
        let count = snapshot
            .devices
            .iter()
            .find(|device| device.uid == snapshot.config.device_uid)
            .map_or(0, |device| device.channels.min(512) as usize);
        let mut rows = self.rows.borrow_mut();
        if rows.len() != count {
            for row in rows.drain(..) {
                row.removeFromSuperview();
            }
            let height = (count as f64 * 25.0).max(400.0);
            self.document.setFrameSize(NSSize::new(340.0, height));
            for index in 0..count {
                let label = NSTextField::labelWithString(ns_string!("—"), main);
                label.setFrame(NSRect::new(
                    NSPoint::new(0.0, height - (index + 1) as f64 * 25.0),
                    NSSize::new(340.0, 23.0),
                ));
                self.document.addSubview(&label);
                rows.push(label);
            }
            if let Some(first) = rows.first() {
                self.document.scrollRectToVisible(first.frame());
            }
        }
        for (index, row) in rows.iter().enumerate() {
            row.setStringValue(&NSString::from_str(&crate::input_meters::channel_text(
                index as u32 + 1,
                snapshot.audio.as_ref(),
            )));
        }
    }
    pub fn smoke(&self, main: MainThreadMarker, snapshot: &Snapshot) {
        self.update(main, snapshot);
        assert!(!self.window.isVisible());
        assert!(self.rows.borrow().len() <= 512);
        let mut missing = snapshot.clone();
        missing.config.device_uid = "missing-overview-source".into();
        self.update(main, &missing);
        assert!(self.rows.borrow().is_empty());
        self.update(main, snapshot);
        if let Some(first) = self.rows.borrow().first() {
            assert!(first.stringValue().to_string().starts_with("Ch 1:"));
        }
    }
}
