//! Isolated non-visible checks; never request permission or open audio.
use super::screen::Screen;
use objc2::MainThreadMarker;
use objc2_foundation::{NSDate, NSRunLoop};

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
