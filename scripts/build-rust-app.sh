#!/bin/bash
# Build the selected profile locally; never install it or reload the audio service.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
features=()
name=ChannelBridgeRustTest
profile=Test
output_directory=work/rust-dist
smoke=true
for option in "$@"; do
    case "$option" in
        --build-only) smoke=false ;;
        --production)
            features=(--features production)
            name=ChannelBridge
            profile=Production
            output_directory=work/rust-production-dist
            export CARGO_TARGET_DIR="$CHANNELBRIDGE_ROOT/work/rust-production-target"
            ;;
        *) printf 'Unknown build option\n' >&2; exit 2 ;;
    esac
done
artifact_directory="${CARGO_TARGET_DIR:-$CHANNELBRIDGE_ROOT/target}"
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --locked --release -p channelbridge-app --target aarch64-apple-darwin ${features[@]+"${features[@]}"}
mkdir -p "$output_directory"
stage="$(mktemp -d "$CHANNELBRIDGE_ROOT/$output_directory/app-stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
bundle="$stage/$name.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "packaging/Rust-$profile-App-Info.plist" "$bundle/Contents/Info.plist"
plutil -lint "$bundle/Contents/Info.plist"
cp "$artifact_directory/aarch64-apple-darwin/release/channelbridge-app" "$bundle/Contents/MacOS/$name"
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
if [[ "$(lipo -archs "$bundle/Contents/MacOS/$name")" != arm64 ]]; then
    echo "Expected an arm64-only executable in $bundle" >&2
    exit 1
fi
"$bundle/Contents/MacOS/$name" --check-bundle
if [[ "$smoke" == true ]]; then
    "$bundle/Contents/MacOS/$name" --smoke --require-bundle
fi
output="$CHANNELBRIDGE_ROOT/$output_directory/$name.app"
rm -rf "$output"
mv "$bundle" "$output"
printf '%s app built: %s\n' "$profile" "$output"
