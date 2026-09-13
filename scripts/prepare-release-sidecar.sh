#!/usr/bin/env bash
set -euo pipefail

# Tauri runs beforeBuildCommand from the Desktop repository root.
DESKTOP_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ROOT_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"
AGENT_DIR="$ROOT_DIR/wt-media-agent"
PYTHON_BIN="${WT_MEDIA_AGENT_PYTHON:-$AGENT_DIR/.venv/bin/python}"

if [[ ! -x "$PYTHON_BIN" ]]; then
  echo "Local Agent build Python is unavailable: $PYTHON_BIN" >&2
  echo "Create the Agent build environment with its 'build' dependency before creating a release." >&2
  exit 1
fi

"$PYTHON_BIN" "$AGENT_DIR/scripts/build_desktop_sidecar.py" \
  --output-dir "$DESKTOP_DIR/src-tauri/binaries" \
  --manifest "$DESKTOP_DIR/target/sidecar-manifest.json"
