#!/usr/bin/env bash
set -euo pipefail

# This repo is the Tauri shell: the frontend is built in `wt-media-cloud/web`
# and arrives here as `.generated/frontend/`. There is no `package.json`, so the
# test surface is the Rust workspace.
#
# The sibling scripts in this directory still invoke `npm`/`vite` from the M0
# layout. That is a known, registered mismatch (see CHG-20260923-056, T-06),
# not something this file can fix on its own.
cargo test --workspace
