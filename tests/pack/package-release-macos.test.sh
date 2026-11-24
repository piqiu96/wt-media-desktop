#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEMP_DIR="$(mktemp -d /private/tmp/wt-media-package-test.XXXXXX)"
trap 'rm -rf "$TEMP_DIR"' EXIT

case "$(uname -m)" in
  arm64|aarch64) ARCH="aarch64" ;;
  x86_64) ARCH="x64" ;;
  *) echo "unsupported test architecture: $(uname -m)" >&2; exit 1 ;;
esac

# A tiny source checkout proves the release name comes from the product Tag,
# independently of the base component version, without touching real source tags.
FIXTURE="$TEMP_DIR/desktop"
mkdir -p "$FIXTURE/scripts/dev" "$FIXTURE/src-tauri"
cp "$DESKTOP_DIR/scripts/package-release-macos.sh" "$FIXTURE/scripts/"
cp "$DESKTOP_DIR/scripts/dev/desktop_version.py" "$FIXTURE/scripts/dev/"
cp "$DESKTOP_DIR/src-tauri/tauri.conf.json" "$FIXTURE/src-tauri/"
git -C "$FIXTURE" init -q
git -C "$FIXTURE" add .
git -C "$FIXTURE" -c user.name=Test -c user.email=test@example.invalid commit -qm fixture
git -C "$FIXTURE" tag v0.1.0-rc.9
export WT_MEDIA_PRODUCT_TAG=v9.8.7
export WT_MEDIA_DESKTOP_SOURCE_TAG=v0.1.0-rc.9
VERSION=9.8.7
APP_NAME="$(node -p 'require(process.argv[1]).productName' "$DESKTOP_DIR/src-tauri/tauri.conf.json")"
EXPECTED_DIR="$TEMP_DIR/${APP_NAME}_${VERSION}_macos-${ARCH}"

OUTPUT="$(bash "$FIXTURE/scripts/package-release-macos.sh" --dry-run --output-dir "$TEMP_DIR")"
grep -Fqx "release-dir=$EXPECTED_DIR" <<<"$OUTPUT"
grep -Fqx "archive=$TEMP_DIR/${APP_NAME}_${VERSION}_macos-${ARCH}.zip" <<<"$OUTPUT"
