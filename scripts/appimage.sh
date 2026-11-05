#!/usr/bin/env bash
# Packs the release build of app/ into an AppImage at <out>.
# Usage: appimage.sh <out>
# appimagetool and the AppImage runtime are pinned by version and checksum.
set -euo pipefail
out=$1

tools=$(mktemp -d)
fetch() {
  curl -fsSL --retry 3 -o "$tools/$2" "$1"
  echo "$3  $tools/$2" | sha256sum -c -
}
fetch https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
  appimagetool ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
fetch https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
  runtime 2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
chmod +x "$tools/appimagetool"

dir="$tools/AppDir"
mkdir -p "$dir/usr/bin" "$dir/usr/share/applications" "$dir/usr/share/icons/hicolor/128x128/apps"
cp app/target/release/audio-mirror "$dir/usr/bin/audio-mirror"
cp app/icons/128x128.png "$dir/usr/share/icons/hicolor/128x128/apps/audio-mirror.png"
cp app/icons/128x128.png "$dir/audio-mirror.png"
cat > "$dir/audio-mirror.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Audio Mirror
Comment=Mirrors one audio source to several outputs
Exec=audio-mirror
Icon=audio-mirror
Categories=AudioVideo;Audio;Utility;
Terminal=false
DESKTOP
cp "$dir/audio-mirror.desktop" "$dir/usr/share/applications/"
cat > "$dir/AppRun" <<'APPRUN'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/audio-mirror" "$@"
APPRUN
chmod +x "$dir/AppRun"

ARCH=x86_64 "$tools/appimagetool" --appimage-extract-and-run \
  --runtime-file "$tools/runtime" "$dir" "$out"
