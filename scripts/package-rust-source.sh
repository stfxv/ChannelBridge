#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
if [[ "$#" != 1 ]]; then
    printf 'Usage: bash scripts/package-rust-source.sh OUTPUT.tar.gz\n' >&2
    exit 2
fi
output="$1"
if [[ "$output" != /* ]]; then output="$CHANNELBRIDGE_ROOT/$output"; fi
stage="$(mktemp -d "${TMPDIR:-/tmp}/channelbridge-source.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/.cargo" "$stage/scripts" "$stage/packaging/assets"
cp Cargo.toml Cargo.lock rust-toolchain.toml LICENSE THIRD_PARTY_RUST.md \
    ARCHITECTURE.md rustfmt.toml clippy.toml \
    RUST_APP_TEST.md RUST_AUDIO_TEST.md RUST_DISTRIBUTION.md \
    RUST_HAL_TEST.md RUST_MIGRATION.md RUST_STYLE.md \
    RUST_RENDER_TIMING.tsv RUST_RESPONSE_GRID.tsv "$stage/"
cp RUST_DISTRIBUTION.md "$stage/README.md"
ditto --norsrc --noextattr --noacl crates "$stage/crates"
cp scripts/*rust* "$stage/scripts/"
cp scripts/build.sh scripts/test.sh "$stage/scripts/"
mkdir -p "$stage/tools/rust-style"
cp tools/rust-style/Cargo.toml tools/rust-style/Cargo.lock \
    tools/rust-style/README.md "$stage/tools/rust-style/"
ditto --norsrc --noextattr --noacl tools/rust-style/src "$stage/tools/rust-style/src"
ditto --norsrc --noextattr --noacl tools/rust-style/tests "$stage/tools/rust-style/tests"
cp packaging/Rust-*.plist "$stage/packaging/"
cp packaging/assets/AppIcon.png "$stage/packaging/assets/"
(cd "$stage"; cargo vendor --locked --respect-source-config --manifest-path "$CHANNELBRIDGE_ROOT/Cargo.toml" rust-vendor > .cargo/config.toml)
COPYFILE_DISABLE=1 tar -czf "$output" -C "$stage" .
