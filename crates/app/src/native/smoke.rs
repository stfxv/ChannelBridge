//! Isolated non-visible checks; never request permission or open audio.
use super::application::AppDelegate;
use super::application::Runtime;
use super::screen::Screen;
use channelbridge_core::CaptureState;
use channelbridge_core::Command;
use channelbridge_core::Config;
use channelbridge_core::controller::Backend;
use channelbridge_core::worker::Worker;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceScreensDidWakeNotification,
    NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{NSDate, NSNotificationName, NSRunLoop};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[derive(Default)]
struct AudioLifecycle {
    opens: AtomicUsize,
    closes: AtomicUsize,
    sending: AtomicBool,
}
/// Keeps all notification checks independent of microphones, HAL and permission.
struct FakeAudio(Arc<AudioLifecycle>);
impl Backend for FakeAudio {
    type Pending = ();
    type Session = ();
    fn save(&mut self, _: &Config) -> Result<(), String> {
        Ok(())
    }
    fn negotiate(&mut self, _: &Config) -> Result<(), String> {
        Ok(())
    }
    fn ready(&mut self, _: &()) -> Result<bool, String> {
        Ok(true)
    }
    fn open(&mut self, _: &Config) -> Result<(), String> {
        self.0.opens.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn healthy(&mut self, _: &(), _: &Config) -> Result<bool, String> {
        Ok(true)
    }
    fn route(&mut self, _: &(), _: &Config, sending: bool, _: bool) -> Result<(), String> {
        self.0.sending.store(sending, Ordering::Release);
        Ok(())
    }
    fn close(&mut self, _: ()) -> Result<(), String> {
        self.0.sending.store(false, Ordering::Release);
        self.0.closes.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Post through the same NSWorkspace center as macOS, without sleeping the Mac.
/// A fake backend makes every release, reopen and send intent observable.
pub(super) fn sleep_wake(main: MainThreadMarker) {
    let lifecycle = Arc::new(AudioLifecycle::default());
    let backend_lifecycle = Arc::clone(&lifecycle);
    let worker = Worker::spawn(
        Config {
            device_uid: "sleep-wake-fixture".into(),
            ..Config::default()
        },
        move || Ok(FakeAudio(backend_lifecycle)),
    )
    .expect("sleep fixture worker");
    let settings_screen = Screen::new(main, None);
    let mut settings_snapshot = worker.snapshot();
    settings_snapshot.config.device_uid.clear();
    settings_screen.update(&settings_snapshot);
    assert!(!settings_screen.auto_send_button.isEnabled());
    assert!(!settings_screen.mute_button.isEnabled());
    settings_snapshot.config.device_uid = "sleep-wake-fixture".into();
    settings_screen.update(&settings_snapshot);
    assert!(settings_screen.auto_send_button.isEnabled());
    assert!(settings_screen.mute_button.isEnabled());
    let delegate = AppDelegate::new(
        main,
        Runtime {
            menu: None,
            screen: Screen::new(main, None),
            worker,
            permission_monitor: false,
        },
    );
    let send = |command| {
        delegate
            .ivars()
            .runtime
            .borrow()
            .as_ref()
            .expect("sleep fixture")
            .worker
            .send(command)
            .expect("sleep fixture command");
    };
    let wait = |label: &str, predicate: &dyn Fn(&channelbridge_core::worker::Snapshot) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let snapshot = delegate
                .ivars()
                .runtime
                .borrow()
                .as_ref()
                .expect("sleep fixture")
                .worker
                .snapshot();
            if predicate(&snapshot) {
                return snapshot;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{label}: state={:?}, opens={}, closes={}, error={:?}",
                snapshot.state,
                lifecycle.opens.load(Ordering::Acquire),
                lifecycle.closes.load(Ordering::Acquire),
                snapshot.error,
            );
            NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
        }
    };
    let workspace = NSWorkspace::sharedWorkspace();
    let center = workspace.notificationCenter();
    let post = |name: &NSNotificationName| {
        // SAFETY: this isolated main-thread fixture posts only NSWorkspace's
        // power notifications, with their documented workspace object and no
        // userInfo. It never invokes an
        // OS power operation or opens native audio.
        unsafe { center.postNotificationName_object(name, Some(&workspace)) };
    };
    send(Command::PermissionChanged(true));
    send(Command::Start);
    wait("initial sending", &|snapshot| {
        snapshot.state == CaptureState::Sending
    });
    assert_eq!(lifecycle.opens.load(Ordering::Acquire), 1);
    assert!(lifecycle.sending.load(Ordering::Acquire));

    // SAFETY: This notification name is an immutable framework-owned string constant.
    post(unsafe { NSWorkspaceWillSleepNotification });
    let asleep = wait("sleep must release the audio session", &|_| {
        lifecycle.closes.load(Ordering::Acquire) == 1
    });
    assert!(asleep.wanted_send);
    // Screen notifications alone must not undo system-sleep suspension.
    // SAFETY: This notification name is an immutable framework-owned string constant.
    post(unsafe { NSWorkspaceScreensDidWakeNotification });
    // Leave several controller ticks available: sleep must suppress reopening.
    NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.35));
    assert_eq!(lifecycle.opens.load(Ordering::Acquire), 1);
    assert!(!lifecycle.sending.load(Ordering::Acquire));
    // SAFETY: This notification name is an immutable framework-owned string constant.
    post(unsafe { NSWorkspaceDidWakeNotification });
    wait("wake must reopen and restore sending", &|snapshot| {
        lifecycle.opens.load(Ordering::Acquire) == 2 && snapshot.state == CaptureState::Sending
    });
    assert!(lifecycle.sending.load(Ordering::Acquire));

    // SAFETY: This notification name is an immutable framework-owned string constant.
    post(unsafe { NSWorkspaceWillSleepNotification });
    wait("second sleep must release the new session", &|_| {
        lifecycle.closes.load(Ordering::Acquire) == 2
    });
    send(Command::Stop);
    wait("stop while asleep must update intent", &|snapshot| {
        !snapshot.wanted_send
    });
    // SAFETY: This notification name is an immutable framework-owned string constant.
    post(unsafe { NSWorkspaceDidWakeNotification });
    wait("wake after stop must restore preview only", &|snapshot| {
        lifecycle.opens.load(Ordering::Acquire) == 3 && snapshot.state == CaptureState::Previewing
    });
    assert!(!lifecycle.sending.load(Ordering::Acquire));

    // A missed will-sleep notification must still retire an old HAL session.
    send(Command::Start);
    wait("sending before wake-only", &|snapshot| {
        snapshot.state == CaptureState::Sending
    });
    // SAFETY: This notification name is an immutable framework-owned string constant.
    post(unsafe { NSWorkspaceDidWakeNotification });
    wait("wake-only must replace the stale session", &|snapshot| {
        lifecycle.opens.load(Ordering::Acquire) == 4 && snapshot.state == CaptureState::Sending
    });
    assert_eq!(lifecycle.closes.load(Ordering::Acquire), 3);
    assert!(lifecycle.sending.load(Ordering::Acquire));

    send(Command::Quit);
    wait("sleep fixture shutdown", &|snapshot| snapshot.finished);
    let runtime = delegate
        .ivars()
        .runtime
        .borrow_mut()
        .take()
        .expect("sleep fixture runtime");
    runtime.worker.join().expect("sleep fixture shutdown");
    assert_eq!(lifecycle.closes.load(Ordering::Acquire), 4);
    println!(
        "Rust AppKit sleep/wake notifications released and reopened fake audio, preserved send intent and respected stop while asleep; no native audio or system sleep"
    );
}

/// Exercise real settings I/O and native control inventory, without granting
/// permission or opening audio. Uses an isolated, disposable destination.
pub(super) fn device_save(main: MainThreadMarker) {
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
    let worker = channelbridge_macos::native_backend::spawn_worker(
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
    let checkbox_frame = screen.auto_send_button.frame();
    let content = screen.window.contentView().expect("preference content");
    let children = content.subviews();
    for index in 0..children.len() {
        let frame = children.objectAtIndex(index).frame();
        assert!(frame.origin.y >= 0.0);
        assert!(frame.origin.y + frame.size.height <= content.frame().size.height);
    }
    assert!(
        screen.export.button.frame().origin.y + screen.export.button.frame().size.height
            < checkbox_frame.origin.y
    );
    assert!(
        checkbox_frame.origin.y + checkbox_frame.size.height < screen.send_button.frame().origin.y
    );
    assert_eq!(screen.auto_send_button.state(), 0);
    assert!(screen.auto_send_button.isEnabled());
    std::fs::remove_file(&destination).expect("replace config with failure fixture");
    std::fs::create_dir(&destination).expect("create preference save failure");
    screen.auto_send_button.setState(1);
    screen.toggle_auto_send(&worker);
    assert_eq!(screen.auto_send_button.state(), 0);
    assert!(!screen.auto_send_button.isEnabled());
    let rejected = wait(&|snapshot| snapshot.configuration_attempt > muted.configuration_attempt);
    screen.update(&rejected);
    assert!(rejected.command_error.is_some());
    assert_eq!(rejected.config, muted.config);
    assert_eq!(screen.auto_send_button.state(), 0);
    assert_eq!(screen.devices.indexOfSelectedItem(), 1);
    assert!(screen.auto_send_button.isEnabled());
    std::fs::remove_dir(&destination).expect("remove preference save failure");
    screen.auto_send_button.setState(1);
    screen.toggle_auto_send(&worker);
    let enabled = wait(&|snapshot| snapshot.configuration_attempt > rejected.configuration_attempt);
    screen.update(&enabled);
    let mut expected = muted.config.clone();
    expected.auto_send_on_resume = true;
    assert_eq!(enabled.config, expected);
    assert_eq!(
        channelbridge_core::storage::load(&destination).expect("resume preference"),
        expected
    );
    assert_eq!(screen.auto_send_button.state(), 1);
    assert!(!enabled.wanted_send);
    assert_eq!(screen.devices.indexOfSelectedItem(), 1);
    screen.auto_send_button.setState(0);
    screen.toggle_auto_send(&worker);
    let disabled = wait(&|snapshot| snapshot.configuration_attempt > enabled.configuration_attempt);
    screen.update(&disabled);
    assert_eq!(disabled.config, muted.config);
    assert_eq!(screen.auto_send_button.state(), 0);
    assert_eq!(
        channelbridge_core::storage::load(&destination).expect("disabled resume preference"),
        muted.config
    );
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
    screen.auto_send_button.setEnabled(true);
    screen.auto_send_button.setState(1);
    screen.toggle_auto_send(&worker);
    assert_eq!(screen.auto_send_button.state(), 0);
    assert!(screen.action_error.borrow().is_some());
    worker.join().expect("fixture worker shutdown");
    std::fs::remove_dir_all(directory).expect("remove fixture");
    println!(
        "Rust AppKit device selection preserved failed saves and acknowledged successful saves; no audio opened"
    );
}
