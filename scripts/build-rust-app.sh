#!/bin/bash
# Produce a separate test app; never install, launch its UI or replace production.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
export MACOSX_DEPLOYMENT_TARGET=13.0
for architecture in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --locked --release -p channelbridge-app --target "$architecture"
done
mkdir -p work/rust-dist
stage="$(mktemp -d "$CHANNELBRIDGE_ROOT/work/rust-dist/app-stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
bundle="$stage/ChannelBridgeRustTest.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp packaging/Rust-Test-App-Info.plist "$bundle/Contents/Info.plist"
plutil -lint "$bundle/Contents/Info.plist"
lipo -create target/aarch64-apple-darwin/release/channelbridge-app \
    target/x86_64-apple-darwin/release/channelbridge-app \
    -output "$bundle/Contents/MacOS/ChannelBridgeRustTest"
iconset="$stage/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" packaging/assets/AppIcon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" packaging/assets/AppIcon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$bundle/Contents/Resources/AppIcon.icns"
cp LICENSE "$bundle/Contents/Resources/"
cp THIRD_PARTY_RUST.md "$bundle/Contents/Resources/THIRD_PARTY.md"
# Include locked dependency sources and their license files for offline rebuilds.
bash scripts/package-rust-source.sh "$bundle/Contents/Resources/ChannelBridge-source.tar.gz"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"
for architecture in arm64 x86_64; do
    lipo "$bundle/Contents/MacOS/ChannelBridgeRustTest" -verify_arch "$architecture"
done
"$bundle/Contents/MacOS/ChannelBridgeRustTest" --check-bundle
if [[ "${1:-}" != "--build-only" ]]; then
    "$bundle/Contents/MacOS/ChannelBridgeRustTest" --smoke --require-bundle
fi
output="$CHANNELBRIDGE_ROOT/work/rust-dist/ChannelBridgeRustTest.app"
rm -rf "$output"
mv "$bundle" "$output"
printf 'Test app built: %s\n' "$output"
