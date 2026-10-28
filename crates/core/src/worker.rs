//! UI-facing bounded commands and a control-thread-owned backend.
use crate::CaptureState;
use crate::Command;
use crate::Config;
use crate::SystemEvent;
use crate::controller::Backend;
use crate::controller::Controller;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::TrySendError;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;
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
    commands: Arc<Mutex<Option<SyncSender<Command>>>>,
    intent: Arc<AtomicU8>,
    quit: Arc<AtomicBool>,
    system_events: SystemEvents,
    snapshot: Arc<Mutex<Snapshot>>,
    completion: Arc<AtomicU8>,
    initial_config: Config,
    thread: Option<JoinHandle<()>>,
}
fn seal(commands: &Mutex<Option<SyncSender<Command>>>, quit: &AtomicBool) -> bool {
    match commands.lock() {
        Ok(mut admission) => {
            quit.store(true, Ordering::Release);
            admission.take().is_some()
        }
        Err(_) => {
            quit.store(true, Ordering::Release);
            false
        }
    }
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
    #[must_use]
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
#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
enum Completion {
    Running = 0,
    Finished = 1,
    Panicked = 2,
}
impl Completion {
    #[must_use]
    fn load(state: &AtomicU8) -> Self {
        match state.load(Ordering::Acquire) {
            0 => Self::Running,
            1 => Self::Finished,
            _ => Self::Panicked,
        }
    }
}
/// Declared before the backend so it publishes only after stack unwinding has
/// dropped the backend/session owners. Preserve join's panic result as well.
struct WorkerExit {
    completion: Arc<AtomicU8>,
}
impl Drop for WorkerExit {
    fn drop(&mut self) {
        let completion = if thread::panicking() {
            Completion::Panicked
        } else {
            Completion::Finished
        };
        self.completion.store(completion as u8, Ordering::Release);
    }
}
fn apply_command<B: Backend>(
    controller: &mut Controller<B>,
    command: Command,
    error: &mut Option<String>,
    configuration_attempt: &mut u64,
) -> bool {
    let previous_uid = controller.config().device_uid.clone();
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
    previous_uid != controller.config().device_uid
}
#[must_use]
fn finish_shutdown<B: Backend>(
    controller: &mut Controller<B>,
    commands: impl Iterator<Item = Command>,
    command_error: &mut Option<String>,
    configuration_attempt: &mut u64,
) -> Option<String> {
    let shutdown_error = controller.command(Command::Quit).err();
    for command in commands {
        if let Command::Configure(config) = command {
            *configuration_attempt = configuration_attempt.saturating_add(1);
            *command_error = controller.persist_configuration(config).err();
        }
    }
    shutdown_error
}
#[must_use]
fn apply_system_events<B: Backend>(
    events: &SystemEvents,
    quit: &AtomicBool,
    controller: &mut Controller<B>,
    error: &mut Option<String>,
    configuration_attempt: &mut u64,
) -> bool {
    let (power, screens_woke) = events.take();
    let pending_events = power
        .into_iter()
        .chain(screens_woke.then_some(SystemEvent::ScreensWake));
    for event in pending_events {
        if quit.load(Ordering::Acquire) {
            break;
        }
        apply_command(
            controller,
            Command::System(event),
            error,
            configuration_attempt,
        );
    }
    power.is_some() || screens_woke
}
#[derive(Default)]
struct InputPresence {
    observed: Option<(String, bool)>,
}
impl InputPresence {
    #[must_use]
    fn observe(&mut self, uid: &str, inventory: &crate::view::DeviceInventory) -> bool {
        if self
            .observed
            .as_ref()
            .is_some_and(|(previous, _)| previous != uid)
        {
            self.observed = None;
        }
        if uid.is_empty() {
            self.observed = None;
            return false;
        }
        let present = inventory.choices.iter().any(|device| device.uid == uid);
        if !present
            && matches!(
                inventory.completeness,
                crate::view::InventoryCompleteness::Partial { .. }
            )
        {
            return false;
        }
        let reconnected = self
            .observed
            .as_ref()
            .is_some_and(|(previous_uid, previous_present)| {
                previous_uid == uid && !previous_present && present && !uid.is_empty()
            });
        self.observed = Some((uid.to_owned(), present));
        reconnected
    }
}
fn apply_queued_command<B: Backend>(
    controller: &mut Controller<B>,
    command: Command,
    error: &mut Option<String>,
    configuration_attempt: &mut u64,
    presence: &mut InputPresence,
) {
    if apply_command(controller, command, error, configuration_attempt) {
        presence.observed = None;
    }
}
impl Worker {
    /// Construct the backend inside its owning thread, allowing thread-confined
    /// HAL registrations. The factory must not touch AppKit.
    pub fn spawn<B: Backend + 'static>(
        config: Config,
        factory: impl FnOnce() -> Result<B, String> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(32);
        let commands = Arc::new(Mutex::new(Some(sender)));
        let intent = Arc::new(AtomicU8::new(0));
        let quit = Arc::new(AtomicBool::new(false));
        let completion = Arc::new(AtomicU8::new(Completion::Running as u8));
        let thread_completion = Arc::clone(&completion);
        let initial_config = config.clone();
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
        let admission = Arc::clone(&commands);
        let thread = thread::Builder::new()
            .name("channelbridge-control".into())
            .spawn(move || {
                let _exit = WorkerExit {
                    completion: thread_completion,
                };
                let publish = |value: Snapshot| match shared.lock() {
                    Ok(mut snapshot) => *snapshot = value,
                    Err(_) => exiting.store(true, Ordering::Release),
                };
                let backend = match factory() {
                    Ok(backend) => backend,
                    Err(error) => {
                        seal(&admission, &exiting);
                        let configuration_attempt = receiver
                            .try_iter()
                            .filter(|command| matches!(command, Command::Configure(_)))
                            .count() as u64;
                        publish(Snapshot {
                            state: CaptureState::Blocked,
                            config,
                            wanted_send: false,
                            error: Some(error.clone()),
                            command_error: (configuration_attempt != 0).then_some(error),
                            configuration_attempt,
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
                let mut input_presence = InputPresence::default();
                let mut driver_diagnostics = None;
                let mut held_command = None;
                'running: loop {
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    if apply_system_events(
                        &lifecycle,
                        &exiting,
                        &mut controller,
                        &mut command_error,
                        &mut configuration_attempt,
                    ) {
                        next_tick = Instant::now();
                        next_inventory = Instant::now();
                    }
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(command) => {
                            if exiting.load(Ordering::Acquire) {
                                held_command = Some(command);
                                break;
                            }
                            apply_queued_command(
                                &mut controller,
                                command,
                                &mut command_error,
                                &mut configuration_attempt,
                                &mut input_presence,
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    // Finite drain budget: UI bursts cannot starve health checks.
                    for _ in 0..31 {
                        if exiting.load(Ordering::Acquire) {
                            break 'running;
                        }
                        match receiver.try_recv() {
                            Ok(command) => {
                                if exiting.load(Ordering::Acquire) {
                                    held_command = Some(command);
                                    break 'running;
                                }
                                apply_queued_command(
                                    &mut controller,
                                    command,
                                    &mut command_error,
                                    &mut configuration_attempt,
                                    &mut input_presence,
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
                        &exiting,
                        &mut controller,
                        &mut command_error,
                        &mut configuration_attempt,
                    ) {
                        next_tick = Instant::now();
                        next_inventory = Instant::now();
                    }
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    if Instant::now() >= next_inventory {
                        let inventory = controller.devices();
                        if exiting.load(Ordering::Acquire) {
                            break;
                        }
                        match inventory {
                            Ok(current) => {
                                if input_presence.observe(&controller.config().device_uid, &current)
                                {
                                    apply_command(
                                        &mut controller,
                                        Command::InputReconnected,
                                        &mut command_error,
                                        &mut configuration_attempt,
                                    );
                                    next_tick = Instant::now();
                                }
                                devices = current.choices.into();
                                device_error = match current.completeness {
                                    crate::view::InventoryCompleteness::Complete => None,
                                    crate::view::InventoryCompleteness::Partial { warning } => {
                                        Some(warning)
                                    }
                                };
                            }
                            Err(error) => device_error = Some(error),
                        }
                        if exiting.load(Ordering::Acquire) {
                            break;
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
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    let manual_intent = requested.swap(0, Ordering::AcqRel);
                    match manual_intent {
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
                    if exiting.load(Ordering::Acquire) {
                        break;
                    }
                    if Instant::now() >= next_tick {
                        controller.tick_with_manual_intent(
                            start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                            manual_intent != 0,
                        );
                        next_tick = Instant::now() + Duration::from_millis(100);
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
                seal(&admission, &exiting);
                // Sealing waits for accepted sends, even if poison retains the sender.
                let shutdown_error = finish_shutdown(
                    &mut controller,
                    held_command.into_iter().chain(receiver.try_iter()),
                    &mut command_error,
                    &mut configuration_attempt,
                );
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
            completion,
            initial_config,
            thread: Some(thread),
        })
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        if self.snapshot().finished {
            return Err("control worker has stopped".into());
        }
        if matches!(command, Command::Quit) {
            return if seal(&self.commands, &self.quit) {
                Ok(())
            } else {
                Err("control worker has stopped".into())
            };
        }
        let admission = match self.commands.lock() {
            Ok(admission) => admission,
            Err(_) => {
                self.quit.store(true, Ordering::Release);
                return Err("control worker command admission is unavailable".into());
            }
        };
        if self.quit.load(Ordering::Acquire) {
            return Err("control worker has stopped".into());
        }
        let sender = admission.as_ref().ok_or("control worker has stopped")?;
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
            command => sender.try_send(command).map_err(|error| match error {
                TrySendError::Full(_) => "control command queue is full".into(),
                TrySendError::Disconnected(_) => "control worker has stopped".into(),
            }),
        }
    }
    #[must_use]
    pub fn system_events(&self) -> SystemEvents {
        self.system_events.clone()
    }
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let completion = Completion::load(&self.completion);
        let completed = completion != Completion::Running;
        let mut value = match self.snapshot.lock() {
            Ok(snapshot) => {
                let mut value = snapshot.clone();
                // Stored finished marks normal final publication, not owner destruction.
                if completion == Completion::Panicked || (completed && !value.finished) {
                    value.state = CaptureState::Blocked;
                    value.wanted_send = false;
                    value.audio = None;
                    value.diagnostics = None;
                    value.driver_diagnostics = None;
                    value.error = Some("control worker terminated unexpectedly".into());
                }
                value
            }
            Err(_) => {
                self.quit.store(true, Ordering::Release);
                Snapshot {
                    state: CaptureState::Blocked,
                    config: self.initial_config.clone(),
                    wanted_send: false,
                    error: Some("control worker snapshot is unavailable".into()),
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
                }
            }
        };
        value.finished = completed;
        value
    }
    /// Signal without blocking AppKit. Poll snapshot.finished before joining.
    pub fn request_shutdown(&self) {
        seal(&self.commands, &self.quit);
    }
    pub fn join(mut self) -> thread::Result<()> {
        self.request_shutdown();
        self.thread
            .take()
            .expect("Worker owns its join handle until this consuming join call")
            .join()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // The worker still owns sessions while closing. Detaching its handle is
        // preferable to blocking the UI on an unresponsive device teardown.
        seal(&self.commands, &self.quit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn complete(choices: &[crate::view::DeviceChoice]) -> crate::view::DeviceInventory {
        crate::view::DeviceInventory {
            choices: choices.to_vec(),
            completeness: crate::view::InventoryCompleteness::Complete,
        }
    }
    use std::rc::Rc;
    use std::sync::atomic::AtomicUsize;
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
    struct ExitBackend {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        saves: Arc<Mutex<Vec<String>>>,
        panic_on_save: bool,
        panic_on_drop: bool,
    }
    impl Drop for ExitBackend {
        fn drop(&mut self) {
            self.entered
                .send(())
                .expect("test observes backend destruction");
            self.release
                .recv()
                .expect("test releases backend destruction");
            assert!(!self.panic_on_drop, "fixture backend drop panic");
        }
    }
    impl Backend for ExitBackend {
        type Pending = ();
        type Session = ();
        fn save(&mut self, config: &Config) -> Result<(), String> {
            assert!(!self.panic_on_save, "fixture save panic");
            self.saves
                .lock()
                .expect("save log remains healthy")
                .push(config.device_uid.clone());
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
    fn poison<T: Send>(mutex: &Mutex<T>, corrupt: impl FnOnce(&mut T) + Send) {
        assert!(
            thread::scope(|scope| {
                scope
                    .spawn(|| {
                        let mut guard = mutex.lock().expect("fixture mutex starts healthy");
                        corrupt(&mut guard);
                        panic!("fixture poison");
                    })
                    .join()
            })
            .is_err()
        );
    }
    fn wait_finished(worker: &Worker) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.snapshot().finished {
            assert!(
                Instant::now() < deadline,
                "worker teardown did not complete"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn normal_completion_waits_for_backend_drop_including_drop_panic() {
        for panic_on_drop in [false, true] {
            let (entered, observed) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            let worker = Worker::spawn(Config::default(), move || {
                Ok(ExitBackend {
                    entered,
                    release: gate,
                    saves: Arc::new(Mutex::new(Vec::new())),
                    panic_on_save: false,
                    panic_on_drop,
                })
            })
            .expect("worker starts");
            worker.request_shutdown();
            observed
                .recv_timeout(Duration::from_secs(2))
                .expect("backend drop starts");
            let before = worker.snapshot();
            release.send(()).expect("backend drop is waiting");
            wait_finished(&worker);
            assert!(
                !before.finished,
                "normal final publication precedes backend destruction"
            );
            if panic_on_drop {
                let after = worker.snapshot();
                assert_eq!(after.state, CaptureState::Blocked);
                assert_eq!(
                    after.error.as_deref(),
                    Some("control worker terminated unexpectedly")
                );
            }
            assert_eq!(worker.join().is_err(), panic_on_drop);
        }
    }
    #[test]
    fn poisoned_snapshot_is_independent_and_completion_survives_unwind() {
        for (panic_on_save, observe_poison) in [(false, false), (false, true), (true, false)] {
            let (factory_entered, factory_observed) = mpsc::channel();
            let (factory_release, factory_gate) = mpsc::channel();
            let (entered, observed) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            let original = Config {
                device_uid: "original".into(),
                ..Config::default()
            };
            let worker = Worker::spawn(original.clone(), move || {
                factory_entered.send(()).expect("factory barrier observed");
                factory_gate.recv().expect("factory released");
                Ok(ExitBackend {
                    entered,
                    release: gate,
                    saves: Arc::new(Mutex::new(Vec::new())),
                    panic_on_save,
                    panic_on_drop: false,
                })
            })
            .expect("worker starts");
            factory_observed
                .recv_timeout(Duration::from_secs(2))
                .expect("factory waits");
            if panic_on_save {
                worker
                    .send(Command::Configure(original.clone()))
                    .expect("configure accepted before poison");
            }
            poison(&worker.snapshot, |value| {
                value.config.device_uid = "corrupt".into();
                value.finished = true;
                value.configuration_attempt = 99;
                value.retained_io_contexts = 99;
            });
            // In the panic case, let the queued command unwind before observing poison.
            factory_release.send(()).expect("factory waits for release");
            if observe_poison {
                let _ = worker.snapshot();
                assert!(
                    worker.send(Command::Start).is_err(),
                    "snapshot failure seals healthy admission through quit"
                );
            }
            observed
                .recv_timeout(Duration::from_secs(2))
                .expect("backend teardown starts");
            let before = worker.snapshot();
            release.send(()).expect("backend teardown is gated");
            wait_finished(&worker);
            assert_eq!(before.config, original);
            assert_eq!(before.state, CaptureState::Blocked);
            assert!(!before.finished);
            assert!(!before.wanted_send);
            assert!(
                before.audio.is_none()
                    && before.diagnostics.is_none()
                    && before.driver_diagnostics.is_none()
            );
            assert_eq!(before.configuration_attempt, 0);
            assert_eq!(before.retained_io_contexts, 0);
            assert!(before.control_history.is_empty() && before.devices.is_empty());
            assert!(
                before
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("unavailable"))
            );
            assert_eq!(worker.join().is_err(), panic_on_save);
        }
    }
    #[test]
    fn unexpected_completion_clears_session_readings_but_preserves_known_history() {
        let (factory_release, factory_gate) = mpsc::channel();
        let (entered, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let worker = Worker::spawn(
            Config {
                device_uid: "source".into(),
                ..Config::default()
            },
            move || {
                factory_gate
                    .recv()
                    .expect("factory remains gated until snapshot seeded");
                Ok(ExitBackend {
                    entered,
                    release: gate,
                    saves: Arc::new(Mutex::new(Vec::new())),
                    panic_on_save: true,
                    panic_on_drop: false,
                })
            },
        )
        .expect("worker starts");
        worker
            .send(Command::Configure(Config {
                device_uid: "source".into(),
                ..Config::default()
            }))
            .expect("panic-triggering command accepted");
        {
            let mut snapshot = worker.snapshot.lock().expect("snapshot remains healthy");
            snapshot.wanted_send = true;
            snapshot.retained_io_contexts = 7;
            snapshot.configuration_attempt = 3;
            snapshot.command_error = Some("prior command failure".into());
            snapshot.audio = Some(crate::view::AudioView {
                physical_inputs: Vec::new(),
                virtual_rate: 48000,
                virtual_bits: 32,
                levels: Vec::new(),
            });
            snapshot.diagnostics = Some(crate::view::AudioDiagnostics::default());
        }
        factory_release.send(()).expect("factory waits for seed");
        observed
            .recv_timeout(Duration::from_secs(2))
            .expect("save panic reaches backend drop");
        let before = worker.snapshot();
        release.send(()).expect("backend drop is gated");
        wait_finished(&worker);
        let after = worker.snapshot();
        assert!(!before.finished);
        assert!(before.audio.is_some());
        assert_eq!(after.state, CaptureState::Blocked);
        assert!(!after.wanted_send);
        assert!(
            after.audio.is_none()
                && after.diagnostics.is_none()
                && after.driver_diagnostics.is_none()
        );
        assert_eq!(after.retained_io_contexts, 7);
        assert_eq!(after.configuration_attempt, 3);
        assert_eq!(
            after.command_error.as_deref(),
            Some("prior command failure")
        );
        assert_eq!(
            after.error.as_deref(),
            Some("control worker terminated unexpectedly")
        );
        assert!(worker.join().is_err());
    }
    #[test]
    fn poisoned_admission_drains_accepted_prefix_even_with_retained_sender() {
        for factory_fails in [false, true] {
            let (release, gate) = mpsc::channel();
            let saves = Arc::new(Mutex::new(Vec::new()));
            let backend_saves = Arc::clone(&saves);
            let (entered, observed) = mpsc::channel();
            let (drop_release, drop_gate) = mpsc::channel();
            let worker = Worker::spawn(Config::default(), move || {
                gate.recv()
                    .expect("factory released after admission poison");
                if factory_fails {
                    return Err("fixture factory error".into());
                }
                Ok(ExitBackend {
                    entered,
                    release: drop_gate,
                    saves: backend_saves,
                    panic_on_save: false,
                    panic_on_drop: false,
                })
            })
            .expect("worker starts");
            for uid in ["first", "second"] {
                worker
                    .send(Command::Configure(Config {
                        device_uid: uid.into(),
                        ..Config::default()
                    }))
                    .expect("queued configure accepted");
            }
            poison(&worker.commands, |_| {});
            let rejected = worker.send(Command::Configure(Config {
                device_uid: "late".into(),
                ..Config::default()
            }));
            worker.request_shutdown();
            release.send(()).expect("factory waits");
            if !factory_fails {
                observed
                    .recv_timeout(Duration::from_secs(2))
                    .expect("finite drain reaches backend drop");
                drop_release.send(()).expect("backend drop waits");
            }
            wait_finished(&worker);
            assert!(rejected.is_err(), "poisoned admission must reject commands");
            assert_eq!(worker.snapshot().configuration_attempt, 2);
            if !factory_fails {
                assert_eq!(
                    *saves.lock().expect("save log remains healthy"),
                    ["first", "second"]
                );
            }
            worker
                .join()
                .expect("poison refusal permits orderly return");
        }
    }
    #[test]
    fn shutdown_includes_the_command_held_outside_the_receiver_before_its_fifo_tail() {
        let mut controller = Controller::new(
            LocalBackend {
                _local: Rc::new(()),
                drops: Arc::new(AtomicUsize::new(0)),
                panic_on_save: false,
            },
            Config {
                device_uid: "source".into(),
                ..Config::default()
            },
        );
        let (sender, receiver) = mpsc::sync_channel(2);
        for uid in ["held", "queued"] {
            sender
                .send(Command::Configure(Config {
                    device_uid: uid.into(),
                    ..Config::default()
                }))
                .expect(
                    "two commands fit the two-slot fixture channel while its receiver is alive",
                );
        }
        let held = receiver
            .try_recv()
            .expect("the fixture just queued two commands without another receiver");
        drop(sender);
        let mut error = Some("prior error".into());
        let mut attempts = 0;
        assert_eq!(
            super::finish_shutdown(
                &mut controller,
                std::iter::once(held).chain(receiver.iter()),
                &mut error,
                &mut attempts,
            ),
            None
        );
        assert_eq!(controller.config().device_uid, "queued");
        assert_eq!(attempts, 2);
        assert_eq!(error, None);
        assert!(!controller.wanted_send());
        assert!(controller.command(Command::Start).is_err());
        assert!(
            controller
                .command(Command::Configure(Config {
                    device_uid: "late".into(),
                    ..Config::default()
                }))
                .is_err()
        );
        assert_eq!(controller.config().device_uid, "queued");
    }
    #[test]
    fn partial_inventory_preserves_definite_evidence_and_positive_return_is_usable() {
        let device = crate::view::DeviceChoice {
            uid: "source".into(),
            name: "Input".into(),
            channels: 2,
            rate: 48000.0,
            physical_inputs: Vec::new(),
            format_error: Some("format unavailable".into()),
        };
        let partial_missing = crate::view::DeviceInventory {
            choices: Vec::new(),
            completeness: crate::view::InventoryCompleteness::Partial {
                warning: "entry failed".into(),
            },
        };
        let partial_present = crate::view::DeviceInventory {
            choices: vec![device.clone()],
            ..partial_missing.clone()
        };
        let mut presence = InputPresence::default();
        assert!(!presence.observe("source", &complete(std::slice::from_ref(&device))));
        assert!(!presence.observe("source", &partial_missing));
        assert_eq!(presence.observed, Some(("source".into(), true)));
        assert!(!presence.observe("source", &complete(std::slice::from_ref(&device))));
        assert!(!presence.observe("source", &complete(&[])));
        assert!(!presence.observe("source", &partial_missing));
        assert_eq!(presence.observed, Some(("source".into(), false)));
        assert!(presence.observe("source", &partial_present));
        assert!(!presence.observe("other", &partial_missing));
        assert_eq!(presence.observed, None);
        assert!(!presence.observe("source", &partial_present));
        assert!(!presence.observe("", &partial_missing));
        assert_eq!(presence.observed, None);
    }
    #[test]
    fn manual_stop_arriving_during_return_inventory_wins_before_open() {
        struct PresenceBackend {
            present: Arc<AtomicBool>,
            inventory_count: Arc<AtomicUsize>,
            entered: mpsc::Sender<()>,
            release: Option<mpsc::Receiver<()>>,
            sends: Arc<Mutex<Vec<bool>>>,
        }
        impl Backend for PresenceBackend {
            type Pending = ();
            type Session = ();
            fn devices(&mut self) -> Result<crate::view::DeviceInventory, String> {
                self.inventory_count.fetch_add(1, Ordering::Release);
                if !self.present.load(Ordering::Acquire) {
                    return Ok(crate::view::DeviceInventory::default());
                }
                if let Some(release) = self.release.take() {
                    self.entered.send(()).expect("test retains the inventory-entry receiver while waiting for this notification");
                    release.recv().expect(
                        "test releases inventory only after queuing the competing manual stop",
                    );
                }
                Ok(complete(&[crate::view::DeviceChoice {
                    uid: "source".into(),
                    name: "Input".into(),
                    channels: 2,
                    rate: 48000.0,
                    physical_inputs: Vec::new(),
                    format_error: None,
                }]))
            }
            fn save(&mut self, _: &Config) -> Result<(), String> {
                Ok(())
            }
            fn negotiate(&mut self, _: &Config) -> Result<(), String> {
                if self.present.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    Err("absent".into())
                }
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
            fn route(&mut self, _: &(), _: &Config, sending: bool, _: bool) -> Result<(), String> {
                self.sends
                    .lock()
                    .expect("fixture threads do not panic while holding this mutex")
                    .push(sending);
                Ok(())
            }
            fn close(&mut self, _: ()) -> Result<(), String> {
                Ok(())
            }
        }
        let present = Arc::new(AtomicBool::new(false));
        let inventory_count = Arc::new(AtomicUsize::new(0));
        let sends = Arc::new(Mutex::new(Vec::new()));
        let (entered, entry) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let backend = PresenceBackend {
            present: Arc::clone(&present),
            inventory_count: Arc::clone(&inventory_count),
            entered,
            release: Some(gate),
            sends: Arc::clone(&sends),
        };
        let worker = Worker::spawn(
            Config {
                device_uid: "source".into(),
                auto_send_on_resume: true,
                ..Config::default()
            },
            move || Ok(backend),
        )
        .expect("fixture worker thread can be created with its fake backend");
        worker
            .send(Command::PermissionChanged(true))
            .expect("running worker has room for the initial permission command");
        let deadline = Instant::now() + Duration::from_secs(2);
        while inventory_count.load(Ordering::Acquire) == 0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!worker.snapshot().wanted_send);
        present.store(true, Ordering::Release);
        entry
            .recv_timeout(Duration::from_secs(2))
            .expect("present-device inventory signals entry before waiting on its release gate");
        worker
            .send(Command::Stop)
            .expect("manual stop bypasses the UI queue while worker admission remains open");
        release
            .send(())
            .expect("inventory callback retains its receiver while waiting for the manual stop");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = worker.snapshot();
            if snapshot.state == CaptureState::Previewing {
                assert!(!snapshot.wanted_send);
                assert!(snapshot.control_history.iter().any(
                    |event| event.kind == crate::view::ControlEventKind::AutomaticSendRequested
                ));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "return did not restore preview after manual stop"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            *sends
                .lock()
                .expect("fixture threads do not panic while holding this mutex"),
            [false],
            "no sending route may open before same-turn Stop"
        );
        worker
            .join()
            .expect("fixture worker completes without panic");
    }

    #[test]
    fn queued_selection_changes_reset_absence_even_when_selection_returns_between_polls() {
        let mut controller = Controller::new(
            LocalBackend {
                _local: Rc::new(()),
                drops: Arc::new(AtomicUsize::new(0)),
                panic_on_save: false,
            },
            Config {
                device_uid: "source".into(),
                ..Config::default()
            },
        );
        let mut presence = InputPresence::default();
        let _ = presence.observe("source", &complete(&[]));
        let mut error = None;
        let mut attempt = 0;
        for uid in ["other", "source"] {
            let config = Config {
                device_uid: uid.into(),
                ..controller.config().clone()
            };
            super::apply_queued_command(
                &mut controller,
                Command::Configure(config),
                &mut error,
                &mut attempt,
                &mut presence,
            );
        }
        assert!(presence.observed.is_none());
        let _ = presence.observe("source", &complete(&[]));
        let config = Config {
            auto_send_on_resume: true,
            ..controller.config().clone()
        };
        super::apply_queued_command(
            &mut controller,
            Command::Configure(config),
            &mut error,
            &mut attempt,
            &mut presence,
        );
        assert_eq!(
            presence.observed,
            Some(("source".into(), false)),
            "option changes preserve presence tracking"
        );
        let config = Config {
            left: 0,
            device_uid: "invalid".into(),
            ..controller.config().clone()
        };
        super::apply_queued_command(
            &mut controller,
            Command::Configure(config),
            &mut error,
            &mut attempt,
            &mut presence,
        );
        assert!(error.is_some());
        assert_eq!(presence.observed, Some(("source".into(), false)));
    }

    #[test]
    fn input_presence_requires_a_real_return_of_the_same_selected_uid() {
        let device = crate::view::DeviceChoice {
            uid: "source".into(),
            name: "Input".into(),
            channels: 2,
            rate: 48000.0,
            physical_inputs: Vec::new(),
            format_error: None,
        };
        let mut presence = InputPresence::default();
        assert!(
            !presence.observe("source", &complete(std::slice::from_ref(&device))),
            "startup present is not a reconnect"
        );
        assert!(!presence.observe("source", &complete(&[])));
        assert!(presence.observe("source", &complete(std::slice::from_ref(&device))));
        assert!(!presence.observe("source", &complete(std::slice::from_ref(&device))));
        assert!(
            !presence.observe("other", &complete(&[])),
            "selection changes establish a baseline"
        );
        assert!(!presence.observe("source", &complete(std::slice::from_ref(&device))));
        let mut absent_at_startup = InputPresence::default();
        assert!(!absent_at_startup.observe("source", &complete(&[])));
        assert!(absent_at_startup.observe("source", &complete(&[device])));
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
                gate.recv()
                    .expect("test releases backend creation after filling the command queue");
                Ok(LocalBackend {
                    _local: Rc::new(()),
                    drops: backend_drops,
                    panic_on_save: false,
                })
            },
        )
        .expect("fixture worker thread can be created with its local fake backend");
        worker
            .send(Command::PermissionChanged(true))
            .expect("initial permission command fits the empty 32-slot queue");
        for _ in 0..31 {
            worker
                .send(Command::Overview(true))
                .expect("31 overview commands fill the remaining 31 queue slots");
        }
        assert!(worker.send(Command::Refresh).is_err());
        // OS lifecycle must remain deliverable even during a full UI queue.
        worker
            .send(Command::System(crate::SystemEvent::Sleep))
            .expect("sleep uses reserved lifecycle admission even when the UI queue is full");
        worker
            .send(Command::System(crate::SystemEvent::Wake))
            .expect("wake uses reserved lifecycle admission even when the UI queue is full");
        worker
            .send(Command::Start)
            .expect("manual start bypasses the full UI queue while admission is open");
        worker
            .send(Command::Stop)
            .expect("manual stop bypasses the full UI queue while admission is open");
        release
            .send(())
            .expect("backend factory retains the gate receiver while queue behavior is checked");
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
            worker
                .snapshot()
                .control_history
                .last()
                .expect("opening the fake session records a session-opened history event")
                .kind,
            crate::view::ControlEventKind::SessionOpened
        );
        assert_eq!(worker.snapshot().retained_io_contexts, 7);
        let diagnostics = worker
            .snapshot()
            .diagnostics
            .expect("previewing fake session publishes its diagnostic counters");
        assert_eq!(diagnostics.capture_written_frames, (1_u64 << 54) + 3);
        let json: serde_json::Value = serde_json::to_value(&diagnostics)
            .expect("integer diagnostic counters have a JSON representation");
        assert_eq!(
            json["capture_written_frames"].as_u64(),
            Some((1_u64 << 54) + 3)
        );
        assert_eq!(diagnostics.output_missing_frames, 7);
        worker
            .send(Command::PermissionChanged(false))
            .expect("permission revocation uses reserved admission while the worker is running");
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.snapshot().state != CaptureState::Blocked {
            assert!(
                Instant::now() < deadline,
                "permission revocation not processed"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(worker.snapshot().diagnostics.is_none());
        worker
            .send(Command::Quit)
            .expect("quit seals running worker admission even when its queue is full");
        let shared_snapshot = Arc::clone(&worker.snapshot);
        worker
            .join()
            .expect("fixture worker completes without panic");
        assert!(
            shared_snapshot
                .lock()
                .expect("fixture threads do not panic while holding this mutex")
                .diagnostics
                .is_none()
        );
        assert_eq!(
            shared_snapshot
                .lock()
                .expect("fixture threads do not panic while holding this mutex")
                .retained_io_contexts,
            7
        );
        assert_eq!(
            shared_snapshot
                .lock()
                .expect("fixture threads do not panic while holding this mutex")
                .control_history
                .last()
                .expect("permission revocation records an event before shutdown")
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
        .expect("fixture worker starts before its save hook intentionally panics");
        worker
            .send(Command::Configure(Config {
                left: 2,
                ..original.clone()
            }))
            .expect("panic-triggering configure command fits the initially empty queue");
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
                .expect("thread creation succeeds before the fixture factory intentionally panics");
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
