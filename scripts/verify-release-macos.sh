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

# The Agent's configuration, checked in the artifact rather than in the source
# tree (ADR-0016 §6: `config_online/` reaches the product as `产物/config`, and
# the frozen Agent reads it from `Contents/Resources/config`, where
# scripts/stage-release-config.sh put it). `diff -r` is the judgement the ADR asks
# for; the counts around it keep the check from passing on two empty directories,
# and the file-missing arm is separate so a release that lost the step says which
# step it lost.
root_dir="$(cd "$DESKTOP_DIR/.." && pwd)"
agent_config_dir="$root_dir/wt-media-agent/config_online"
product_config_dir="$mount_dir/WT Media.app/Contents/Resources/config"
expected_files="$(find "$agent_config_dir" -type f | wc -l | tr -d ' ')"
if [[ "$expected_files" -eq 0 ]]; then
  echo "config_online/ carries no files, so a mirror of it would prove nothing: $agent_config_dir" >&2
  exit 1
fi
if [[ ! -d "$product_config_dir" ]]; then
  echo "The packaged app carries no Agent configuration: $product_config_dir is missing." >&2
  echo "build-release-macos.sh must run scripts/stage-release-config.sh on the app before signing it." >&2
  exit 1
fi
if ! diff -r "$agent_config_dir" "$product_config_dir"; then
  echo "The packaged Agent configuration is not a mirror of $agent_config_dir" >&2
  exit 1
fi
echo "Agent configuration in the DMG is a ${expected_files}-file mirror of config_online/: $product_config_dir"

# The five version classes, checked in the shipped artifact rather than in the
# source tree: `--verify` recomputes the digest of the mounted app's resources and
# compares it with the record the app carries, so a DMG assembled from a tree that
# moved on after the build says so here instead of after installation.
bash "$SCRIPT_DIR/release-versions.sh" --verify "$mount_dir/WT Media.app"

echo "macOS DMG contains a complete ad-hoc-signed app: $dmg_path"
