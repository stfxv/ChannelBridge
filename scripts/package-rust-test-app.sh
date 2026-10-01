#!/bin/bash
# One development installer for the app and driver. Does not install or restart.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
app_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' packaging/Rust-Test-App-Info.plist)"
driver_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' packaging/Rust-Test-Driver-Info.plist)"
if [[ "$app_version" != "$driver_version" ]]; then
    printf 'App and driver package versions must agree\n' >&2
    exit 1
fi
bash scripts/build-rust-driver.sh
bash scripts/build-rust-app.sh --build-only
bash scripts/verify-rust-source.sh
stage="$(mktemp -d "$CHANNELBRIDGE_ROOT/work/rust-dist/installer-stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
root="$stage/root"
app_path='Applications/ChannelBridge Rust Test.app'
driver_path='Library/Audio/Plug-Ins/HAL/ChannelBridgeRustTest.driver'
mkdir -p "$root/Applications" "$root/Library/Audio/Plug-Ins/HAL"
ditto --norsrc --noextattr --noacl work/rust-dist/ChannelBridgeRustTest.app "$root/$app_path"
ditto --norsrc --noextattr --noacl work/rust-dist/ChannelBridgeRustTest.driver "$root/$driver_path"
xattr -cr "$root"
COPYFILE_DISABLE=1 pkgbuild --root "$root" \
    --component-plist packaging/Rust-Test-App-Components.plist \
    --identifier local.channelbridge.rust.test.pkg --version "$app_version" \
    --ownership recommended --install-location / "$stage/ChannelBridge-Rust-Test.pkg"
pkgutil --expand-full "$stage/ChannelBridge-Rust-Test.pkg" "$stage/expanded"
for path in "$app_path" "$driver_path"; do
    codesign --verify --strict "$stage/expanded/Payload/$path"
    cmp "$root/$path/Contents/MacOS/ChannelBridgeRustTest" \
        "$stage/expanded/Payload/$path/Contents/MacOS/ChannelBridgeRustTest"
done
cmp "$root/$app_path/Contents/Resources/ChannelBridge-source.tar.gz" \
    "$stage/expanded/Payload/$app_path/Contents/Resources/ChannelBridge-source.tar.gz"
mv "$stage/ChannelBridge-Rust-Test.pkg" work/rust-dist/ChannelBridge-Rust-Test.pkg
printf 'Combined test installer verified: %s\n' "$CHANNELBRIDGE_ROOT/work/rust-dist/ChannelBridge-Rust-Test.pkg"
