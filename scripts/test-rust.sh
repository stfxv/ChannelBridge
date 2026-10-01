#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
mkdir -p work/test/rust
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --quiet -p channelbridge-audio --example realtime_allocations
cargo run --quiet -p channelbridge-macos --example realtime_allocations
cargo test -p channelbridge-audio --features model-check ring::model_tests
clang -Wall -Wextra -Werror scripts/test_rust_abi.c -framework CoreAudio -o work/test/rust/abi
work/test/rust/abi > work/test/rust/apple-layout.txt
cargo run --quiet -p channelbridge-hal-abi --example layout > work/test/rust/rust-layout.txt
diff -u work/test/rust/apple-layout.txt work/test/rust/rust-layout.txt
cargo build -p channelbridge-driver
clang -g -fsanitize=address,undefined scripts/test_rust_driver.c -framework CoreAudio -framework CoreFoundation -o work/test/rust/driver
work/test/rust/driver "$CHANNELBRIDGE_ROOT/target/debug/libchannelbridge_driver.dylib"
