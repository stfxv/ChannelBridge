//! Routing policy and configuration. No AppKit, HAL or audio callback ownership.
#![forbid(unsafe_code)]

mod config;
mod control;
pub mod controller;
pub mod diagnostics;
mod file_publication;
pub mod storage;
pub mod view;
pub mod worker;

pub use config::{Config, Mode, SelectionError};
pub use control::{CaptureState, Command, SystemEvent};
