#!/usr/bin/env bash
# Package Fotaro for macOS: Fotaro.app in a disk image, named as the in-app updater looks
# for it: target/installer/Fotaro-<version>-macos-<arch>.dmg.
#
#   installer/macos.sh            (after: cargo build --release -p fotaro)
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(cargo metadata --no-deps --format-version 1 | python3 -c \
  'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="fotaro"))')
arch=$(uname -m)
[ "$arch" = arm64 ] && arch=aarch64
out=target/installer
stage=target/installer/macos
rm -rf "$stage"
app="$stage/Fotaro.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$out"

cp target/release/fotaro "$app/Contents/MacOS/fotaro"
cp LICENSE "$app/Contents/Resources/LICENSE.txt"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Fotaro</string>
  <key>CFBundleDisplayName</key><string>Fotaro</string>
  <key>CFBundleIdentifier</key><string>io.github.jboneng.fotaro</string>
  <key>CFBundleExecutable</key><string>fotaro</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.photography</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
# Not notarised: signed ad hoc, so it runs after "Open" from the context menu once.
codesign --force --deep --sign - "$app"

ln -s /Applications "$stage/Applications"
dmg="$out/Fotaro-$version-macos-$arch.dmg"
rm -f "$dmg"
hdiutil create -volname "Fotaro $version" -srcfolder "$stage" -ov -format UDZO "$dmg"
echo "Installer: $dmg"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "version=$version" >> "$GITHUB_OUTPUT"
  echo "installer=$dmg" >> "$GITHUB_OUTPUT"
fi
