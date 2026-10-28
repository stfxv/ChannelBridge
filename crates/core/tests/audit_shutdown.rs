use channelbridge_core::Command;
use channelbridge_core::Config;
use channelbridge_core::SystemEvent;
use channelbridge_core::controller::Backend;
use channelbridge_core::worker::Snapshot;
use channelbridge_core::worker::Worker;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

#[derive(Default)]
struct Fake {
    events: Arc<Mutex<Vec<String>>>,
    opened: Option<mpsc::Sender<()>>,
    saving: Option<mpsc::Sender<()>>,
    save_gate: Option<mpsc::Receiver<()>>,
    close_fail: bool,
    inventory: Option<Arc<Mutex<Result<channelbridge_core::view::DeviceInventory, String>>>>,
}
impl Backend for Fake {
    type Pending = ();
    type Session = ();
    fn devices(&mut self) -> Result<channelbridge_core::view::DeviceInventory, String> {
        self.inventory.as_ref().map_or_else(
            || Ok(channelbridge_core::view::DeviceInventory::default()),
            |inventory| {
                inventory
                    .lock()
                    .expect("fixture threads do not panic while holding this mutex")
                    .clone()
            },
        )
    }
    fn save(&mut self, config: &Config) -> Result<(), String> {
        self.events
            .lock()
            .expect("fixture threads do not panic while holding this mutex")
            .push(format!("save:{}", config.device_uid));
        if let Some(saving) = self.saving.take() {
            saving
                .send(())
                .expect("test retains the save notification receiver until persistence starts");
            self.save_gate
                .take()
                .expect("the saving notification and release gate are installed together")
                .recv()
                .expect("fixture sender signals this rendezvous before dropping its channel");
        }
        if config.device_uid == "fail" {
            Err("save failed".into())
        } else {
            Ok(())
        }
    }
    fn negotiate(&mut self, _: &Config) -> Result<(), String> {
        Ok(())
    }
    fn ready(&mut self, _: &()) -> Result<bool, String> {
        Ok(true)
    }
    fn open(&mut self, _: &Config) -> Result<(), String> {
        self.events
            .lock()
            .expect("fixture threads do not panic while holding this mutex")
            .push("open".into());
        if let Some(opened) = self.opened.take() {
            opened
                .send(())
                .expect("test retains the open notification receiver until the session opens");
        }
        Ok(())
    }
    fn healthy(&mut self, _: &(), _: &Config) -> Result<bool, String> {
        Ok(true)
    }
    fn route(&mut self, _: &(), _: &Config, _: bool, _: bool) -> Result<(), String> {
        self.events
            .lock()
            .expect("fixture threads do not panic while holding this mutex")
            .push("route".into());
        Ok(())
    }
    fn close(&mut self, _: ()) -> Result<(), String> {
        self.events
            .lock()
            .expect("fixture threads do not panic while holding this mutex")
            .push("close".into());
        if self.close_fail {
            Err("close failed".into())
        } else {
            Ok(())
        }
    }
}
fn config(uid: &str) -> Config {
    Config {
        device_uid: uid.into(),
        ..Config::default()
    }
}
fn wait(worker: &Worker, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = worker.snapshot();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not publish expected completion: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn accepted_configuration_fifo_completes_after_sealing_a_full_queue() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let saved = events.clone();
    let (release, gate) = mpsc::channel();
    let worker = Worker::spawn(config("original"), move || {
        gate.recv()
            .expect("fixture sender signals this rendezvous before dropping its channel");
        Ok(Fake {
            events: saved,
            ..Fake::default()
        })
    })
    .expect("fixture worker thread can be created with its gated fake backend");
    for i in 0..32 {
        worker
            .send(Command::Configure(config(&i.to_string())))
            .expect("worker admission is open and the fixture command sequence fits its queue");
    }
    assert!(worker.send(Command::Refresh).is_err());
    worker
        .send(Command::Quit)
        .expect("quit seals worker admission even with a full command queue");
    assert!(worker.send(Command::Configure(config("rejected"))).is_err());
    assert!(worker.send(Command::Start).is_err());
    release
        .send(())
        .expect("gated fake backend retains its receiver until the test releases it");
    let snapshot = wait(&worker, |snapshot| snapshot.finished);
    assert_eq!(snapshot.configuration_attempt, 32);
    assert_eq!(snapshot.config.device_uid, "31");
    assert!(!snapshot.wanted_send);
    assert_eq!(
        *events
            .lock()
            .expect("fixture threads do not panic while holding this mutex"),
        (0..32).map(|i| format!("save:{i}")).collect::<Vec<_>>()
    );
    worker
        .join()
        .expect("fixture worker completes without panic");
}
#[test]
fn shutdown_closes_before_remaining_saves_and_preserves_independent_errors() {
    for (close_fail, finish_success) in [(false, true), (true, false)] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let log = events.clone();
        let (opened, open) = mpsc::channel();
        let (saving, save) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let worker = Worker::spawn(config("source"), move || {
            Ok(Fake {
                events: log,
                opened: Some(opened),
                saving: Some(saving),
                save_gate: Some(gate),
                close_fail,
                ..Fake::default()
            })
        })
        .expect("fixture worker thread can be created with its fake backend");
        worker
            .send(Command::PermissionChanged(true))
            .expect("worker admission is open and the fixture command sequence fits its queue");
        open.recv_timeout(Duration::from_secs(3))
            .expect("fixture sender signals this rendezvous before dropping its channel");
        worker
            .send(Command::Configure(config("source")))
            .expect("worker admission is open and the fixture command sequence fits its queue");
        save.recv_timeout(Duration::from_secs(3))
            .expect("fixture sender signals this rendezvous before dropping its channel");
        worker
            .send(Command::Configure(Config {
                left: 0,
                ..config("invalid")
            }))
            .expect("worker admission is open and the fixture command sequence fits its queue");
        worker
            .send(Command::Configure(config("fail")))
            .expect("worker admission is open and the fixture command sequence fits its queue");
        if finish_success {
            worker
                .send(Command::Configure(config("final")))
                .expect("worker admission is open and the fixture command sequence fits its queue");
        }
        worker.request_shutdown();
        worker
            .send(Command::System(SystemEvent::Wake))
            .expect_err("request_shutdown seals admission before another command is sent");
        release
            .send(())
            .expect("gated fake backend retains its receiver until the test releases it");
        let snapshot = wait(&worker, |snapshot| snapshot.finished);
        assert_eq!(
            snapshot.configuration_attempt,
            if finish_success { 4 } else { 3 }
        );
        assert_eq!(
            snapshot.command_error.as_deref(),
            if finish_success {
                None
            } else {
                Some("save failed")
            }
        );
        assert_eq!(
            snapshot.error.as_deref(),
            if close_fail {
                Some("close failed")
            } else {
                None
            }
        );
        assert_eq!(
            snapshot.config.device_uid,
            if finish_success { "final" } else { "source" }
        );
        let log = events
            .lock()
            .expect("fixture threads do not panic while holding this mutex");
        let closed = log
            .iter()
            .position(|event| event == "close")
            .expect("shutdown records a close event for the opened fake session");
        assert_eq!(
            &log[closed..],
            if finish_success {
                vec!["close", "save:fail", "save:final"]
            } else {
                vec!["close", "save:fail"]
            }
        );
        drop(log);
        worker
            .join()
            .expect("fixture worker completes without panic");
    }
}
#[test]
fn shutdown_without_remaining_configuration_preserves_prior_command_error() {
    let worker = Worker::spawn(config("source"), || Ok(Fake::default()))
        .expect("fixture worker thread can be created with its fake backend");
    worker
        .send(Command::Configure(config("fail")))
        .expect("worker admission is open and the fixture command sequence fits its queue");
    wait(&worker, |snapshot| snapshot.configuration_attempt == 1);
    worker.request_shutdown();
    let snapshot = wait(&worker, |snapshot| snapshot.finished);
    assert_eq!(snapshot.command_error.as_deref(), Some("save failed"));
    assert_eq!(snapshot.configuration_attempt, 1);
    worker
        .join()
        .expect("fixture worker completes without panic");
}
#[test]
fn factory_failure_completes_accepted_configuration_attempts() {
    let (release, gate) = mpsc::channel();
    let worker = Worker::spawn::<Fake>(config("source"), move || {
        gate.recv()
            .expect("fixture sender signals this rendezvous before dropping its channel");
        Err("factory failed".into())
    })
    .expect("fixture worker thread can be created with its gated fake backend");
    for _ in 0..3 {
        worker
            .send(Command::Configure(config("next")))
            .expect("worker admission is open and the fixture command sequence fits its queue");
    }
    worker
        .send(Command::Refresh)
        .expect("worker admission is open and the fixture command sequence fits its queue");
    release
        .send(())
        .expect("gated fake backend retains its receiver until the test releases it");
    let snapshot = wait(&worker, |snapshot| snapshot.finished);
    assert_eq!(snapshot.configuration_attempt, 3);
    assert_eq!(snapshot.command_error.as_deref(), Some("factory failed"));
    assert_eq!(snapshot.config.device_uid, "source");
    assert!(worker.send(Command::Configure(config("rejected"))).is_err());
    worker
        .join()
        .expect("fixture worker completes without panic");
}
#[test]
fn racing_seal_and_senders_accounts_for_every_accepted_configuration() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let saved = events.clone();
    let (release, gate) = mpsc::channel();
    let worker = Worker::spawn(config("source"), move || {
        gate.recv()
            .expect("fixture sender signals this rendezvous before dropping its channel");
        Ok(Fake {
            events: saved,
            ..Fake::default()
        })
    })
    .expect("fixture worker thread can be created with its gated fake backend");
    let accepted = AtomicUsize::new(0);
    let barrier = Barrier::new(5);
    thread::scope(|scope| {
        for _ in 0..4 {
            let (worker, accepted, barrier) = (&worker, &accepted, &barrier);
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..16 {
                    if worker.send(Command::Configure(config("next"))).is_ok() {
                        accepted.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
        barrier.wait();
        worker.request_shutdown();
    });
    release
        .send(())
        .expect("gated fake backend retains its receiver until the test releases it");
    let snapshot = wait(&worker, |snapshot| snapshot.finished);
    assert_eq!(
        snapshot.configuration_attempt,
        accepted.load(Ordering::Relaxed) as u64
    );
    assert_eq!(
        events
            .lock()
            .expect("fixture threads do not panic while holding this mutex")
            .len(),
        accepted.load(Ordering::Relaxed)
    );
    worker
        .join()
        .expect("fixture worker completes without panic");
}

#[test]
fn whole_inventory_failure_retains_choices_and_presence_history() {
    use channelbridge_core::view::{
        ControlEventKind, DeviceChoice, DeviceInventory, InventoryCompleteness,
    };
    let complete = DeviceInventory {
        choices: vec![DeviceChoice {
            uid: "source".into(),
            name: "Input".into(),
            channels: 2,
            rate: 48000.0,
            physical_inputs: Vec::new(),
            format_error: None,
        }],
        completeness: InventoryCompleteness::Complete,
    };
    let inventory = Arc::new(Mutex::new(Ok(complete.clone())));
    let readings = inventory.clone();
    let worker = Worker::spawn(config("source"), move || {
        Ok(Fake {
            inventory: Some(readings),
            ..Fake::default()
        })
    })
    .expect("fixture worker thread can be created with its fake backend");
    wait(&worker, |snapshot| snapshot.devices.len() == 1);
    *inventory
        .lock()
        .expect("fixture threads do not panic while holding this mutex") =
        Err("whole list failed".into());
    worker
        .send(Command::System(SystemEvent::ScreensWake))
        .expect("worker admission is open and the fixture command sequence fits its queue");
    let failed = wait(&worker, |snapshot| {
        snapshot.device_error.as_deref() == Some("whole list failed")
    });
    assert_eq!(&*failed.devices, &complete.choices);
    *inventory
        .lock()
        .expect("fixture threads do not panic while holding this mutex") = Ok(complete);
    worker
        .send(Command::System(SystemEvent::ScreensWake))
        .expect("worker admission is open and the fixture command sequence fits its queue");
    let recovered = wait(&worker, |snapshot| snapshot.device_error.is_none());
    assert!(
        !recovered
            .control_history
            .iter()
            .any(|event| event.kind == ControlEventKind::InputReconnected)
    );
    worker
        .join()
        .expect("fixture worker completes without panic");
}
