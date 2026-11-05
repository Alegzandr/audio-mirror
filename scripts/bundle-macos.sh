#!/usr/bin/env bash
# Builds "Audio Mirror.app", a universal (arm64 + x86_64) bundle, into <out>.
# Usage: bundle-macos.sh <version> <out>
# The bundle is signed ad hoc, as the Tauri bundler did without an identity.
set -euo pipefail
version=$1 out=$2
export MACOSX_DEPLOYMENT_TARGET=13.0

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked --manifest-path app/Cargo.toml --target "$target"
done

app="$out/Audio Mirror.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/audio-mirror" \
  app/target/aarch64-apple-darwin/release/audio-mirror \
  app/target/x86_64-apple-darwin/release/audio-mirror
cp app/icons/icon.icns "$app/Contents/Resources/icon.icns"
sed "s/@VERSION@/$version/g" app/macos/Info.plist > "$app/Contents/Info.plist"
codesign --force --deep --sign - "$app"
