//! Minimal plug-in ABI declarations, kept separate from HAL client APIs.
//! Layouts are checked against the installed Apple SDK by scripts/test-rust.sh.
pub mod interface;
mod types;
pub use types::*;
