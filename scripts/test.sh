#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/test-rust.sh
source scripts/rust-env.sh
cargo test --workspace --locked --features production
cargo clippy --workspace --all-targets --locked --features production -- -D warnings
