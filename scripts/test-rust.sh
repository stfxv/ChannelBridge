#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
mkdir -p work/test/rust
bash scripts/check-rust-style.sh check
bash scripts/test-rust-style.sh
cargo test --offline --locked --manifest-path tools/rust-style/Cargo.toml
cargo clippy --offline --locked --manifest-path tools/rust-style/Cargo.toml --all-targets \
    -- -D warnings -D clippy::unwrap_used -D clippy::disallowed_methods
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo run --locked --quiet -p channelbridge-app -- --sleep-wake-smoke
cargo run --locked --quiet -p channelbridge-audio --example realtime_allocations
cargo run --locked --quiet -p channelbridge-macos --example native_realtime_allocations
cargo clippy --locked -p channelbridge-audio --all-targets --features model-check -- -D warnings
cargo test --locked -p channelbridge-audio --features model-check model_tests
cargo test --locked --release -p channelbridge-audio response_grid -- --ignored
clang -Wall -Wextra -Werror scripts/test_rust_abi.c -framework CoreAudio -o work/test/rust/abi
work/test/rust/abi > work/test/rust/apple-layout.txt
cargo run --locked --quiet -p channelbridge-hal-abi --example layout > work/test/rust/rust-layout.txt
diff -u work/test/rust/apple-layout.txt work/test/rust/rust-layout.txt
driver_library="$(cargo build --locked -p channelbridge-driver --message-format=json | python3 -c '
import json, sys
libraries = [
    name for message in map(json.loads, sys.stdin)
    if message.get("reason") == "compiler-artifact" and message["target"]["name"] == "channelbridge_driver"
    for name in message["filenames"] if name.endswith(".dylib")
]
if len(libraries) != 1:
    sys.exit("expected one ChannelBridge driver library from Cargo")
print(libraries[0])
')"
clang -g -fsanitize=address,undefined scripts/test_rust_driver.c -framework CoreAudio -framework CoreFoundation -o work/test/rust/driver
work/test/rust/driver "$driver_library"
