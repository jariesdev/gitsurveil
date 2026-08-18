#!/usr/bin/env bash
#
# Renders the PNG icons from their SVG sources.
#
# The SVGs are the source of truth; the PNGs are build output that happens to
# be committed, because Tauri needs them at build time and contributors should
# not need a rasteriser to compile. Re-run after editing either SVG.
#
# Requires rsvg-convert (brew install librsvg).

set -euo pipefail

cd "$(dirname "$0")/../crates/gitsurveil-app/icons"

command -v rsvg-convert >/dev/null || {
  echo "error: rsvg-convert not found (brew install librsvg)" >&2
  exit 1
}

# App icon. 1024 is what macOS wants for the largest slot; Tauri downsamples.
rsvg-convert -w 1024 -h 1024 icon.svg -o icon.png
echo "icon.png"

# Tauri's macOS bundler builds an .icns from a fixed set of sizes and fails
# with "No matching IconType" if they are missing, so regenerate the platform
# set from the master PNG. It also emits mobile assets, which this desktop-only
# project ignores (see .gitignore).
if command -v pnpm >/dev/null 2>&1; then
  (cd ../../.. && pnpm tauri icon crates/gitsurveil-app/icons/icon.png \
      -o crates/gitsurveil-app/icons >/dev/null 2>&1)
  echo "32x32.png 128x128.png 128x128@2x.png icon.icns icon.ico"
fi

# Tray, one per severity band (specs/priority-engine.md). Idle is green to
# convey "all clear" at a glance; the rest carry escalating urgency colours.
render_tray() {
  sed "s/__COLOR__/$2/g" tray.svg > "/tmp/tray-$1.svg"
  rsvg-convert -w 512 -h 512 "/tmp/tray-$1.svg" -o "tray-$1.png"
  rm -f "/tmp/tray-$1.svg"
  echo "tray-$1.png"
}

render_tray idle     "#3fb950"
render_tray info     "#8c8c91"
render_tray normal   "#3884ff"
render_tray high     "#ff9500"
render_tray critical "#ff3b30"
