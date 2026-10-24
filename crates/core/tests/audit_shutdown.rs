use channelbridge_core::{
    Command, Config,
    controller::Backend,
    worker::{Snapshot, Worker},
};
use std::{
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Fake {
    events: Arc<Mutex<Vec<String>>>,
    opened: Option<mpsc::Sender<()>>,
    saving: Option<mpsc::Sender<()>>,
    save_gate: Option<mpsc::Receiver<()>>,
    close_fail: bool,
}
impl Backend for Fake {
    type Pending = ();
    type Session = ();
    fn save(&mut self, config: &Config) -> Result<(), String> {
        self.events
            .lock()
            .unwrap()
            .push(format!("save:{}", config.device_uid));
        if let Some(saving) = self.saving.take() {
            saving.send(()).unwrap();
            self.save_gate.take().unwrap().recv().unwrap();
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
        self.events.lock().unwrap().push("open".into());
        if let Some(opened) = self.opened.take() {
            opened.send(()).unwrap();
        }
        Ok(())
    }
    fn healthy(&mut self, _: &(), _: &Config) -> Result<bool, String> {
        Ok(true)
    }
    fn route(&mut self, _: &(), _: &Config, _: bool, _: bool) -> Result<(), String> {
        self.events.lock().unwrap().push("route".into());
        Ok(())
    }
    fn close(&mut self, _: ()) -> Result<(), String> {
        self.events.lock().unwrap().push("close".into());
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
        gate.recv().unwrap();
        Ok(Fake {
            events: saved,
            ..Fake::default()
        })
    })
    .unwrap();
    for i in 0..32 {
        worker
            .send(Command::Configure(config(&i.to_string())))
            .unwrap();
    }
    assert!(worker.send(Command::Refresh).is_err());
    worker.send(Command::Quit).unwrap();
    assert!(worker.send(Command::Configure(config("rejected"))).is_err());
    assert!(worker.send(Command::Start).is_err());
    release.send(()).unwrap();
    let snapshot = wait(&worker, |snapshot| snapshot.finished);
    assert_eq!(snapshot.configuration_attempt, 32);
    assert_eq!(snapshot.config.device_uid, "31");
    assert!(!snapshot.wanted_send);
    assert_eq!(
        *events.lock().unwrap(),
        (0..32).map(|i| format!("save:{i}")).collect::<Vec<_>>()
    );
    worker.join().unwrap();
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
            })
        })
        .unwrap();
        worker.send(Command::PermissionChanged(true)).unwrap();
        open.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.send(Command::Configure(config("source"))).unwrap();
        save.recv_timeout(Duration::from_secs(3)).unwrap();
        worker
            .send(Command::Configure(Config {
                left: 0,
                ..config("invalid")
            }))
            .unwrap();
        worker.send(Command::Configure(config("fail"))).unwrap();
        if finish_success {
            worker.send(Command::Configure(config("final"))).unwrap();
        }
        worker.request_shutdown();
        worker
            .send(Command::System(channelbridge_core::SystemEvent::Wake))
            .unwrap_err();
        release.send(()).unwrap();
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
        let log = events.lock().unwrap();
        let closed = log.iter().position(|event| event == "close").unwrap();
        assert_eq!(
            &log[closed..],
            if finish_success {
                vec!["close", "save:fail", "save:final"]
            } else {
                vec!["close", "save:fail"]
            }
        );
        drop(log);
        worker.join().unwrap();
    }
}
#[test]
fn shutdown_without_remaining_configuration_preserves_prior_command_error() {
    let worker = Worker::spawn(config("source"), || Ok(Fake::default())).unwrap();
    worker.send(Command::Configure(config("fail"))).unwrap();
    wait(&worker, |snapshot| snapshot.configuration_attempt == 1);
    worker.request_shutdown();
    let snapshot = wait(&worker, |snapshot| snapshot.finished);
    assert_eq!(snapshot.command_error.as_deref(), Some("save failed"));
    assert_eq!(snapshot.configuration_attempt, 1);
    worker.join().unwrap();
}
#[test]
fn factory_failure_completes_accepted_configuration_attempts() {
    let (release, gate) = mpsc::channel();
    let worker = Worker::spawn::<Fake>(config("source"), move || {
        gate.recv().unwrap();
        Err("factory failed".into())
    })
    .unwrap();
    for _ in 0..3 {
        worker.send(Command::Configure(config("next"))).unwrap();
    }
    worker.send(Command::Refresh).unwrap();
    release.send(()).unwrap();
    let snapshot = wait(&worker, |snapshot| snapshot.finished);
    assert_eq!(snapshot.configuration_attempt, 3);
    assert_eq!(snapshot.command_error.as_deref(), Some("factory failed"));
    assert_eq!(snapshot.config.device_uid, "source");
    assert!(worker.send(Command::Configure(config("rejected"))).is_err());
    worker.join().unwrap();
}
#[test]
fn racing_seal_and_senders_accounts_for_every_accepted_configuration() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let saved = events.clone();
    let (release, gate) = mpsc::channel();
    let worker = Worker::spawn(config("source"), move || {
        gate.recv().unwrap();
        Ok(Fake {
            events: saved,
            ..Fake::default()
        })
    })
    .unwrap();
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
    release.send(()).unwrap();
    let snapshot = wait(&worker, |snapshot| snapshot.finished);
    assert_eq!(
        snapshot.configuration_attempt,
        accepted.load(Ordering::Relaxed) as u64
    );
    assert_eq!(
        events.lock().unwrap().len(),
        accepted.load(Ordering::Relaxed)
    );
    worker.join().unwrap();
}
