#!/bin/bash
# Build the separate test driver; never install it or reload the audio service.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
export MACOSX_DEPLOYMENT_TARGET=13.0
for architecture in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --locked --release -p channelbridge-driver --target "$architecture"
done
bundle="$CHANNELBRIDGE_ROOT/work/rust-dist/ChannelBridgeRustTest.driver"
mkdir -p "$bundle/Contents/MacOS"
cp packaging/Rust-Test-Driver-Info.plist "$bundle/Contents/Info.plist"
plutil -lint "$bundle/Contents/Info.plist"
lipo -create target/aarch64-apple-darwin/release/libchannelbridge_driver.dylib \
    target/x86_64-apple-darwin/release/libchannelbridge_driver.dylib \
    -output "$bundle/Contents/MacOS/ChannelBridgeRustTest"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"
for architecture in arm64 x86_64; do
    lipo "$bundle/Contents/MacOS/ChannelBridgeRustTest" -verify_arch "$architecture"
done
printf 'Test driver built: %s\n' "$bundle"
