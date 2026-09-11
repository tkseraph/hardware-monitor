#!/bin/bash
# Build the release app, repair its signature, then package the DMG.
#
# Tauri's default ad-hoc signature on this toolchain fails
# `codesign --verify --deep --strict` with "code has no resources but
# signature indicates they must be present" (F15), and the DMG bundler
# cleans up the intermediate .app. So the order is: full build, repair the
# .app signature, then package the DMG from the signed .app with hdiutil.
#
# This produces a local ad-hoc trust chain — fine for personal use, NOT
# Developer ID or notarization. Those need a separate, explicitly
# authorized step.
set -euo pipefail

cd "$(dirname "$0")/../monitor-app"

APP="src-tauri/target/release/bundle/macos/monitor.app"
DMG_DIR="src-tauri/target/release/bundle/dmg"
DMG="$DMG_DIR/monitor_0.1.0_aarch64.dmg"

echo "== building (app) =="
npm run tauri build -- --bundles app

echo "== repairing ad-hoc signature =="
codesign --force --deep --sign - "$APP"

echo "== verifying signature (strict) =="
codesign --verify --deep --strict "$APP"
echo "signature OK"

echo "== packaging DMG from signed app =="
mkdir -p "$DMG_DIR"
rm -f "$DMG"
hdiutil create -volname "monitor" -srcfolder "$APP" -ov -format UDZO "$DMG" >/dev/null

echo "== verifying DMG =="
hdiutil verify "$DMG" | tail -1

echo "== sha256 =="
shasum -a 256 "$DMG"

echo "done"
