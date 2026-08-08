#!/usr/bin/env bash
# Stages RSSSTV.app with an ad-hoc signature and wraps it in a drag-and-drop
# disk image. macOS only: sips, iconutil, codesign, and hdiutil.
# Usage: build-app.sh <rssstv-binary> <help-directory> <licenses.html> <output.dmg>
set -euo pipefail

binary="$1"
help="$2"
licenses="$3"
dmg="$4"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)"

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT

app="$staging/dmg/RSSSTV.app"
contents="$app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"

# The icon source is 512 px, so every iconset size up to 512 is a reduction;
# icon_512x512@2x would be an upscale and is left out.
iconset="$staging/rssstv.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$root/rssstv/assets/icon.png" \
    --out "$iconset/icon_${size}x${size}.png" > /dev/null
done
for size in 16 32 128 256; do
  sips -z "$((size * 2))" "$((size * 2))" "$root/rssstv/assets/icon.png" \
    --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/rssstv.icns"

# NSMicrophoneUsageDescription is load-bearing: a bundled process that opens
# a capture device without it is killed by the system.
cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>RSSSTV</string>
  <key>CFBundleExecutable</key>
  <string>rssstv</string>
  <key>CFBundleIconFile</key>
  <string>rssstv</string>
  <key>CFBundleIdentifier</key>
  <string>org.kb10uy.rssstv</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>RSSSTV</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>RSSSTV listens to the selected capture device to receive SSTV transmissions.</string>
</dict>
</plist>
PLIST

install -m 755 "$binary" "$contents/MacOS/rssstv"
cp -R "$help" "$contents/Resources/help"
cp "$root/LICENSE" "$licenses" "$contents/Resources/"
codesign --force -s - "$app"

cp -R "$root/templates" "$staging/dmg/templates"
ln -s /Applications "$staging/dmg/Applications"
hdiutil create -volname "RSSSTV" -srcfolder "$staging/dmg" -format UDZO -ov "$dmg"
