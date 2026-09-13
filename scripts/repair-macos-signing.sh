#!/usr/bin/env bash
set -euo pipefail

APP_PATH="${1:?usage: repair-macos-signing.sh /path/to/WT Media.app}"
SIDECAR_PATH="$APP_PATH/Contents/MacOS/wt-media-agent"

test -x "$SIDECAR_PATH"

# PyInstaller embeds a linker-signed libpython. Tauri's hardened-runtime
# signature on the sidecar makes macOS 26 reject that nested library. Keep the
# outer app hardened, but use a plain ad-hoc signature for the child sidecar.
/usr/bin/codesign --remove-signature "$SIDECAR_PATH"
/usr/bin/codesign --force --sign - "$SIDECAR_PATH"
/usr/bin/codesign --force --sign - --options runtime "$APP_PATH"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$APP_PATH"
