//! Control-thread state machine. Backend calls must not block audio callbacks.
use crate::{CaptureState, Command, Config};

pub trait Backend {
    fn driver_diagnostics(&self) -> Option<crate::view::DriverReading> {
        None
    }
    fn devices(&mut self) -> Result<Vec<crate::view::DeviceChoice>, String> {
        Ok(Vec::new())
    }
    type Pending;
    type Session;
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
    overview: bool,
    permission: bool,
    quitting: bool,
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
            overview: false,
            permission: false,
            quitting: false,
            teardown_failed: false,
            retry_at: 0,
            retry_delay: 1000,
            state: CaptureState::Idle,
            error: None,
            history: std::sync::Arc::from([]),
        }
    }
    pub fn devices(&mut self) -> Result<Vec<crate::view::DeviceChoice>, String> {
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
        });
        self.history = events.into();
    }
    pub fn driver_diagnostics(&self) -> Option<crate::view::DriverReading> {
        self.backend.driver_diagnostics()
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
    pub fn command(&mut self, command: Command) -> Result<(), String> {
        if self.quitting {
            return Err("worker is shutting down".into());
        }
        match command {
            Command::Start => self.wanted_send = true,
            Command::Stop => self.wanted_send = false,
            Command::Quit => {
                self.quitting = true;
                self.wanted_send = false;
                self.disconnect()?;
                self.state = CaptureState::Idle;
                return Ok(());
            }
            Command::Overview(value) => self.overview = value,
            Command::PermissionChanged(value) => {
                self.permission = value;
                if !value {
                    self.record(None, crate::view::ControlEventKind::PermissionRevoked, None);
                    self.disconnect()?;
                    self.state = CaptureState::Blocked;
                }
            }
            Command::Refresh => {
                self.retry_at = 0;
            }
            Command::Configure(config) => {
                config
                    .validate_selection()
                    .map_err(|error| format!("{error:?}"))?;
                // Commit before changing either the live route or saved policy.
                self.backend.save(&config)?;
                let changed_device = self.config.device_uid != config.device_uid;
                self.config = config;
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
        if self.quitting || self.teardown_failed {
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
                Ok(true) => return,
                Ok(false) => self.retry(now_ms, "device or format changed".into()),
                Err(error) => self.retry(now_ms, error),
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
        opens: usize,
        closes: usize,
        sends: Vec<bool>,
    }
    impl Backend for Fake {
        type Pending = ();
        type Session = ();
        fn save(&mut self, _: &Config) -> Result<(), String> {
            if self.save_fail {
                Err("save failed".into())
            } else {
                Ok(())
            }
        }
        fn negotiate(&mut self, _: &Config) -> Result<(), String> {
            Ok(())
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
