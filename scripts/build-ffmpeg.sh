#!/usr/bin/env sh
# Builds the two FFmpeg libraries OBS's resampler needs (libavutil and
# libswresample) as static libraries, into third_party/ffmpeg/<target>.
#
# Usage: scripts/build-ffmpeg.sh <rust-target> [extra configure flags]
set -eu

VERSION=9.0.1
# Checked after download: the archive is only trusted if it matches.
SHA256=cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635
TARGET=${1:?rust target triple}
shift

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX="$ROOT/third_party/ffmpeg/$TARGET"
WORK="$ROOT/third_party/build/ffmpeg-$VERSION-$TARGET"

if [ -f "$PREFIX/lib/libswresample.a" ]; then
  echo "FFmpeg already built in $PREFIX"
  exit 0
fi

mkdir -p "$ROOT/third_party/build"
ARCHIVE="$ROOT/third_party/build/ffmpeg-$VERSION.tar.xz"
if [ ! -f "$ARCHIVE" ]; then
  curl -fsSL --proto '=https' --tlsv1.2 "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz" -o "$ARCHIVE.part"
  mv "$ARCHIVE.part" "$ARCHIVE"
fi
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL=$(sha256sum "$ARCHIVE" | cut -d ' ' -f1)
else
  ACTUAL=$(shasum -a 256 "$ARCHIVE" | cut -d ' ' -f1)
fi
if [ "$ACTUAL" != "$SHA256" ]; then
  echo "Checksum mismatch for $ARCHIVE (got $ACTUAL)" >&2
  rm -f "$ARCHIVE"
  exit 1
fi
rm -rf "$WORK"
mkdir -p "$WORK"
tar -xf "$ARCHIVE" -C "$WORK" --strip-components=1

cd "$WORK"
./configure \
  --prefix="$PREFIX" \
  --enable-static --disable-shared --enable-pic \
  --disable-programs --disable-doc --disable-network --disable-autodetect \
  --disable-everything \
  --disable-avcodec --disable-avformat --disable-avdevice --disable-avfilter \
  --disable-swscale \
  --enable-swresample \
  --disable-x86asm \
  "$@"
make -j"$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
make install
echo "FFmpeg installed in $PREFIX"
