#!/usr/bin/env bash
set -euo pipefail

# Stage the Agent's release configuration into a built app bundle (ADR-0016 §6:
# `config_online/` reaches the product as `产物/config`).
#
# Runs *after* `cargo tauri build` and *before* `repair-macos-signing.sh`, and the
# order is the point: `Contents/Resources` is sealed by the outer signature, so a
# file added after that signature would invalidate it. That is the same reason
# the sidecar manifest is written there, and this placement is measured the same
# way (`repair-macos-signing.sh` writes the record, then signs, then verifies
# with `--deep --strict`).
#
# It is not a `bundle.resources` entry even though Tauri would carry one: Tauri
# refuses a resources glob that matches no file ("glob pattern config/* path not
# found or didn't match any files"), so declaring `config/*` would break
# `cargo build`/`cargo test` in any checkout where the release step has not run.
# A release step belongs in the release path, not in everyone's compile.
#
# The copy is wholesale and read back (`build_desktop_sidecar.py::sync_config`),
# and `verify-release-macos.sh` re-checks it in the DMG against `config_online/`.
#
# usage: stage-release-config.sh "/path/to/App.app"
APP_PATH="${1:?usage: stage-release-config.sh /path/to/App.app}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"
AGENT_DIR="$ROOT_DIR/wt-media-agent"
PYTHON_BIN="${WT_MEDIA_AGENT_PYTHON:-$AGENT_DIR/.venv/bin/python}"

if [[ ! -x "$PYTHON_BIN" ]]; then
  echo "Local Agent build Python is unavailable: $PYTHON_BIN" >&2
  echo "Create the Agent build environment with its 'build' dependency before creating a release." >&2
  exit 1
fi

test -d "$APP_PATH/Contents/Resources"

"$PYTHON_BIN" "$AGENT_DIR/scripts/build_desktop_sidecar.py" \
  --config-dir "$APP_PATH/Contents/Resources/config"
