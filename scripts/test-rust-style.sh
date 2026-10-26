#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
mkdir -p "$CHANNELBRIDGE_ROOT/work/test/rust"
stage="$(mktemp -d "$CHANNELBRIDGE_ROOT/work/test/rust/style-policy.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/src"
cat > "$stage/Cargo.toml" <<'TOML'
[package]
name = "channelbridge-style-policy-fixture"
version = "0.0.0"
edition = "2024"
[workspace]
TOML
cat > "$stage/src/main.rs" <<'RUST'
use std::sync::Mutex;
fn main() {
    let mutex = Mutex::new(1);
    let Ok(guard) = mutex.lock() else { return };
    drop(guard);
}
RUST
export CLIPPY_CONF_DIR="$CHANNELBRIDGE_ROOT"
export CARGO_TARGET_DIR="$stage/target"
cargo clippy --offline --quiet --manifest-path "$stage/Cargo.toml" \
    -- -D clippy::disallowed_methods
cat > "$stage/src/main.rs" <<'RUST'
use std::sync::{Mutex, PoisonError as PoisonState};
fn main() {
    let mutex = Mutex::new(1);
    let guard = mutex.lock().unwrap_or_else(|error: PoisonState<_>| error.into_inner());
    drop(guard);
}
RUST
if cargo clippy --offline --quiet --manifest-path "$stage/Cargo.toml" \
    -- -D clippy::disallowed_methods > "$stage/negative.log" 2>&1; then
    printf 'Poison extraction unexpectedly passed the typed policy check\n' >&2
    exit 1
fi
python3 - "$stage/negative.log" <<'PY'
import pathlib, sys
diagnostic = pathlib.Path(sys.argv[1]).read_text()
if 'disallowed method' not in diagnostic or 'Do not read poisoned synchronization state' not in diagnostic:
    raise SystemExit(diagnostic)
PY
printf 'Typed poison policy accepts refusal and rejects extraction through an alias\n'
