#!/usr/bin/env bash
# Draws the Windows installer's artwork from assets/logo.svg: the header
# (150x57) and the welcome/finish sidebar (164x314), as the 24-bit BMPs NSIS
# wants, into gui/src-tauri/packaging/windows/. Needs ImageMagick 7 with
# librsvg and the Noto Sans fonts (it stops without them, so the artwork
# stays the same). Run it again after a logo change.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/gui/src-tauri/packaging/windows"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# fc-match falls back to another font rather than fail: check the family.
for style in bold regular; do
  if [ "$(fc-match -f '%{family[0]}' "Noto Sans:$style")" != "Noto Sans" ]; then
    echo "nsis-artwork.sh: the Noto Sans font is missing" >&2
    exit 1
  fi
done
bold="$(fc-match -f '%{file}' 'Noto Sans:bold')"
regular="$(fc-match -f '%{file}' 'Noto Sans:regular')"
ink='#1A1A19'
yellow='#FFD23F'

# RSVG: librsvg, not ImageMagick's own renderer (which draws the strokes a
# little differently); fails if ImageMagick was built without it.
magick -background none -density 600 "RSVG:$root/assets/logo.svg" -resize 96x96 "$tmp/logo96.png"
magick -background none -density 600 "RSVG:$root/assets/logo.svg" -resize 44x44 "$tmp/logo44.png"

# The sidebar: the envelope on the logo's yellow, the name and what it does.
magick -size 164x314 "xc:$yellow" "$tmp/logo96.png" -gravity north -geometry +0+70 -composite \
  -font "$bold" -fill "$ink" -pointsize 26 -annotate +0+182 'tmpmail' \
  -font "$regular" -pointsize 12 -annotate +0+220 'Disposable email,' \
  -annotate +0+237 'live mail, one-click codes' \
  -alpha off -type TrueColor "BMP3:$out/nsis-sidebar.bmp"

# The header, on the page's white: the name and the logo tile.
magick -size 150x57 xc:white "$tmp/logo44.png" -gravity east -geometry +8+0 -composite \
  -font "$bold" -fill "$ink" -pointsize 18 -gravity west -annotate +8+0 'tmpmail' \
  -alpha off -type TrueColor "BMP3:$out/nsis-header.bmp"

echo "wrote $out/nsis-sidebar.bmp and $out/nsis-header.bmp"
