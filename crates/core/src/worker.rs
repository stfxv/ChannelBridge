//! UI-facing bounded commands and a control-thread-owned backend.
use crate::{
    CaptureState, Command, Config,
    controller::{Backend, Controller},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub state: CaptureState,
    pub config: Config,
    pub wanted_send: bool,
    pub error: Option<String>,
    pub command_error: Option<String>,
    pub finished: bool,
}
pub struct Worker {
    commands: SyncSender<Command>,
    intent: Arc<AtomicU8>,
    quit: Arc<AtomicBool>,
    snapshot: Arc<Mutex<Snapshot>>,
    thread: Option<JoinHandle<()>>,
}
fn apply_command<B: Backend>(
    controller: &mut Controller<B>,
    command: Command,
    error: &mut Option<String>,
) {
    let clear = matches!(command, Command::Configure(_) | Command::Refresh);
    match controller.command(command) {
        Err(failure) => *error = Some(failure),
        Ok(()) if clear => *error = None,
        Ok(()) => {}
    }
}
impl Worker {
    /// Construct the backend inside its owning thread, allowing thread-confined
    /// HAL registrations. The factory must not touch AppKit.
    pub fn spawn<B: Backend + 'static>(
        config: Config,
        factory: impl FnOnce() -> Result<B, String> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::sync_channel(32);
        let intent = Arc::new(AtomicU8::new(0));
        let quit = Arc::new(AtomicBool::new(false));
        let snapshot = Arc::new(Mutex::new(Snapshot {
            state: CaptureState::Idle,
            config: config.clone(),
            wanted_send: false,
            error: None,
            command_error: None,
            finished: false,
        }));
        let shared = Arc::clone(&snapshot);
        let requested = Arc::clone(&intent);
        let exiting = Arc::clone(&quit);
        let thread = thread::Builder::new()
            .name("channelbridge-control".into())
            .spawn(move || {
                let publish = |value: Snapshot| {
                    *shared.lock().unwrap_or_else(|error| error.into_inner()) = value;
                };
                let backend = match factory() {
                    Ok(backend) => backend,
                    Err(error) => {
                        publish(Snapshot {
                            state: CaptureState::Blocked,
                            config,
                            wanted_send: false,
                            error: Some(error),
                            command_error: None,
                            finished: true,
                        });
                        return;
                    }
                };
                let mut controller = Controller::new(backend, config);
                let start = Instant::now();
                let mut next_tick = Instant::now();
                let mut command_error = None;
                loop {
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(command) => {
                            apply_command(&mut controller, command, &mut command_error);
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    // Finite drain budget: UI bursts cannot starve health checks.
                    for _ in 0..31 {
                        match receiver.try_recv() {
                            Ok(command) => {
                                apply_command(&mut controller, command, &mut command_error);
                            }
                            Err(_) => break,
                        }
                    }
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    match requested.swap(0, Ordering::AcqRel) {
                        1 => {
                            apply_command(&mut controller, Command::Start, &mut command_error);
                        }
                        2 => {
                            apply_command(&mut controller, Command::Stop, &mut command_error);
                        }
                        _ => {}
                    }
                    if Instant::now() >= next_tick {
                        controller.tick(start.elapsed().as_millis().min(u64::MAX as u128) as u64);
                        next_tick = Instant::now() + Duration::from_millis(100);
                    }
                    publish(Snapshot {
                        state: controller.state(),
                        config: controller.config().clone(),
                        wanted_send: controller.wanted_send(),
                        error: controller.error().map(str::to_owned),
                        command_error: command_error.clone(),
                        finished: false,
                    });
                }
                let shutdown_error = controller.command(Command::Quit).err();
                publish(Snapshot {
                    state: controller.state(),
                    config: controller.config().clone(),
                    wanted_send: false,
                    error: shutdown_error.or_else(|| controller.error().map(str::to_owned)),
                    command_error,
                    finished: true,
                });
            })?;
        Ok(Self {
            commands,
            intent,
            quit,
            snapshot,
            thread: Some(thread),
        })
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        if self.quit.load(Ordering::Acquire) || self.snapshot().finished {
            return Err("control worker has stopped".into());
        }
        // Stop and Quit do not depend on free queue capacity. Last send intent
        // supersedes earlier intent while settings retain their queue order.
        match command {
            Command::Start => {
                self.intent.store(1, Ordering::Release);
                Ok(())
            }
            Command::Stop => {
                self.intent.store(2, Ordering::Release);
                Ok(())
            }
            Command::Quit => {
                self.quit.store(true, Ordering::Release);
                Ok(())
            }
            command => self
                .commands
                .try_send(command)
                .map_err(|error| match error {
                    TrySendError::Full(_) => "control command queue is full".into(),
                    TrySendError::Disconnected(_) => "control worker has stopped".into(),
                }),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
    /// Signal without blocking AppKit. Poll snapshot.finished before joining.
    pub fn request_shutdown(&self) {
        self.quit.store(true, Ordering::Release);
    }
    pub fn join(mut self) -> thread::Result<()> {
        self.request_shutdown();
        self.thread.take().expect("worker thread").join()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // The worker still owns sessions while closing. Detaching its handle is
        // preferable to blocking the UI on an unresponsive device teardown.
        self.quit.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{rc::Rc, sync::atomic::AtomicUsize};
    struct LocalBackend {
        _local: Rc<()>,
        drops: Arc<AtomicUsize>,
    }
    impl Drop for LocalBackend {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
    impl Backend for LocalBackend {
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
            Ok(())
        }
        fn healthy(&mut self, _: &(), _: &Config) -> Result<bool, String> {
            Ok(true)
        }
        fn route(&mut self, _: &(), _: &Config, _: bool, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn close(&mut self, _: ()) -> Result<(), String> {
            Ok(())
        }
    }
    #[test]
    fn full_queue_does_not_block_stop_or_shutdown_and_backend_stays_local() {
        let (release, gate) = mpsc::channel();
        let drops = Arc::new(AtomicUsize::new(0));
        let backend_drops = Arc::clone(&drops);
        let parent = thread::current().id();
        let worker = Worker::spawn(
            Config {
                device_uid: "source".into(),
                ..Config::default()
            },
            move || {
                assert_ne!(thread::current().id(), parent);
                gate.recv().unwrap();
                Ok(LocalBackend {
                    _local: Rc::new(()),
                    drops: backend_drops,
                })
            },
        )
        .unwrap();
        worker.send(Command::PermissionChanged(true)).unwrap();
        for _ in 0..31 {
            worker.send(Command::Overview(true)).unwrap();
        }
        assert!(worker.send(Command::Refresh).is_err());
        worker.send(Command::Start).unwrap();
        worker.send(Command::Stop).unwrap();
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.snapshot().state != CaptureState::Previewing {
            assert!(
                Instant::now() < deadline,
                "worker did not finish negotiation"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!worker.snapshot().wanted_send);
        worker.send(Command::Quit).unwrap();
        worker.join().unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
