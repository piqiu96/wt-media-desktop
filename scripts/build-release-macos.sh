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

APP_PATH="$DESKTOP_DIR/target/release/bundle/macos/WT Media.app"
case "$(uname -m)" in
  arm64|aarch64) DMG_SUFFIX="aarch64" ;;
  x86_64) DMG_SUFFIX="x64" ;;
  *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
esac
DMG_PATH="$DESKTOP_DIR/target/release/bundle/dmg/WT Media_0.1.0_${DMG_SUFFIX}.dmg"
bash "$SCRIPT_DIR/repair-macos-signing.sh" "$APP_PATH"

STAGING_DIR="$(mktemp -d /private/tmp/wt-media-dmg.XXXXXX)"
trap 'rm -rf "$STAGING_DIR"' EXIT
cp -R "$APP_PATH" "$STAGING_DIR/WT Media.app"
ln -s /Applications "$STAGING_DIR/Applications"
mkdir -p "$(dirname "$DMG_PATH")"
hdiutil create -volname "WT Media" -srcfolder "$STAGING_DIR" -ov -format UDZO "$DMG_PATH" >/dev/null
bash "$SCRIPT_DIR/verify-release-macos.sh"
