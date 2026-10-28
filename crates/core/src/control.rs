use crate::Config;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum CaptureState {
    #[default]
    Idle,
    Connecting,
    Previewing,
    Sending,
    Recovering,
    Suspended,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    Sleep,
    Wake,
    ScreensWake,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Start,
    Stop,
    Quit,
    Refresh,
    InputReconnected,
    Configure(Config),
    Overview(bool),
    PermissionChanged(bool),
    System(SystemEvent),
}
