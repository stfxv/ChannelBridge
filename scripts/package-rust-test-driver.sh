#!/bin/bash
# Development-only package. No postinstall audio-service restart.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
bash scripts/build-rust-driver.sh
stage="$(mktemp -d "${TMPDIR:-/tmp}/channelbridge-rust-package.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
destination="$stage/Library/Audio/Plug-Ins/HAL"
mkdir -p "$destination"
ditto --norsrc --noextattr --noacl work/rust-dist/ChannelBridgeRustTest.driver "$destination/ChannelBridgeRustTest.driver"
xattr -cr "$stage"
COPYFILE_DISABLE=1 pkgbuild --root "$stage" \
    --component-plist packaging/Rust-Test-Driver-Components.plist \
    --identifier local.channelbridge.rust.driver.test.pkg --version 0.5.0 \
    --ownership recommended --install-location / \
    work/rust-dist/ChannelBridgeRustTest.pkg
