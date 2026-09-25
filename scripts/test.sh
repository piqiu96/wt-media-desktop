#!/usr/bin/env bash
set -euo pipefail

# This repo is the Tauri shell: the frontend is built in `wt-media-cloud/web`
# and arrives here as `.generated/frontend/`. There is no `package.json`, so the
# test surface is the Rust workspace plus the shell suites under `tests/`.
DESKTOP_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BINARIES_DIR="$DESKTOP_DIR/src-tauri/binaries"

# `src-tauri/tauri.conf.json` declares `externalBin: ["binaries/wt-media-agent"]`
# and Tauri's build script refuses to compile when that path is absent — so a
# clean clone cannot run `cargo test` at all until something is there.
#
# The real file is written by `scripts/prepare-release-sidecar.sh`, which needs
# a sibling `wt-media-agent` checkout and its build venv. Needing those to run a
# unit test is the wrong dependency: the suite does not read the sidecar's
# *contents*. `the_bundled_sidecar_is_resolved_from_the_app_and_checked_against_its_record`
# is `#[ignore]`d and pins the packaged direction from inside a fabricated
# bundle; what a run here needs is the path, not the binary. Measured both ways
# (real sidecar vs placeholder) the suite reads `377 tests / 372 passed /
# 0 failed / 5 ignored` either way.
#
# So: a placeholder, written only when the path is missing, loud about itself,
# and executable-but-failing so nothing can mistake it for the Agent. It cannot
# reach a release: `build-release-macos.sh` runs `release-versions.sh --check`,
# which refuses any package without `target/sidecar-manifest.json` — a file only
# the real build writes.
host_triple() {
  local triple
  triple="$(rustc -vV 2>/dev/null | awk '/^host:/ {print $2}')"
  if [[ -z "$triple" ]]; then
    case "$(uname -m)" in
      arm64|aarch64) triple="aarch64-apple-darwin" ;;
      x86_64) triple="x86_64-apple-darwin" ;;
    esac
  fi
  printf '%s' "$triple"
}

TRIPLE="$(host_triple)"
SIDECAR="$BINARIES_DIR/wt-media-agent-${TRIPLE:-aarch64-apple-darwin}"
if [[ ! -e "$SIDECAR" ]]; then
  echo "--- no sidecar at ${SIDECAR#"$DESKTOP_DIR"/}; writing a placeholder"
  echo "    the Tauri build script only needs the path to exist; the real binary"
  echo "    comes from scripts/prepare-release-sidecar.sh (sibling wt-media-agent)."
  mkdir -p "$BINARIES_DIR"
  printf '#!/bin/sh\n# Desktop test placeholder -- NOT the Local Agent.\nexit 1\n' > "$SIDECAR"
  chmod +x "$SIDECAR"
fi

cd "$DESKTOP_DIR"
cargo test --workspace

# The release-surface checks are shell scripts, not Rust tests: they exercise the
# packaging scripts against trees they fabricate themselves, so they need no
# sibling checkout and can run here.
for suite in "$DESKTOP_DIR"/tests/*.test.sh; do
  echo "--- $(basename "$suite")"
  bash "$suite"
done
