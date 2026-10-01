#!/bin/bash
# Archive only corresponding Rust sources; do not carry legacy audio code.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
if [[ "$#" != 1 ]]; then
    printf 'Usage: bash scripts/package-rust-source.sh OUTPUT.tar.gz\n' >&2
    exit 2
fi
output="$1"
if [[ "$output" != /* ]]; then output="$CHANNELBRIDGE_ROOT/$output"; fi
mkdir -p work/rust-dist
stage="$(mktemp -d "$CHANNELBRIDGE_ROOT/work/rust-dist/source-stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/.cargo" "$stage/scripts" "$stage/packaging/assets"
cp Cargo.toml Cargo.lock rust-toolchain.toml LICENSE THIRD_PARTY_RUST.md \
    ARCHITECTURE.md RUST_*.md RUST_*.tsv "$stage/"
cp RUST_DISTRIBUTION.md "$stage/README.md"
ditto --norsrc --noextattr --noacl crates "$stage/crates"
cp scripts/*rust* "$stage/scripts/"
cp packaging/Rust-*.plist "$stage/packaging/"
cp packaging/assets/AppIcon.png "$stage/packaging/assets/"
(cd "$stage"; cargo vendor --locked --respect-source-config --manifest-path "$CHANNELBRIDGE_ROOT/Cargo.toml" rust-vendor > .cargo/config.toml)
COPYFILE_DISABLE=1 tar -czf "$output" -C "$stage" .
