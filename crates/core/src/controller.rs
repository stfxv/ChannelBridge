//! Control-thread state machine. Backend calls must not block audio callbacks.
use crate::{CaptureState, Command, Config};

pub trait Backend {
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
        }
    }
    pub fn devices(&mut self) -> Result<Vec<crate::view::DeviceChoice>, String> {
        self.backend.devices()
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
                self.pending = None;
                match self.backend.open(&self.config) {
                    Err(error) => {
                        if self.backend.open_failure_requires_stop() {
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
