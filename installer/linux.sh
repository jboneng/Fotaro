#!/usr/bin/env bash
# Package Fotaro for Linux as an AppImage (with the libraries it needs), named as the
# in-app updater looks for it: target/installer/Fotaro-<version>-linux-<arch>.AppImage.
#
#   installer/linux.sh            (after: cargo build --release -p fotaro)
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(cargo metadata --no-deps --format-version 1 | python3 -c \
  'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="fotaro"))')
arch=$(uname -m)
out=target/installer
stage=target/installer/linux
rm -rf "$stage"
mkdir -p "$stage" "$out"

cat > "$stage/fotaro.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Fotaro
Comment=Photo organiser and RAW editor
Exec=fotaro
Icon=fotaro
Categories=Graphics;Photography;
Terminal=false
EOF

# An icon is required: a plain one until Fotaro has its own.
python3 - "$stage/fotaro.png" <<'EOF'
import struct, sys, zlib
size, rgb = 256, bytes((0x6c, 0xb4, 0xe6))
raw = b"".join(b"\0" + rgb * size for _ in range(size))
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
EOF

tool="$stage/linuxdeploy-$arch.AppImage"
curl -fsSL -o "$tool" "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$arch.AppImage"
chmod +x "$tool"

appimage="$out/Fotaro-$version-linux-$arch.AppImage"
rm -f "$appimage"
export APPIMAGE_EXTRACT_AND_RUN=1 LDAI_OUTPUT="$appimage" OUTPUT="$appimage"
"$tool" --appdir "$stage/AppDir" \
  --executable target/release/fotaro \
  --desktop-file "$stage/fotaro.desktop" \
  --icon-file "$stage/fotaro.png" \
  --output appimage
echo "Installer: $appimage"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "version=$version" >> "$GITHUB_OUTPUT"
  echo "installer=$appimage" >> "$GITHUB_OUTPUT"
fi
