#!/usr/bin/env bash
#
# Builds gitsurveild and stages it where Tauri expects a sidecar binary, so
# the packaged app actually contains the daemon.
#
# Without this the installer ships the menubar app alone and the popover says
# "the service isn't running" forever, with no daemon anywhere to start.
#
# Tauri requires sidecars be suffixed with the Rust target triple, and strips
# that suffix when bundling. Run before `tauri build`.

set -euo pipefail

cd "$(dirname "$0")/.."

# The triple the daemon is actually built for. A cross-compiled release passes
# --target, so honour that over the host.
TARGET="${1:-}"
if [[ -z "$TARGET" ]]; then
  TARGET=$(rustc -vV | sed -n 's/^host: //p')
fi

DEST="crates/gitsurveil-app/binaries"
mkdir -p "$DEST"

echo "building gitsurveild for $TARGET"
if [[ -n "${1:-}" ]]; then
  cargo build --release -p gitsurveild --target "$TARGET"
  BUILT="target/$TARGET/release/gitsurveild"
else
  cargo build --release -p gitsurveild
  BUILT="target/release/gitsurveild"
fi

EXT=""
[[ "$TARGET" == *windows* ]] && EXT=".exe"
[[ -n "$EXT" ]] && BUILT="$BUILT$EXT"

cp "$BUILT" "$DEST/gitsurveild-$TARGET$EXT"
echo "staged $DEST/gitsurveild-$TARGET$EXT"
