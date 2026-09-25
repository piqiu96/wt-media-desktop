#!/usr/bin/env bash
set -euo pipefail

# This repo is the Tauri shell: the frontend is built in `wt-media-cloud/web`
# and arrives here as `.generated/frontend/`. There is no `package.json`, so the
# test surface is the Rust workspace.
#
# The sibling scripts in this directory still invoke `npm`/`vite` from the M0
# layout. That is a known, registered mismatch (see CHG-20260923-056, T-06),
# not something this file can fix on its own.
DESKTOP_DIR="$(cd "$(dirname "$0")/.." && pwd)"

cargo test --workspace

# The release-surface checks are shell scripts, not Rust tests: they exercise the
# packaging scripts against trees they fabricate themselves, so they need no
# sibling checkout and can run here.
for suite in "$DESKTOP_DIR"/tests/*.test.sh; do
  echo "--- $(basename "$suite")"
  bash "$suite"
done
