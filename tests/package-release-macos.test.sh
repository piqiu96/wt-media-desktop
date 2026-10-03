#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TEMP_DIR="$(mktemp -d /private/tmp/wt-media-package-test.XXXXXX)"
trap 'rm -rf "$TEMP_DIR"' EXIT

case "$(uname -m)" in
  arm64|aarch64) ARCH="aarch64" ;;
  x86_64) ARCH="x64" ;;
  *) echo "unsupported test architecture: $(uname -m)" >&2; exit 1 ;;
esac

VERSION="$(node -p 'require(process.argv[1]).version' "$DESKTOP_DIR/src-tauri/tauri.conf.json")"
APP_NAME="$(node -p 'require(process.argv[1]).productName' "$DESKTOP_DIR/src-tauri/tauri.conf.json")"
EXPECTED_DIR="$TEMP_DIR/${APP_NAME}_${VERSION}_macos-${ARCH}"

OUTPUT="$(bash "$DESKTOP_DIR/scripts/package-release-macos.sh" --dry-run --output-dir "$TEMP_DIR")"
grep -Fqx "release-dir=$EXPECTED_DIR" <<<"$OUTPUT"
grep -Fqx "archive=$TEMP_DIR/${APP_NAME}_${VERSION}_macos-${ARCH}.zip" <<<"$OUTPUT"
