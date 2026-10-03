#!/usr/bin/env bash
set -euo pipefail

DESKTOP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"

if [[ "${WT_MEDIA_RELEASE_PREBUILT:-0}" != "1" ]]; then
  bash "$DESKTOP_DIR/scripts/prepare-release-sidecar.sh"
  bash "$ROOT_DIR/wt-media-workspace/scripts/build-desktop-frontend.sh"
  mkdir -p "$DESKTOP_DIR/.generated"
  rm -rf "$DESKTOP_DIR/.generated/agent-config"
  cp -R "$ROOT_DIR/wt-media-agent/config_online" "$DESKTOP_DIR/.generated/agent-config"
fi

MANIFEST="$DESKTOP_DIR/target/sidecar-manifest.json"
FRONTEND="$DESKTOP_DIR/.generated/frontend"
CONFIG="$DESKTOP_DIR/.generated/agent-config/agent.toml"
[[ -s "$MANIFEST" && -s "$FRONTEND/index.html" && -s "$FRONTEND/frontend-build.json" && -s "$CONFIG" ]] || {
  echo "release inputs incomplete: sidecar manifest, stamped frontend, and Agent config are required" >&2
  exit 1
}
python3 "$DESKTOP_DIR/scripts/verify-release-inputs.py" \
  --manifest "$MANIFEST" \
  --binary-dir "$DESKTOP_DIR/src-tauri/binaries" \
  --frontend "$FRONTEND" \
  --agent-config "$CONFIG"
python3 "$DESKTOP_DIR/scripts/stage-runtime-sidecar-manifest.py" \
  --build-manifest "$MANIFEST" \
  --output "$DESKTOP_DIR/.generated/sidecar-manifest.json"
