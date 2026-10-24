//! Control-thread state machine. Backend calls must not block audio callbacks.
use crate::{CaptureState, Command, Config};

pub trait Backend {
    fn retained_io_contexts(&self) -> u64 {
        0
    }
    fn driver_diagnostics(&self) -> Option<crate::view::DriverReading> {
        None
    }
    fn devices(&mut self) -> Result<crate::view::DeviceInventory, String> {
        Ok(crate::view::DeviceInventory::default())
    }
    type Pending;
    type Session;
    fn requested_rate(&self, _pending: &Self::Pending) -> Option<u32> {
        None
    }
    fn diagnostics(&self, _session: &Self::Session) -> Option<crate::view::AudioDiagnostics> {
        None
    }
    fn audio_view(
        &self,
        _session: &Self::Session,
        _config: &Config,
        _overview: bool,
    ) -> Option<crate::view::AudioView> {
        None
    }
    fn save(&mut self, config: &Config) -> Result<(), String>;
    fn negotiate(&mut self, config: &Config) -> Result<Self::Pending, String>;
    fn ready(&mut self, pending: &Self::Pending) -> Result<bool, String>;
    fn open(&mut self, config: &Config) -> Result<Self::Session, String>;
    fn healthy(&mut self, session: &Self::Session, config: &Config) -> Result<bool, String>;
    fn route(
        &mut self,
        session: &Self::Session,
        config: &Config,
        sending: bool,
        overview: bool,
    ) -> Result<(), String>;
    fn open_failure_requires_stop(&self) -> bool {
        false
    }
    fn close(&mut self, session: Self::Session) -> Result<(), String>;
}
/// `now_ms` is monotonic worker time, not a wall clock. Only the worker owns
/// sessions. User send intent survives reconnects and is independent of phase.
pub struct Controller<B: Backend> {
    backend: B,
    config: Config,
    pending: Option<B::Pending>,
    session: Option<B::Session>,
    wanted_send: bool,
    route_pending: bool,
    overview: bool,
    permission: bool,
    quitting: bool,
    suspended: bool,
    teardown_failed: bool,
    retry_at: u64,
    retry_delay: u64,
    state: CaptureState,
    error: Option<String>,
    history: std::sync::Arc<[crate::view::ControlEvent]>,
}
impl<B: Backend> Controller<B> {
    pub fn new(backend: B, config: Config) -> Self {
        Self {
            backend,
            config,
            pending: None,
            session: None,
            wanted_send: false,
            route_pending: false,
            overview: false,
            permission: false,
            quitting: false,
            suspended: false,
            teardown_failed: false,
            retry_at: 0,
            retry_delay: 1000,
            state: CaptureState::Idle,
            error: None,
            history: std::sync::Arc::from([]),
        }
    }
    pub fn devices(&mut self) -> Result<crate::view::DeviceInventory, String> {
        self.backend.devices()
    }
    pub fn history(&self) -> std::sync::Arc<[crate::view::ControlEvent]> {
        std::sync::Arc::clone(&self.history)
    }
    fn record(
        &mut self,
        worker_ms: Option<u64>,
        kind: crate::view::ControlEventKind,
        detail: Option<&str>,
    ) {
        let detail = detail.map(|text| {
            let mut end = text.len().min(256);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text[..end].to_owned()
        });
        let mut events = Vec::with_capacity(32);
        events.extend(
            self.history
                .iter()
                .skip(self.history.len().saturating_sub(31))
                .cloned(),
        );
        events.push(crate::view::ControlEvent {
            worker_ms,
            kind,
            detail,
            requested_rate_hz: self
                .pending
                .as_ref()
                .and_then(|pending| self.backend.requested_rate(pending)),
        });
        self.history = events.into();
    }
    pub fn driver_diagnostics(&self) -> Option<crate::view::DriverReading> {
        self.backend.driver_diagnostics()
    }
    pub fn retained_io_contexts(&self) -> u64 {
        self.backend.retained_io_contexts()
    }
    pub fn audio_view(&self) -> Option<crate::view::AudioView> {
        self.session.as_ref().and_then(|session| {
            self.backend
                .audio_view(session, &self.config, self.overview)
        })
    }
    pub fn state(&self) -> CaptureState {
        self.state
    }
    pub fn diagnostics(&self) -> Option<crate::view::AudioDiagnostics> {
        self.session
            .as_ref()
            .and_then(|session| self.backend.diagnostics(session))
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn wanted_send(&self) -> bool {
        self.wanted_send
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    fn disconnect(&mut self) -> Result<(), String> {
        self.pending = None;
        self.route_pending = false;
        if let Some(session) = self.session.take() {
            if let Err(error) = self.backend.close(session) {
                self.record(
                    None,
                    crate::view::ControlEventKind::TeardownFailed,
                    Some(&error),
                );
                self.teardown_failed = true;
                self.state = CaptureState::Blocked;
                self.error = Some(error.clone());
                return Err(error);
            }
        }
        Ok(())
    }
    fn automatic_send(&mut self, reason: &str) {
        if self.config.auto_send_on_resume {
            self.wanted_send = true;
            self.record(
                None,
                crate::view::ControlEventKind::AutomaticSendRequested,
                Some(reason),
            );
        }
    }
    pub(crate) fn persist_configuration(&mut self, config: Config) -> Result<bool, String> {
        config
            .validate_selection()
            .map_err(|error| format!("{error:?}"))?;
        self.backend.save(&config)?;
        let changed_device = self.config.device_uid != config.device_uid;
        self.config = config;
        Ok(changed_device)
    }
    pub fn command(&mut self, command: Command) -> Result<(), String> {
        if self.quitting {
            return Err("worker is shutting down".into());
        }
        match command {
            Command::Start => {
                self.wanted_send = true;
                self.record(None, crate::view::ControlEventKind::SendStarted, None);
            }
            Command::Stop => {
                self.wanted_send = false;
                self.record(None, crate::view::ControlEventKind::SendStopped, None);
            }
            Command::Quit => {
                self.quitting = true;
                self.wanted_send = false;
                self.disconnect()?;
                self.state = CaptureState::Idle;
                return Ok(());
            }
            Command::Overview(value) => self.overview = value,
            Command::System(event) => {
                use crate::{SystemEvent, view::ControlEventKind};
                match event {
                    SystemEvent::Sleep => {
                        if !self.suspended {
                            self.record(None, ControlEventKind::SystemSleep, None);
                            self.suspended = true;
                            self.disconnect()?;
                        }
                        if !self.teardown_failed {
                            self.error = None;
                            self.state = CaptureState::Suspended;
                        }
                    }
                    SystemEvent::Wake => {
                        self.record(None, ControlEventKind::SystemWake, None);
                        self.suspended = false;
                        self.automatic_send("wake");
                        // Also retire a stale session when will-sleep was missed.
                        // A wake never clears an unsafe teardown failure.
                        self.disconnect()?;
                        if !self.teardown_failed {
                            self.retry_at = 0;
                            self.route_pending = false;
                            self.retry_delay = 1000;
                            self.error = None;
                            self.state = CaptureState::Recovering;
                        }
                    }
                    SystemEvent::ScreensWake => {
                        self.record(None, ControlEventKind::ScreensWake, None);
                        // Display sleep alone does not invalidate a working route.
                        // The worker immediately checks health and refreshes inventory.
                        self.retry_at = 0;
                    }
                }
                return Ok(());
            }
            Command::PermissionChanged(value) => {
                self.permission = value;
                if !value {
                    self.record(None, crate::view::ControlEventKind::PermissionRevoked, None);
                    self.disconnect()?;
                    self.state = CaptureState::Blocked;
                }
            }
            Command::InputReconnected => {
                self.record(None, crate::view::ControlEventKind::InputReconnected, None);
                self.automatic_send("reconnect");
                self.route_pending = true;
                self.retry_at = 0;
                return Ok(());
            }
            Command::Refresh => {
                self.retry_at = 0;
            }
            Command::Configure(config) => {
                let changed_device = self.persist_configuration(config)?;
                if changed_device {
                    self.record(None, crate::view::ControlEventKind::DeviceChanged, None);
                    self.disconnect()?;
                    self.retry_at = 0;
                }
            }
        }
        if let Some(session) = self.session.as_ref() {
            if let Err(error) =
                self.backend
                    .route(session, &self.config, self.wanted_send, self.overview)
            {
                self.record(
                    None,
                    crate::view::ControlEventKind::RouteFailed,
                    Some(&error),
                );
                self.error = Some(error.clone());
                self.disconnect()?;
                self.state = CaptureState::Recovering;
                self.retry_at = 0;
                return Err(error);
            }
            self.route_pending = false;
            self.state = if self.wanted_send {
                CaptureState::Sending
            } else {
                CaptureState::Previewing
            };
        }
        Ok(())
    }
    fn retry(&mut self, now_ms: u64, error: String) {
        self.record(
            Some(now_ms),
            crate::view::ControlEventKind::RetryScheduled,
            Some(&error),
        );
        self.error = Some(error);
        if self.disconnect().is_err() {
            return;
        }
        self.state = CaptureState::Recovering;
        self.retry_at = now_ms.saturating_add(self.retry_delay);
        self.retry_delay = (self.retry_delay * 2).min(30000);
    }
    pub fn tick(&mut self, now_ms: u64) {
        self.tick_with_manual_intent(now_ms, false);
    }
    pub(crate) fn tick_with_manual_intent(&mut self, now_ms: u64, manual_intent: bool) {
        if self.quitting || self.teardown_failed || self.suspended {
            return;
        }
        if !self.permission {
            self.state = CaptureState::Blocked;
            return;
        }
        if self.config.validate_selection().is_err() {
            self.state = CaptureState::Idle;
            return;
        }
        if let Some(session) = self.session.as_ref() {
            match self.backend.healthy(session, &self.config) {
                Ok(true) => {
                    if self.route_pending {
                        self.route_pending = false;
                        match self.backend.route(
                            session,
                            &self.config,
                            self.wanted_send,
                            self.overview,
                        ) {
                            Ok(()) => {
                                self.state = if self.wanted_send {
                                    CaptureState::Sending
                                } else {
                                    CaptureState::Previewing
                                }
                            }
                            Err(error) => self.retry(now_ms, error),
                        }
                    }
                    return;
                }
                Ok(false) => {
                    if !manual_intent {
                        self.automatic_send("reconnect");
                    }
                    self.retry(now_ms, "device or format changed".into());
                }
                Err(error) => {
                    if !manual_intent {
                        self.automatic_send("reconnect");
                    }
                    self.retry(now_ms, error);
                }
            }
            return;
        }
        if now_ms < self.retry_at {
            return;
        }
        if self.pending.is_none() {
            self.record(
                Some(now_ms),
                crate::view::ControlEventKind::NegotiationStarted,
                None,
            );
            match self.backend.negotiate(&self.config) {
                Ok(pending) => {
                    self.pending = Some(pending);
                    self.record(
                        Some(now_ms),
                        crate::view::ControlEventKind::NegotiationRequested,
                        None,
                    );
                    self.state = CaptureState::Connecting;
                }
                Err(error) => {
                    self.retry(now_ms, error);
                    return;
                }
            }
        }
        let pending = self.pending.as_ref().expect("negotiation exists");
        match self.backend.ready(pending) {
            Ok(false) => {}
            Err(error) => self.retry(now_ms, error),
            Ok(true) => {
                self.record(
                    Some(now_ms),
                    crate::view::ControlEventKind::NegotiationReady,
                    None,
                );
                self.pending = None;
                match self.backend.open(&self.config) {
                    Err(error) => {
                        if self.backend.open_failure_requires_stop() {
                            self.record(
                                Some(now_ms),
                                crate::view::ControlEventKind::TeardownFailed,
                                Some(&error),
                            );
                            self.teardown_failed = true;
                            self.error = Some(error);
                            self.state = CaptureState::Blocked;
                        } else {
                            self.retry(now_ms, error);
                        }
                    }
                    Ok(session) => {
                        self.session = Some(session);
                        let result = self.backend.route(
                            self.session.as_ref().expect("opened session"),
                            &self.config,
                            self.wanted_send,
                            self.overview,
                        );
                        if let Err(error) = result {
                            self.retry(now_ms, error);
                            return;
                        }
                        self.route_pending = false;
                        self.retry_delay = 1000;
                        self.record(
                            Some(now_ms),
                            crate::view::ControlEventKind::SessionOpened,
                            None,
                        );
                        self.error = None;
                        self.state = if self.wanted_send {
                            CaptureState::Sending
                        } else {
                            CaptureState::Previewing
                        };
                    }
                }
            }
        }
    }
}

impl<B: Backend> Drop for Controller<B> {
    fn drop(&mut self) {
        let _ = self.disconnect();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        ready: bool,
        healthy: bool,
        save_fail: bool,
        close_fail: bool,
        open_fail: bool,
        retained_on_open_failure: bool,
        negotiate_fail: bool,
        negotiations: usize,
        opens: usize,
        closes: usize,
        sends: Vec<bool>,
    }
    impl Backend for Fake {
        type Pending = ();
        type Session = ();
        fn requested_rate(&self, _: &()) -> Option<u32> {
            Some(48000)
        }
        fn save(&mut self, _: &Config) -> Result<(), String> {
            if self.save_fail {
                Err("save failed".into())
            } else {
                Ok(())
            }
        }
        fn negotiate(&mut self, _: &Config) -> Result<(), String> {
            self.negotiations += 1;
            if self.negotiate_fail {
                Err("device not ready after wake".into())
            } else {
                Ok(())
            }
        }
        fn ready(&mut self, _: &()) -> Result<bool, String> {
            Ok(self.ready)
        }
        fn open(&mut self, _: &Config) -> Result<(), String> {
            self.opens += 1;
            if self.open_fail {
                Err("open failed".into())
            } else {
                Ok(())
            }
        }
        fn healthy(&mut self, _: &(), _: &Config) -> Result<bool, String> {
            Ok(self.healthy)
        }
        fn route(&mut self, _: &(), _: &Config, sending: bool, _: bool) -> Result<(), String> {
            self.sends.push(sending);
            Ok(())
        }
        fn open_failure_requires_stop(&self) -> bool {
            self.retained_on_open_failure
        }
        fn close(&mut self, _: ()) -> Result<(), String> {
            self.closes += 1;
            if self.close_fail {
                Err("close failed".into())
            } else {
                Ok(())
            }
        }
    }
    fn controller() -> Controller<Fake> {
        Controller::new(
            Fake {
                healthy: true,
                ..Fake::default()
            },
            Config {
                device_uid: "source".into(),
                ..Config::default()
            },
        )
    }
    #[test]
    fn input_return_defers_existing_session_route_until_manual_intent_is_applied() {
        let mut worker = controller();
        worker
            .command(Command::Configure(Config {
                auto_send_on_resume: true,
                ..worker.config().clone()
            }))
            .unwrap();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.backend.ready = true;
        worker.tick(0);
        worker.command(Command::InputReconnected).unwrap();
        assert_eq!(
            worker.backend.sends,
            [false],
            "inventory must not route before manual intent"
        );
        worker.command(Command::Stop).unwrap();
        worker.tick_with_manual_intent(1, true);
        assert!(worker.backend.sends.iter().all(|sending| !sending));
        worker.command(Command::InputReconnected).unwrap();
        worker.tick(2);
        assert_eq!(worker.state(), CaptureState::Sending);
        assert_eq!(worker.backend.sends.last(), Some(&true));
    }

    #[test]
    fn same_turn_manual_stop_wins_over_health_recovery_and_later_wake_can_resume() {
        let mut worker = controller();
        worker
            .command(Command::Configure(Config {
                auto_send_on_resume: true,
                muted: true,
                ..worker.config().clone()
            }))
            .unwrap();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.backend.ready = true;
        worker.tick(0);
        worker.command(Command::Start).unwrap();
        worker.command(Command::Stop).unwrap();
        worker.backend.healthy = false;
        worker.tick_with_manual_intent(1, true);
        worker.backend.healthy = true;
        worker.tick(1001);
        assert_eq!(worker.state(), CaptureState::Previewing);
        assert!(!worker.wanted_send());
        assert!(worker.config().muted);
        worker
            .command(Command::System(crate::SystemEvent::Wake))
            .unwrap();
        worker.tick(1002);
        assert_eq!(worker.state(), CaptureState::Sending);
        assert!(worker.config().muted);
        worker.command(Command::Stop).unwrap();
        worker
            .command(Command::Configure(Config {
                device_uid: "other".into(),
                ..worker.config().clone()
            }))
            .unwrap();
        worker.tick(1003);
        assert_eq!(worker.state(), CaptureState::Previewing);
        assert!(!worker.wanted_send());
    }

    #[test]
    fn automatic_send_is_event_based_and_enabling_does_not_start() {
        use crate::SystemEvent;
        for auto in [false, true] {
            let mut worker = controller();
            worker.command(Command::PermissionChanged(true)).unwrap();
            worker.backend.ready = true;
            worker.tick(0);
            let config = Config {
                auto_send_on_resume: auto,
                ..worker.config().clone()
            };
            worker.command(Command::Configure(config)).unwrap();
            assert_eq!(worker.state(), CaptureState::Previewing);
            assert!(!worker.wanted_send());
            worker.command(Command::System(SystemEvent::Wake)).unwrap();
            worker.tick(1);
            assert_eq!(worker.wanted_send(), auto);
            assert_eq!(
                worker.state(),
                if auto {
                    CaptureState::Sending
                } else {
                    CaptureState::Previewing
                }
            );
            worker.command(Command::Stop).unwrap();
            assert_eq!(worker.state(), CaptureState::Previewing);
            worker.backend.healthy = false;
            worker.tick(2);
            worker.backend.healthy = true;
            worker.tick(1002);
            assert_eq!(worker.wanted_send(), auto);
            assert_eq!(
                worker.state(),
                if auto {
                    CaptureState::Sending
                } else {
                    CaptureState::Previewing
                }
            );
        }
    }

    #[test]
    fn input_return_bypasses_backoff_and_requests_send_only_when_opted_in() {
        for auto in [false, true] {
            let mut worker = controller();
            worker
                .command(Command::Configure(Config {
                    auto_send_on_resume: auto,
                    ..worker.config().clone()
                }))
                .unwrap();
            worker.command(Command::PermissionChanged(true)).unwrap();
            worker.backend.negotiate_fail = true;
            worker.tick(0);
            assert!(
                !worker.wanted_send(),
                "startup while absent must wait for a real return"
            );
            worker.backend.negotiate_fail = false;
            worker.backend.ready = true;
            worker.command(Command::InputReconnected).unwrap();
            worker.tick(1);
            assert_eq!(worker.wanted_send(), auto);
            assert_eq!(
                worker.state(),
                if auto {
                    CaptureState::Sending
                } else {
                    CaptureState::Previewing
                }
            );
            assert_eq!(worker.backend.opens, 1);
        }
    }

    #[test]
    fn automatic_intent_never_bypasses_permission_or_unknown_teardown() {
        use crate::SystemEvent;
        let mut worker = controller();
        worker
            .command(Command::Configure(Config {
                auto_send_on_resume: true,
                ..worker.config().clone()
            }))
            .unwrap();
        worker.backend.ready = true;
        worker.command(Command::System(SystemEvent::Wake)).unwrap();
        worker.command(Command::InputReconnected).unwrap();
        worker.tick(0);
        assert_eq!(worker.state(), CaptureState::Blocked);
        assert_eq!(worker.backend.opens, 0);
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.tick(1);
        worker.backend.close_fail = true;
        worker.backend.healthy = false;
        worker.tick(2);
        worker.backend.close_fail = false;
        worker.command(Command::Stop).unwrap();
        worker.command(Command::System(SystemEvent::Wake)).unwrap();
        worker.command(Command::InputReconnected).unwrap();
        worker.tick(100000);
        assert!(worker.wanted_send());
        assert_eq!(worker.state(), CaptureState::Blocked);
        assert_eq!(worker.backend.opens, 1);
    }

    #[test]
    fn failed_save_cannot_enable_automatic_send_or_change_intent() {
        let mut worker = controller();
        worker.backend.save_fail = true;
        assert!(
            worker
                .command(Command::Configure(Config {
                    auto_send_on_resume: true,
                    ..worker.config().clone()
                }))
                .is_err()
        );
        assert!(!worker.config().auto_send_on_resume);
        worker.command(Command::InputReconnected).unwrap();
        assert!(!worker.wanted_send());
    }

    #[test]
    fn detached_sender_reopens_with_exact_policy_and_records_user_intent() {
        use crate::view::ControlEventKind;
        let mut worker = controller();
        let config = Config {
            left: 3,
            right: 4,
            mode: crate::Mode::Stereo,
            muted: true,
            ..worker.config().clone()
        };
        worker.command(Command::Configure(config.clone())).unwrap();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.command(Command::Start).unwrap();
        worker.backend.ready = true;
        worker.tick(0);
        worker.backend.healthy = false;
        worker.backend.negotiate_fail = true;
        worker.tick(100);
        worker.tick(1100);
        assert_eq!(worker.state(), CaptureState::Recovering);
        assert!(worker.wanted_send());
        assert_eq!(worker.config(), &config);
        worker.backend.negotiate_fail = false;
        worker.backend.healthy = true;
        worker.tick(3100);
        assert_eq!(worker.state(), CaptureState::Sending);
        assert_eq!(worker.backend.sends, [true, true]);
        assert_eq!(worker.config(), &config);
        let history = worker.history();
        assert_eq!(
            history
                .iter()
                .filter(|event| event.kind == ControlEventKind::SendStarted)
                .count(),
            1
        );
        assert!(
            !history
                .iter()
                .any(|event| event.kind == ControlEventKind::SendStopped)
        );
    }

    #[test]
    fn stop_while_device_is_absent_survives_wake_and_restores_preview() {
        use crate::{SystemEvent, view::ControlEventKind};
        let mut worker = controller();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.command(Command::Start).unwrap();
        worker.backend.ready = true;
        worker.tick(0);
        worker.backend.healthy = false;
        worker.backend.negotiate_fail = true;
        worker.tick(100);
        worker.tick(1100);
        worker.command(Command::Stop).unwrap();
        worker.command(Command::System(SystemEvent::Sleep)).unwrap();
        worker.command(Command::System(SystemEvent::Wake)).unwrap();
        worker.tick(1200);
        assert_eq!(worker.state(), CaptureState::Recovering);
        assert!(!worker.wanted_send());
        worker.backend.healthy = true;
        worker.backend.negotiate_fail = false;
        worker.tick(2200);
        assert_eq!(worker.state(), CaptureState::Previewing);
        assert_eq!(worker.backend.sends, [true, false]);
        assert!(
            worker
                .history()
                .iter()
                .any(|event| event.kind == ControlEventKind::SendStopped)
        );
    }

    #[test]
    fn unknown_teardown_failure_stays_blocked_across_recovery_commands() {
        use crate::SystemEvent;
        let mut worker = controller();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.command(Command::Start).unwrap();
        worker.backend.ready = true;
        worker.tick(0);
        worker.backend.close_fail = true;
        worker.backend.healthy = false;
        worker.tick(100);
        worker.backend.close_fail = false;
        worker.backend.healthy = true;
        for command in [
            Command::System(SystemEvent::Sleep),
            Command::System(SystemEvent::Wake),
            Command::System(SystemEvent::ScreensWake),
            Command::Refresh,
            Command::Stop,
            Command::Start,
            Command::PermissionChanged(false),
            Command::PermissionChanged(true),
            Command::Configure(Config {
                device_uid: "replacement".into(),
                ..worker.config().clone()
            }),
        ] {
            worker.command(command).unwrap();
            worker.tick(100000);
            assert_eq!(worker.state(), CaptureState::Blocked);
            assert_eq!(worker.backend.opens, 1);
        }
        assert!(worker.wanted_send());
        assert_eq!(worker.error(), Some("close failed"));
    }

    #[test]
    fn sleep_releases_streams_and_wake_preserves_stop_mute_and_selection() {
        use crate::SystemEvent;
        let mut worker = controller();
        let config = Config {
            left: 3,
            right: 4,
            mode: crate::Mode::Stereo,
            muted: true,
            ..worker.config().clone()
        };
        worker.command(Command::Configure(config.clone())).unwrap();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.command(Command::Start).unwrap();
        worker.backend.ready = true;
        worker.tick(0);
        worker.command(Command::System(SystemEvent::Sleep)).unwrap();
        assert_eq!(worker.state(), CaptureState::Suspended);
        assert_eq!(worker.backend.closes, 1);
        assert!(worker.wanted_send());
        worker.command(Command::System(SystemEvent::Sleep)).unwrap();
        worker
            .command(Command::System(SystemEvent::ScreensWake))
            .unwrap();
        worker.tick(10000);
        assert_eq!((worker.backend.opens, worker.backend.closes), (1, 1));
        worker.command(Command::Stop).unwrap();
        worker.command(Command::System(SystemEvent::Wake)).unwrap();
        worker.tick(10001);
        assert_eq!(worker.state(), CaptureState::Previewing);
        assert_eq!(worker.backend.sends, [true, false]);
        assert_eq!(worker.config(), &config);
        worker.command(Command::Start).unwrap();
        worker
            .command(Command::System(SystemEvent::ScreensWake))
            .unwrap();
        worker.tick(10002);
        assert_eq!(
            worker.backend.opens, 2,
            "display wake must preserve a healthy route"
        );
        worker.command(Command::System(SystemEvent::Wake)).unwrap();
        worker.tick(10003);
        assert_eq!(
            worker.backend.opens, 3,
            "wake-only must retire a stale session"
        );
        assert_eq!(worker.state(), CaptureState::Sending);
        assert_eq!(worker.config(), &config);
    }
    #[test]
    fn wake_restarts_pending_negotiation_and_retries_devices_still_unavailable() {
        use crate::SystemEvent;
        let mut worker = controller();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.command(Command::Start).unwrap();
        worker.tick(0);
        assert!(worker.pending.is_some());
        worker.command(Command::System(SystemEvent::Sleep)).unwrap();
        assert!(worker.pending.is_none());
        worker.tick(10000);
        assert_eq!(worker.backend.negotiations, 1);
        worker.backend.negotiate_fail = true;
        worker.command(Command::System(SystemEvent::Wake)).unwrap();
        worker.tick(10001);
        assert_eq!(worker.state(), CaptureState::Recovering);
        worker.tick(11000);
        assert_eq!(worker.backend.negotiations, 2);
        worker.backend.negotiate_fail = false;
        worker.backend.ready = true;
        worker.tick(11001);
        assert_eq!(worker.state(), CaptureState::Sending);
        assert_eq!(worker.backend.negotiations, 3);
        // A new wake discards even a long retry accumulated before sleep.
        worker.retry_delay = 30000;
        worker.retry(12000, "fixture disconnected".into());
        worker.command(Command::System(SystemEvent::Wake)).unwrap();
        worker.tick(12001);
        assert_eq!(worker.state(), CaptureState::Sending);
        assert_eq!(worker.retry_delay, 1000);
    }
    #[test]
    fn wake_never_bypasses_revoked_permission_or_unsafe_teardown() {
        use crate::SystemEvent;
        for failed_teardown in [false, true] {
            let mut worker = controller();
            worker.command(Command::PermissionChanged(true)).unwrap();
            worker.command(Command::Start).unwrap();
            worker.backend.ready = true;
            worker.tick(0);
            worker.backend.close_fail = failed_teardown;
            assert_eq!(
                worker.command(Command::System(SystemEvent::Sleep)).is_err(),
                failed_teardown
            );
            worker.command(Command::PermissionChanged(false)).unwrap();
            worker.command(Command::System(SystemEvent::Wake)).unwrap();
            worker.tick(10000);
            assert_eq!(worker.state(), CaptureState::Blocked);
            assert_eq!(worker.backend.opens, 1);
            worker.command(Command::PermissionChanged(true)).unwrap();
            worker.tick(10001);
            assert_eq!(worker.backend.opens, if failed_teardown { 1 } else { 2 });
            if failed_teardown {
                assert_eq!(worker.error(), Some("close failed"));
            }
        }
    }
    #[test]
    fn pending_rate_is_preserved_in_failure_history_before_disconnect() {
        let mut worker = controller();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.tick(0);
        assert_eq!(worker.state(), CaptureState::Connecting);
        worker.retry(100, "format follow timed out".into());
        assert!(worker.pending.is_none());
        let history = worker.history();
        let failed = history.last().unwrap();
        assert_eq!(failed.kind, crate::view::ControlEventKind::RetryScheduled);
        assert_eq!(failed.requested_rate_hz, Some(48000));
        assert_eq!(failed.detail.as_deref(), Some("format follow timed out"));
    }
    #[test]
    fn history_keeps_recovery_reason_after_success_and_bounds_utf8_details() {
        use crate::view::ControlEventKind;
        let mut worker = controller();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.backend.ready = true;
        worker.backend.open_fail = true;
        worker.tick(0);
        let failed = worker.history();
        assert_eq!(
            failed.last().unwrap().kind,
            ControlEventKind::RetryScheduled
        );
        assert_eq!(
            failed.last().unwrap().detail.as_deref(),
            Some("open failed")
        );
        worker.backend.open_fail = false;
        worker.tick(1000);
        assert_eq!(worker.error(), None);
        assert!(
            worker
                .history()
                .iter()
                .any(|event| event.kind == ControlEventKind::NegotiationRequested
                    && event.requested_rate_hz == Some(48000))
        );
        assert!(
            worker
                .history()
                .iter()
                .any(|event| event.kind == ControlEventKind::NegotiationReady
                    && event.requested_rate_hz == Some(48000))
        );
        assert_eq!(
            worker.history().last().unwrap().kind,
            ControlEventKind::SessionOpened
        );
        assert!(
            worker
                .history()
                .iter()
                .any(|event| event.detail.as_deref() == Some("open failed"))
        );
        assert_eq!(
            failed.last().unwrap().kind,
            ControlEventKind::RetryScheduled
        );
        for index in 0..40 {
            worker.retry(2000 + index, "音".repeat(200));
        }
        let events = worker.history();
        assert_eq!(events.len(), 32);
        assert_eq!(events.first().unwrap().worker_ms, Some(2008));
        assert_eq!(events.last().unwrap().worker_ms, Some(2039));
        assert!(
            events
                .iter()
                .all(|event| event.detail.as_ref().unwrap().len() == 255)
        );
    }
    #[test]
    fn clean_start_failure_retries_but_retained_context_never_starts_replacement() {
        for retained in [false, true] {
            let mut worker = controller();
            worker.command(Command::PermissionChanged(true)).unwrap();
            worker.backend.ready = true;
            worker.backend.open_fail = true;
            worker.backend.retained_on_open_failure = retained;
            worker.tick(0);
            assert_eq!(worker.backend.opens, 1);
            assert_eq!(
                worker.state(),
                if retained {
                    CaptureState::Blocked
                } else {
                    CaptureState::Recovering
                }
            );
            worker.backend.open_fail = false;
            worker.tick(999);
            assert_eq!(worker.backend.opens, 1);
            worker.tick(1000);
            assert_eq!(worker.backend.opens, if retained { 1 } else { 2 });
            if retained {
                worker.command(Command::Refresh).unwrap();
                worker.command(Command::Start).unwrap();
                worker.tick(100000);
                assert_eq!(worker.backend.opens, 1);
            }
        }
    }
    #[test]
    fn permission_revocation_closes_audio_and_stop_prevents_restart_on_return() {
        let mut worker = controller();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.command(Command::Start).unwrap();
        worker.backend.ready = true;
        worker.tick(0);
        assert_eq!(worker.state(), CaptureState::Sending);
        worker.command(Command::PermissionChanged(false)).unwrap();
        assert_eq!(worker.backend.closes, 1);
        assert_eq!(worker.state(), CaptureState::Blocked);
        worker.tick(10000);
        assert_eq!(worker.backend.opens, 1);
        worker.command(Command::Stop).unwrap();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.tick(10001);
        assert_eq!(worker.state(), CaptureState::Previewing);
        assert_eq!(worker.backend.sends.last(), Some(&false));
        worker.command(Command::Quit).unwrap();
        assert_eq!(worker.backend.closes, 2);
        worker.tick(20000);
        assert_eq!(worker.backend.opens, 2);
        assert!(worker.command(Command::Start).is_err());
    }
    #[test]
    fn stop_during_negotiation_survives_reconnect_and_preview_stays_live() {
        let mut worker = controller();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.command(Command::Start).unwrap();
        worker.tick(0);
        assert_eq!(worker.state(), CaptureState::Connecting);
        worker.command(Command::Stop).unwrap();
        worker.backend.ready = true;
        worker.tick(10);
        assert_eq!(worker.state(), CaptureState::Previewing);
        assert_eq!(worker.backend.sends, [false]);
        worker.backend.healthy = false;
        worker.tick(20);
        assert_eq!(worker.state(), CaptureState::Recovering);
        worker.tick(100);
        assert_eq!(worker.backend.opens, 1);
        worker.backend.healthy = true;
        worker.tick(1020);
        assert_eq!(worker.backend.opens, 2);
        assert_eq!(worker.backend.sends, [false, false]);
        worker.command(Command::Start).unwrap();
        worker.backend.healthy = false;
        worker.tick(1030);
        worker.backend.healthy = true;
        worker.tick(2030);
        assert_eq!(worker.state(), CaptureState::Sending);
        assert!(worker.wanted_send());
    }
    #[test]
    fn failed_save_preserves_policy_and_failed_teardown_blocks_new_sessions() {
        let mut worker = controller();
        worker.command(Command::PermissionChanged(true)).unwrap();
        worker.backend.ready = true;
        worker.tick(0);
        let old = worker.config().clone();
        worker.backend.save_fail = true;
        let changed = Config {
            device_uid: "different".into(),
            ..old.clone()
        };
        assert!(worker.command(Command::Configure(changed)).is_err());
        assert_eq!(worker.config(), &old);
        assert_eq!(worker.backend.closes, 0);
        worker.backend.close_fail = true;
        worker.backend.healthy = false;
        worker.tick(1);
        assert_eq!(worker.state(), CaptureState::Blocked);
        worker.command(Command::Refresh).unwrap();
        worker.tick(100000);
        assert_eq!(worker.backend.opens, 1);
    }
}
