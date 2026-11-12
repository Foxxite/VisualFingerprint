#!/usr/bin/env bash
# Put ffmpeg and ffprobe where the installer build packs them: crates/vfp-app/ffmpeg/.
# Use a GPL build with libx264, libmp3lame and libopus (for example BtbN or Gyan win64-gpl).
# Usage: scripts/fetch-desktop-ffmpeg.sh <folder that holds ffmpeg[.exe] and ffprobe[.exe]>
# Then build: cargo tauri build --config tauri.ffmpeg.conf.json   (run in crates/vfp-app)
set -euo pipefail
SRC="${1:?Give the folder that holds ffmpeg and ffprobe}"
OUT="$(cd "$(dirname "$0")/.." && pwd)/crates/vfp-app/ffmpeg"
mkdir -p "$OUT"
n=0
for f in ffmpeg ffprobe; do
  for x in "" .exe; do
    if [ -f "$SRC/$f$x" ]; then cp "$SRC/$f$x" "$OUT/$f$x"; echo "copied $f$x"; n=$((n+1)); fi
  done
done
[ "$n" -ge 2 ] || { echo "ffmpeg and ffprobe not found in $SRC" >&2; exit 1; }
"$OUT/ffmpeg"* -hide_banner -encoders 2>/dev/null | grep -qE "libx264 " || { echo "This build has no libx264. Stop." >&2; exit 1; }
