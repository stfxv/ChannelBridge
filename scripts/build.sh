#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
BUILD="$ROOT/work/build"
DIST="$ROOT/dist"
APP="$DIST/ChannelBridge.app"
DRIVER="$BUILD/package/Library/Audio/Plug-Ins/HAL/ChannelBridge.driver"
mkdir -p "$BUILD" "$DIST" "$APP/Contents/MacOS" "$APP/Contents/Resources" "$DRIVER/Contents/MacOS"
# Compile an independent, renamed driver. Upstream source is kept verbatim.
cp vendor/blackhole/BlackHole.c "$BUILD/ChannelBridge.c"
patch -s "$BUILD/ChannelBridge.c" driver/compatibility.patch
clang -O2 -fblocks -arch arm64 -arch x86_64 -mmacosx-version-min=13.0 \
  -include "$ROOT/driver/Config.h" -bundle "$BUILD/ChannelBridge.c" \
  -framework CoreAudio -framework CoreFoundation -framework Accelerate \
  -o "$DRIVER/Contents/MacOS/ChannelBridge"
cp packaging/Driver-Info.plist "$DRIVER/Contents/Info.plist"
codesign --force --sign - "$DRIVER"
COPYFILE_DISABLE=1 pkgbuild --root "$BUILD/package" --identifier local.channelbridge.driver.pkg --version 0.1.0 \
  --component-plist packaging/Driver-Components.plist --install-location / "$BUILD/driver-component.pkg"
productbuild --distribution packaging/Distribution.xml --package-path "$BUILD" "$APP/Contents/Resources/ChannelBridgeDriver.pkg"
# The app is native to the build machine; the driver is universal.
CGO_ENABLED=1 CGO_CFLAGS='-O2 -fobjc-arc -mmacosx-version-min=13.0' go build -trimpath -o "$APP/Contents/MacOS/ChannelBridge" ./cmd/channelbridge
cp packaging/App-Info.plist "$APP/Contents/Info.plist"
# Package the transparent source icon at every standard macOS icon size.
ICONSET="$BUILD/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" packaging/assets/AppIcon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" packaging/assets/AppIcon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
cp LICENSE "$APP/Contents/Resources/LICENSE"
cp THIRD_PARTY.md "$APP/Contents/Resources/THIRD_PARTY.md"
# Include the complete corresponding source, without build products.
tar -czf "$APP/Contents/Resources/ChannelBridge-source.tar.gz" go.mod cmd internal scripts driver packaging vendor LICENSE README.md THIRD_PARTY.md VERIFICATION.md
codesign --force --sign - "$APP"
codesign --verify --strict "$APP"
echo "Built: $APP"
