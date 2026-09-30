#!/bin/bash
# Use a normally installed toolchain, or the isolated local bootstrap without
# changing the user's shell configuration or global Cargo/Rustup directories.
CHANNELBRIDGE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if ! command -v cargo >/dev/null 2>&1; then
    export CARGO_HOME="$CHANNELBRIDGE_ROOT/work/rust-bootstrap/cargo"
    export RUSTUP_HOME="$CHANNELBRIDGE_ROOT/work/rust-bootstrap/rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
fi
