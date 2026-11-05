#!/usr/bin/env sh
# Libraries needed by GPUI, PulseAudio and the FFmpeg build on Ubuntu.
set -eu
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential file pkg-config \
  libfontconfig-dev \
  libfreetype-dev \
  libwayland-dev \
  libx11-xcb-dev \
  libxkbcommon-dev \
  libxkbcommon-x11-dev \
  libvulkan-dev \
  libzstd-dev \
  libpulse-dev \
  xz-utils
