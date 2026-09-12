#!/bin/bash
# Atomic release build (R13): build → verify → stage → manifest → swap.
#
# A failed build NEVER touches the existing final artifact. The DMG is built
# and verified under a staging name, the manifest is generated from the staged
# file, and only after every check passes is the previous DMG moved aside and
# the new one renamed into place.
#
# Tauri's default ad-hoc signature on this toolchain fails
# `codesign --verify --deep --strict` with "code has no resources but
# signature indicates they must be present" (F15), so the .app is re-signed
# before packaging.
#
# This produces a local ad-hoc trust chain — fine for personal use, NOT
# Developer ID or notarization. Those need a separate, explicitly
# authorized step.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$REPO_ROOT/monitor-app"

VERSION="$(grep -m1 '^version' src-tauri/Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
ARCH="$(uname -m)"
APP="src-tauri/target/release/bundle/macos/monitor.app"
DMG_DIR="src-tauri/target/release/bundle/dmg"
STAGING_DMG="$DMG_DIR/.monitor_${VERSION}_${ARCH}.staging.dmg"
FINAL_DMG="$DMG_DIR/monitor_${VERSION}_${ARCH}.dmg"
PREV_DMG="$DMG_DIR/monitor_${VERSION}_${ARCH}.prev.dmg"
MANIFEST="$DMG_DIR/monitor_${VERSION}_${ARCH}.manifest.json"

echo "== building (app) =="
npm run tauri build -- --bundles app

echo "== repairing ad-hoc signature =="
codesign --force --deep --sign - "$APP"

echo "== verifying signature (strict) =="
codesign --verify --deep --strict "$APP"
echo "signature OK"

echo "== packaging DMG into staging =="
mkdir -p "$DMG_DIR"
rm -f "$STAGING_DMG"
hdiutil create -volname "monitor" -srcfolder "$APP" -ov -format UDZO "$STAGING_DMG" >/dev/null

echo "== verifying staged DMG =="
hdiutil verify "$STAGING_DMG" | tail -1

echo "== generating manifest from staged artifact =="
python3 "$REPO_ROOT/scripts/gen-manifest.py" "$STAGING_DMG" "$MANIFEST"

echo "== atomic swap into final location =="
# Keep the previous final DMG for rollback; only replace after staging verified.
if [ -f "$FINAL_DMG" ]; then
  mv -f "$FINAL_DMG" "$PREV_DMG"
  echo "previous DMG kept as: $(basename "$PREV_DMG")"
fi
mv -f "$STAGING_DMG" "$FINAL_DMG"

echo "== sha256 (final) =="
shasum -a 256 "$FINAL_DMG"

echo "done: $FINAL_DMG"
echo "manifest: $MANIFEST"
