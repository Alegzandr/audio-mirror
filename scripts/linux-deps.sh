#!/usr/bin/env sh
# Libraries needed by Tauri 2, PulseAudio and the FFmpeg build on Ubuntu.
set -eu
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential file \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libssl-dev \
  libpulse-dev \
  xz-utils
