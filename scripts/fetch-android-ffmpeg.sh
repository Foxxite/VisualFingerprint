#!/usr/bin/env bash
# Fetch the Android (arm64) ffmpeg and ffprobe programs and put them where Gradle packs them.
# Source: https://github.com/hzw1199/Android-FFmpeg-Prebuilt (FFmpeg 9.0, LGPL-2.1, no x264).
# The programs ship in the APK as libffmpeg.so and libffprobe.so, so Android extracts them
# into the app's native library folder, the one place an app may run programs from.
set -euo pipefail

OUT="$(cd "$(dirname "$0")/.." && pwd)/crates/vfp-app/android-ffmpeg/arm64-v8a"
REPO="hzw1199/Android-FFmpeg-Prebuilt"
DIR="ffmpeg-9.0/bin"
SHA_FFMPEG="9085507b0dc32643b4d6d084a7e7d3469ef17907a7ba15c22d3997ed09c932aa"
SHA_FFPROBE="d8e929fcb2b3b5a1a7c8dc234f6d98834eaf8ff2ce408a85060a470b243e0bb5"

mkdir -p "$OUT"
fetch() { # name, libname, sha
  local f="$OUT/$2"
  if [ -f "$f" ] && [ "$(sha256sum "$f" | cut -d' ' -f1)" = "$3" ]; then echo "$2 is up to date"; return; fi
  gh api -H "Accept: application/vnd.github.raw" "repos/$REPO/contents/$DIR/$1" > "$f.part"
  if [ "$(sha256sum "$f.part" | cut -d' ' -f1)" != "$3" ]; then
    rm -f "$f.part"; echo "Checksum of $1 does not match. Stop." >&2; exit 1
  fi
  mv "$f.part" "$f"; echo "got $2"
}
fetch ffmpeg libffmpeg.so "$SHA_FFMPEG"
fetch ffprobe libffprobe.so "$SHA_FFPROBE"
