#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS DMG builds must run on macOS." >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$DESKTOP_DIR"
cargo tauri build --bundles app

APP_NAME="$(node -p 'require(process.argv[1]).productName' "$DESKTOP_DIR/src-tauri/tauri.conf.json")"
APP_PATH="$DESKTOP_DIR/target/release/bundle/macos/$APP_NAME.app"
# The name is read rather than written because the app was renamed once and every
# script here kept looking for the old one. `stage-release-config.sh` reports that
# as a bare exit status under `set -e`, so the build looked like it had finished.
if [[ ! -d "$APP_PATH" ]]; then
  echo "cargo tauri build produced no app bundle at: $APP_PATH" >&2
  echo "The name comes from productName in src-tauri/tauri.conf.json." >&2
  exit 1
fi

# The release gate, and it runs here rather than earlier because the sidecar
# manifest and the frontend marker only exist once the build has produced them:
# the five version classes have to be resolvable and the packaged Agent has to be
# the one this Desktop pins *before* anything is staged, signed or packaged.
# (`--check` is what refuses a package whose sidecar reports a version the pin
# does not name -- scripts/release-versions.sh.)
bash "$SCRIPT_DIR/release-versions.sh" --check

VERSION="$(node -p 'require(process.argv[1]).version' "$DESKTOP_DIR/src-tauri/tauri.conf.json")"
case "$(uname -m)" in
  arm64|aarch64) DMG_SUFFIX="aarch64" ;;
  x86_64) DMG_SUFFIX="x64" ;;
  *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
esac
# The DMG is named after the product name and version in tauri.conf.json, the same
# way verify-release-macos.sh and package-release-macos.sh compute both. Hardcoding
# them here meant a version bump or a rename would build a DMG that verification
# then reported as missing.
DMG_PATH="$DESKTOP_DIR/target/release/bundle/dmg/${APP_NAME}_${VERSION}_${DMG_SUFFIX}.dmg"
bash "$SCRIPT_DIR/stage-release-config.sh" "$APP_PATH"
bash "$SCRIPT_DIR/repair-macos-signing.sh" "$APP_PATH"

STAGING_DIR="$(mktemp -d /private/tmp/wt-media-dmg.XXXXXX)"
trap 'rm -rf "$STAGING_DIR"' EXIT
cp -R "$APP_PATH" "$STAGING_DIR/$APP_NAME.app"
ln -s /Applications "$STAGING_DIR/Applications"
mkdir -p "$(dirname "$DMG_PATH")"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGING_DIR" -ov -format UDZO "$DMG_PATH" >/dev/null
bash "$SCRIPT_DIR/verify-release-macos.sh"
