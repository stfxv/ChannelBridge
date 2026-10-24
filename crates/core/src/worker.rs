//! UI-facing bounded commands and a control-thread-owned backend.
use crate::{
    CaptureState, Command, Config, SystemEvent,
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
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub state: CaptureState,
    pub config: Config,
    pub wanted_send: bool,
    pub error: Option<String>,
    pub command_error: Option<String>,
    /// Completed Configure attempts, including failed saves.
    pub configuration_attempt: u64,
    pub finished: bool,
    pub audio: Option<crate::view::AudioView>,
    pub diagnostics: Option<crate::view::AudioDiagnostics>,
    pub retained_io_contexts: u64,
    pub driver_diagnostics: Option<crate::view::DriverDiagnostics>,
    pub control_history: Arc<[crate::view::ControlEvent]>,
    pub devices: Arc<[crate::view::DeviceChoice]>,
    pub device_error: Option<String>,
}
pub struct Worker {
    commands: SyncSender<Command>,
    intent: Arc<AtomicU8>,
    quit: Arc<AtomicBool>,
    system_events: SystemEvents,
    snapshot: Arc<Mutex<Snapshot>>,
    thread: Option<JoinHandle<()>>,
}
/// Notification callbacks only publish atomics; they never access UI or HAL.
/// The latest sleep/wake state wins if notifications arrive before a worker turn.
/// A coalesced wake still retires the old session, including a missed sleep.
#[derive(Clone)]
pub struct SystemEvents {
    pending: Arc<AtomicU8>,
    quit: Arc<AtomicBool>,
}
impl SystemEvents {
    pub fn notify(&self, event: SystemEvent) {
        if self.quit.load(Ordering::Acquire) {
            return;
        }
        match event {
            SystemEvent::Sleep | SystemEvent::Wake => {
                let state = if event == SystemEvent::Sleep { 1 } else { 2 };
                let _ = self
                    .pending
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                        Some(state | (old & 4))
                    });
            }
            SystemEvent::ScreensWake => {
                self.pending.fetch_or(4, Ordering::Release);
            }
        }
    }
    fn take(&self) -> (Option<SystemEvent>, bool) {
        let pending = self.pending.swap(0, Ordering::AcqRel);
        let state = match pending & 3 {
            1 => Some(SystemEvent::Sleep),
            2 => Some(SystemEvent::Wake),
            _ => None,
        };
        (state, pending & 4 != 0)
    }
}
/// Declared before the backend so it publishes only after stack unwinding has
/// dropped the backend/session owners. Preserve join's panic result as well.
struct WorkerExit {
    snapshot: Arc<Mutex<Snapshot>>,
}
impl Drop for WorkerExit {
    fn drop(&mut self) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !snapshot.finished {
            snapshot.finished = true;
            snapshot.state = CaptureState::Blocked;
            snapshot.wanted_send = false;
            snapshot.audio = None;
            snapshot.diagnostics = None;
            snapshot.driver_diagnostics = None;
            snapshot.error = Some("control worker terminated unexpectedly".into());
        }
    }
}
fn apply_command<B: Backend>(
    controller: &mut Controller<B>,
    command: Command,
    error: &mut Option<String>,
    configuration_attempt: &mut u64,
) {
    let configuring = matches!(command, Command::Configure(_));
    let clear = matches!(command, Command::Configure(_) | Command::Refresh);
    match controller.command(command) {
        Err(failure) => *error = Some(failure),
        Ok(()) if clear => *error = None,
        Ok(()) => {}
    }
    if configuring {
        *configuration_attempt = configuration_attempt.saturating_add(1);
    }
}
fn apply_system_events<B: Backend>(
    events: &SystemEvents,
    controller: &mut Controller<B>,
    error: &mut Option<String>,
    configuration_attempt: &mut u64,
) -> bool {
    let (power, screens_woke) = events.take();
    for event in power
        .into_iter()
        .chain(screens_woke.then_some(SystemEvent::ScreensWake))
    {
        apply_command(
            controller,
            Command::System(event),
            error,
            configuration_attempt,
        );
    }
    power.is_some() || screens_woke
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
        let system_events = SystemEvents {
            pending: Arc::new(AtomicU8::new(0)),
            quit: Arc::clone(&quit),
        };
        let snapshot = Arc::new(Mutex::new(Snapshot {
            state: CaptureState::Idle,
            config: config.clone(),
            wanted_send: false,
            error: None,
            command_error: None,
            configuration_attempt: 0,
            finished: false,
            audio: None,
            diagnostics: None,
            retained_io_contexts: 0,
            driver_diagnostics: None,
            control_history: Arc::from([]),
            devices: Arc::from([]),
            device_error: None,
        }));
        let shared = Arc::clone(&snapshot);
        let requested = Arc::clone(&intent);
        let exiting = Arc::clone(&quit);
        let lifecycle = system_events.clone();
        let thread = thread::Builder::new()
            .name("channelbridge-control".into())
            .spawn(move || {
                let _exit = WorkerExit {
                    snapshot: Arc::clone(&shared),
                };
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
                            configuration_attempt: 0,
                            finished: true,
                            audio: None,
                            diagnostics: None,
                            retained_io_contexts: 0,
                            driver_diagnostics: None,
                            control_history: Arc::from([]),
                            devices: Arc::from([]),
                            device_error: None,
                        });
                        return;
                    }
                };
                let mut controller = Controller::new(backend, config);
                let start = Instant::now();
                let mut next_tick = Instant::now();
                let mut command_error = None;
                let mut configuration_attempt = 0;
                let mut devices = Arc::<[crate::view::DeviceChoice]>::from([]);
                let mut device_error = None;
                let mut next_inventory = Instant::now();
                let mut driver_diagnostics = None;
                loop {
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    if apply_system_events(
                        &lifecycle,
                        &mut controller,
                        &mut command_error,
                        &mut configuration_attempt,
                    ) {
                        next_tick = Instant::now();
                        next_inventory = Instant::now();
                    }
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(command) => {
                            apply_command(
                                &mut controller,
                                command,
                                &mut command_error,
                                &mut configuration_attempt,
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    // Finite drain budget: UI bursts cannot starve health checks.
                    for _ in 0..31 {
                        match receiver.try_recv() {
                            Ok(command) => {
                                apply_command(
                                    &mut controller,
                                    command,
                                    &mut command_error,
                                    &mut configuration_attempt,
                                );
                            }
                            Err(_) => break,
                        }
                    }
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    if apply_system_events(
                        &lifecycle,
                        &mut controller,
                        &mut command_error,
                        &mut configuration_attempt,
                    ) {
                        next_tick = Instant::now();
                        next_inventory = Instant::now();
                    }
                    match requested.swap(0, Ordering::AcqRel) {
                        1 => {
                            apply_command(
                                &mut controller,
                                Command::Start,
                                &mut command_error,
                                &mut configuration_attempt,
                            );
                        }
                        2 => {
                            apply_command(
                                &mut controller,
                                Command::Stop,
                                &mut command_error,
                                &mut configuration_attempt,
                            );
                        }
                        _ => {}
                    }
                    if Instant::now() >= next_tick {
                        controller.tick(start.elapsed().as_millis().min(u64::MAX as u128) as u64);
                        next_tick = Instant::now() + Duration::from_millis(100);
                    }
                    if Instant::now() >= next_inventory {
                        match controller.devices() {
                            Ok(current) => {
                                devices = current.into();
                                device_error = None;
                            }
                            Err(error) => device_error = Some(error),
                        }
                        driver_diagnostics = controller.driver_diagnostics().map(|reading| {
                            crate::view::DriverDiagnostics {
                                observed_worker_ms: start
                                    .elapsed()
                                    .as_millis()
                                    .min(u64::MAX as u128)
                                    as u64,
                                reading,
                            }
                        });
                        next_inventory = Instant::now() + Duration::from_secs(1);
                    }
                    publish(Snapshot {
                        state: controller.state(),
                        config: controller.config().clone(),
                        wanted_send: controller.wanted_send(),
                        error: controller.error().map(str::to_owned),
                        command_error: command_error.clone(),
                        configuration_attempt,
                        finished: false,
                        audio: controller.audio_view(),
                        diagnostics: controller.diagnostics(),
                        retained_io_contexts: controller.retained_io_contexts(),
                        driver_diagnostics: driver_diagnostics.clone(),
                        control_history: controller.history(),
                        devices: Arc::clone(&devices),
                        device_error: device_error.clone(),
                    });
                }
                let shutdown_error = controller.command(Command::Quit).err();
                publish(Snapshot {
                    state: controller.state(),
                    config: controller.config().clone(),
                    wanted_send: false,
                    error: shutdown_error.or_else(|| controller.error().map(str::to_owned)),
                    command_error,
                    configuration_attempt,
                    finished: true,
                    audio: None,
                    diagnostics: None,
                    retained_io_contexts: controller.retained_io_contexts(),
                    driver_diagnostics: None,
                    control_history: controller.history(),
                    devices,
                    device_error,
                });
            })?;
        Ok(Self {
            commands,
            intent,
            quit,
            system_events,
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
            Command::System(event) => {
                self.system_events.notify(event);
                Ok(())
            }
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
    pub fn system_events(&self) -> SystemEvents {
        self.system_events.clone()
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
        panic_on_save: bool,
    }
    impl Drop for LocalBackend {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
    impl Backend for LocalBackend {
        fn retained_io_contexts(&self) -> u64 {
            7
        }
        type Pending = ();
        type Session = ();
        fn diagnostics(&self, _: &()) -> Option<crate::view::AudioDiagnostics> {
            Some(crate::view::AudioDiagnostics {
                capture_written_frames: (1_u64 << 54) + 3,
                output_missing_frames: 7,
                ..Default::default()
            })
        }
        fn save(&mut self, _: &Config) -> Result<(), String> {
            assert!(!self.panic_on_save, "fixture save panic");
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
                    panic_on_save: false,
                })
            },
        )
        .unwrap();
        worker.send(Command::PermissionChanged(true)).unwrap();
        for _ in 0..31 {
            worker.send(Command::Overview(true)).unwrap();
        }
        assert!(worker.send(Command::Refresh).is_err());
        // OS lifecycle must remain deliverable even during a full UI queue.
        worker
            .send(Command::System(crate::SystemEvent::Sleep))
            .unwrap();
        worker
            .send(Command::System(crate::SystemEvent::Wake))
            .unwrap();
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
        assert!(
            worker
                .snapshot()
                .control_history
                .iter()
                .any(|event| event.kind == crate::view::ControlEventKind::SystemWake)
        );
        assert_eq!(
            worker.snapshot().control_history.last().unwrap().kind,
            crate::view::ControlEventKind::SessionOpened
        );
        assert_eq!(worker.snapshot().retained_io_contexts, 7);
        let diagnostics = worker.snapshot().diagnostics.unwrap();
        assert_eq!(diagnostics.capture_written_frames, (1_u64 << 54) + 3);
        let json: serde_json::Value = serde_json::to_value(&diagnostics).unwrap();
        assert_eq!(
            json["capture_written_frames"].as_u64(),
            Some((1_u64 << 54) + 3)
        );
        assert_eq!(diagnostics.output_missing_frames, 7);
        worker.send(Command::PermissionChanged(false)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.snapshot().state != CaptureState::Blocked {
            assert!(
                Instant::now() < deadline,
                "permission revocation not processed"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(worker.snapshot().diagnostics.is_none());
        worker.send(Command::Quit).unwrap();
        let shared_snapshot = Arc::clone(&worker.snapshot);
        worker.join().unwrap();
        assert!(shared_snapshot.lock().unwrap().diagnostics.is_none());
        assert_eq!(shared_snapshot.lock().unwrap().retained_io_contexts, 7);
        assert_eq!(
            shared_snapshot
                .lock()
                .unwrap()
                .control_history
                .last()
                .unwrap()
                .kind,
            crate::view::ControlEventKind::PermissionRevoked
        );
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn panicked_worker_publishes_completion_after_backend_drop() {
        let drops = Arc::new(AtomicUsize::new(0));
        let backend_drops = Arc::clone(&drops);
        let original = Config {
            device_uid: "source".into(),
            ..Config::default()
        };
        let worker = Worker::spawn(original.clone(), move || {
            Ok(LocalBackend {
                _local: Rc::new(()),
                drops: backend_drops,
                panic_on_save: true,
            })
        })
        .unwrap();
        worker
            .send(Command::Configure(Config {
                left: 2,
                ..original.clone()
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.snapshot().finished {
            assert!(
                Instant::now() < deadline,
                "panic did not publish completion"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        let snapshot = worker.snapshot();
        assert_eq!(snapshot.config, original);
        assert_eq!(snapshot.state, CaptureState::Blocked);
        assert!(!snapshot.wanted_send);
        assert!(snapshot.error.is_some());
        assert!(worker.send(Command::Start).is_err());
        assert!(worker.join().is_err());
    }
    #[test]
    fn factory_panic_also_publishes_completion() {
        let worker =
            Worker::spawn::<LocalBackend>(Config::default(), || panic!("fixture factory panic"))
                .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.snapshot().finished {
            assert!(
                Instant::now() < deadline,
                "factory panic did not publish completion"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(worker.snapshot().state, CaptureState::Blocked);
        assert!(worker.join().is_err());
    }
}
