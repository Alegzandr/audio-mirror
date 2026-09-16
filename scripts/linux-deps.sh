#!/usr/bin/env sh
# Bibliothèques nécessaires à Tauri 2 et à cpal (ALSA) sous Ubuntu.
set -eu
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential file \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libssl-dev \
  libasound2-dev
