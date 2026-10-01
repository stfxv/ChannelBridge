#!/bin/bash
# Production profile installer for the app and driver. Does not install or restart.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
app_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' packaging/Rust-Production-App-Info.plist)"
driver_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' packaging/Rust-Production-Driver-Info.plist)"
if [[ "$app_version" != "$driver_version" ]]; then
    printf 'App and driver package versions must agree\n' >&2
    exit 1
fi
bash scripts/build-rust-driver.sh --production
bash scripts/build-rust-app.sh --production --build-only
bash scripts/verify-rust-source.sh work/rust-production-dist/ChannelBridge.app/Contents/Resources/ChannelBridge-source.tar.gz --production
stage="$(mktemp -d "$CHANNELBRIDGE_ROOT/work/rust-production-dist/installer-stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
root="$stage/root"
app_path='Applications/ChannelBridge.app'
driver_path='Library/Audio/Plug-Ins/HAL/ChannelBridge.driver'
mkdir -p "$root/Applications" "$root/Library/Audio/Plug-Ins/HAL"
ditto --norsrc --noextattr --noacl work/rust-production-dist/ChannelBridge.app "$root/$app_path"
ditto --norsrc --noextattr --noacl work/rust-production-dist/ChannelBridge.driver "$root/$driver_path"
xattr -cr "$root"
COPYFILE_DISABLE=1 pkgbuild --root "$root" \
    --component-plist packaging/Rust-Production-App-Components.plist \
    --identifier local.channelbridge.rust.pkg --version "$app_version" \
    --ownership recommended --install-location / "$stage/ChannelBridge-Rust.pkg"
pkgutil --expand-full "$stage/ChannelBridge-Rust.pkg" "$stage/expanded"
python3 - "$root" "$stage/expanded" <<'PYVERIFY'
import pathlib, sys
root, expanded = map(pathlib.Path, sys.argv[1:])
payload = expanded / 'Payload'
expected = {str(p.relative_to(root)) for p in root.rglob('*')}
actual = {str(p.relative_to(payload)) for p in payload.rglob('*')}
assert actual == expected, (actual - expected, expected - actual)
assert not (expanded / 'Scripts').exists(), 'Installer must not run scripts'
print('Production payload paths match staged app and driver; no installer scripts')
PYVERIFY
for path in "$app_path" "$driver_path"; do
    codesign --verify --strict "$stage/expanded/Payload/$path"
    cmp "$root/$path/Contents/MacOS/ChannelBridge" \
        "$stage/expanded/Payload/$path/Contents/MacOS/ChannelBridge"
done
cmp "$root/$app_path/Contents/Resources/ChannelBridge-source.tar.gz" \
    "$stage/expanded/Payload/$app_path/Contents/Resources/ChannelBridge-source.tar.gz"
mv "$stage/ChannelBridge-Rust.pkg" work/rust-production-dist/ChannelBridge-Rust.pkg
printf 'Production installer verified: %s\n' "$CHANNELBRIDGE_ROOT/work/rust-production-dist/ChannelBridge-Rust.pkg"
