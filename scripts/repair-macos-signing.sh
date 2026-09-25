#!/usr/bin/env bash
set -euo pipefail

APP_PATH="${1:?usage: repair-macos-signing.sh /path/to/WT Media.app}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SIDECAR_PATH="$APP_PATH/Contents/MacOS/wt-media-agent"
RESOURCES_DIR="$APP_PATH/Contents/Resources"
PACKAGED_MANIFEST="$RESOURCES_DIR/sidecar-manifest.json"
BUILD_MANIFEST="$DESKTOP_DIR/target/sidecar-manifest.json"

test -x "$SIDECAR_PATH"
test -f "$BUILD_MANIFEST"

# PyInstaller embeds a linker-signed libpython, and macOS 26 refuses to load it
# into a process whose signature disagrees (`different Team IDs`). The hardened
# runtime flag is what makes the signatures disagree, and it is on the *build
# output* already: `scripts/build_desktop_sidecar.py` passes `--codesign-identity -`,
# and PyInstaller adds `--options=runtime` unless that argument is falsy
# (`utils/osx.py:413-421` in PyInstaller 6.22.2) -- so an identity of literally
# `-` still counts as "an identity was given". Measured: the freshly built sidecar
# carries `flags=0x10002(adhoc,runtime)` and dies at startup, while the same file
# re-signed here carries `flags=0x2(adhoc)` and runs
# (`evidence/task-05-packaging.out`, `task-05-frozen-reading.out`).
#
# Removing and re-adding the signature is therefore not cosmetic. Keep the outer
# app hardened; give the child sidecar a plain ad-hoc signature.
/usr/bin/codesign --remove-signature "$SIDECAR_PATH"
/usr/bin/codesign --force --sign - "$SIDECAR_PATH"

# The record of what shipped is written **here**, between the sidecar's signature
# and the app's, and the order is the whole point:
#
#   1. Signing rewrites the file, so a digest is only true of the sidecar *after*
#      the last write to it. A digest taken at build time describes different
#      bytes, which is why the build manifest and this one legitimately disagree
#      and why comparing them would fail every release. Measured in
#      `evidence/task-04-packaging.out`: the built file and the packaged file are
#      two different digests, and re-signing the same input is byte-for-byte
#      repeatable, so measuring here is not a race.
#   2. `Contents/Resources` is sealed by the app's signature, so a file added
#      after that signature would invalidate it; added before, it is covered by
#      the seal and `codesign --verify --deep --strict` still passes.
#
# The app reads this file before it starts the sidecar
# (`src-tauri/src/sidecar/integrity.rs`), so the `filename` here has to be the
# name the app resolves — the *runtime* name Tauri installs the sidecar under,
# not the target-qualified build filename.
mkdir -p "$RESOURCES_DIR"
PACKAGED_SIDECAR_SHA="$(/usr/bin/shasum -a 256 "$SIDECAR_PATH" | awk '{print $1}')"
SIDECAR_VERSION="$(node -p 'require(process.argv[1]).version' "$BUILD_MANIFEST")"
SIDECAR_TARGET="$(node -p 'require(process.argv[1]).target' "$BUILD_MANIFEST")"
node -e 'const fs=require("fs"); fs.writeFileSync(process.argv[1], JSON.stringify({component:"wt-media-agent",version:process.argv[2],target:process.argv[3],filename:"wt-media-agent",sha256:process.argv[4]}, null, 2) + "\n")' \
  "$PACKAGED_MANIFEST" "$SIDECAR_VERSION" "$SIDECAR_TARGET" "$PACKAGED_SIDECAR_SHA"

# The version record that names all five classes is written in the same window and
# for the same two reasons: it has to be inside the seal, and it has to be written
# after every file it describes. `--record` digests `Contents/Resources` as it
# stands, so running it here rather than in build-release-macos.sh is what makes
# the digest cover the sidecar manifest above it (`--verify` later recomputes that
# digest from the mounted DMG). The seal itself writes only `Contents/
# _CodeSignature/`, which is outside the digested directory -- a digest over the
# whole bundle could never be reproduced, because signing it changes it.
bash "$SCRIPT_DIR/release-versions.sh" --record "$APP_PATH"

/usr/bin/codesign --force --sign - --options runtime "$APP_PATH"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$APP_PATH"

echo "sidecar-manifest=$PACKAGED_MANIFEST"
echo "sidecar-sha256=$PACKAGED_SIDECAR_SHA"
