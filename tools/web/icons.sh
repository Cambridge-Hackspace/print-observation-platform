#!/bin/sh
# Render the app's icons (#60) from web/icons/icon.svg into web/public, which
# Vite copies into the build as they are. Run after changing the icon; the
# PNGs are committed, so a build needs no image tools. Needs rsvg-convert.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$ROOT/web/icons/icon.svg"
OUT="$ROOT/web/public"
for size in 180 192 512; do
  rsvg-convert -w "$size" -h "$size" "$SRC" -o "$OUT/icon-$size.png"
done
cp "$SRC" "$OUT/icon.svg"
