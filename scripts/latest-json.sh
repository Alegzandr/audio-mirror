#!/usr/bin/env bash
# Produit le latest.json lu par l'updater de Tauri.
# Usage : latest-json.sh <dossier> <version> <owner/repo> <tag>
set -euo pipefail
dir=$1 version=$2 repo=$3 tag=$4
base="https://github.com/${repo}/releases/download/${tag}"

entry() {
  local file=$1
  [ -f "$dir/$file" ] && [ -f "$dir/$file.sig" ] || {
    echo "Fichier ou signature manquant : $file" >&2
    exit 1
  }
  jq -n --arg url "$base/$file" --rawfile sig "$dir/$file.sig" '{url: $url, signature: $sig}'
}

win=$(entry "Audio-Mirror_${version}_windows_x64_portable.exe")
linux=$(entry "Audio-Mirror_${version}_linux_x86_64.AppImage")
mac=$(entry "Audio-Mirror_${version}_macos_universal.app.tar.gz")

jq -n \
  --arg version "$version" \
  --arg notes "https://github.com/${repo}/releases/tag/${tag}" \
  --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson win "$win" --argjson linux "$linux" --argjson mac "$mac" \
  '{
    version: $version,
    notes: $notes,
    pub_date: $date,
    platforms: {
      "windows-x86_64": $win,
      "linux-x86_64": $linux,
      "darwin-x86_64": $mac,
      "darwin-aarch64": $mac
    }
  }'
