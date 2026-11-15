#!/bin/bash
# Build the selected profile locally; never install it or reload the audio service.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
features=()
name=ChannelBridgeRustTest
profile=Test
output_directory=work/rust-dist
case "${1:-}" in
    "") ;;
    --production)
        features=(--features production)
        name=ChannelBridge
        profile=Production
        output_directory=work/rust-production-dist
        export CARGO_TARGET_DIR="$CHANNELBRIDGE_ROOT/work/rust-production-target"
        ;;
    *) printf 'Unknown build option\n' >&2; exit 2 ;;
esac
if (( $# > 1 )); then printf 'Too many build options\n' >&2; exit 2; fi
artifact_directory="${CARGO_TARGET_DIR:-$CHANNELBRIDGE_ROOT/target}"
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --locked --release -p channelbridge-driver --target aarch64-apple-darwin ${features[@]+"${features[@]}"}
bundle="$CHANNELBRIDGE_ROOT/$output_directory/$name.driver"
mkdir -p "$bundle/Contents/MacOS"
cp "packaging/Rust-$profile-Driver-Info.plist" "$bundle/Contents/Info.plist"
plutil -lint "$bundle/Contents/Info.plist"
cp "$artifact_directory/aarch64-apple-darwin/release/libchannelbridge_driver.dylib" "$bundle/Contents/MacOS/$name"
install_name_tool -id "@loader_path/$name" "$bundle/Contents/MacOS/$name"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"
if [[ "$(lipo -archs "$bundle/Contents/MacOS/$name")" != arm64 ]]; then
    echo "Expected an arm64-only executable in $bundle" >&2
    exit 1
fi
mkdir -p work/test/rust
test_options=()
if [[ "$profile" == Production ]]; then test_options=(-DCHANNELBRIDGE_PRODUCTION); fi
clang -arch arm64 -g -fsanitize=address,undefined ${test_options[@]+"${test_options[@]}"} scripts/test_rust_driver.c \
    -framework CoreAudio -framework CoreFoundation -o work/test/rust/driver
work/test/rust/driver "$bundle" --bundle
printf '%s driver built: %s\n' "$profile" "$bundle"
