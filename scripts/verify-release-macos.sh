#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS release verification must run on macOS." >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
case "$(uname -m)" in
  arm64) artifact_arch="aarch64" ;;
  x86_64) artifact_arch="x64" ;;
  *) echo "Unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
esac
version="$(cd "$DESKTOP_DIR" && node -p 'require("./src-tauri/tauri.conf.json").version')"
dmg_path="$DESKTOP_DIR/target/release/bundle/dmg/WT Media_${version}_${artifact_arch}.dmg"
if [[ ! -f "$dmg_path" ]]; then
  echo "Expected DMG is missing: $dmg_path" >&2
  exit 1
fi

mount_dir="$(mktemp -d /private/tmp/wt-media-dmg.XXXXXX)"
cleanup() {
  hdiutil detach "$mount_dir" >/dev/null 2>&1 || true
  rmdir "$mount_dir" >/dev/null 2>&1 || true
}
trap cleanup EXIT
hdiutil attach -nobrowse -readonly -mountpoint "$mount_dir" "$dmg_path" >/dev/null
codesign --verify --deep --strict --verbose=2 "$mount_dir/WT Media.app"
echo "macOS DMG contains a complete ad-hoc-signed app: $dmg_path"
