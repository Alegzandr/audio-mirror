#!/usr/bin/env sh
# Builds the two FFmpeg libraries OBS's resampler needs (libavutil and
# libswresample) as static libraries, into third_party/ffmpeg/<target>.
#
# Usage: scripts/build-ffmpeg.sh <rust-target> [extra configure flags]
set -eu

VERSION=9.0.1
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
  curl -fsSL "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz" -o "$ARCHIVE"
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
