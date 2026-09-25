#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/package-release-macos.sh [--output-dir <path>] [--dry-run]

Builds the native macOS Desktop release, verifies its ad-hoc signature and
embedded Local Agent sidecar, then writes an app/DMG release folder and ZIP.
EOF
}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKSPACE_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"
OUTPUT_DIR="$WORKSPACE_DIR/output"
DRY_RUN=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output-dir)
      [[ $# -ge 2 ]] || { echo "--output-dir requires a path" >&2; exit 2; }
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS release packaging must run on macOS." >&2
  exit 1
fi

case "$(uname -m)" in
  arm64|aarch64) ARTIFACT_ARCH="aarch64" ;;
  x86_64) ARTIFACT_ARCH="x64" ;;
  *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
esac

VERSION="$(node -p 'require(process.argv[1]).version' "$DESKTOP_DIR/src-tauri/tauri.conf.json")"
PACKAGE_NAME="WT-Media_${VERSION}_macos-${ARTIFACT_ARCH}"
PACKAGE_DIR="$OUTPUT_DIR/$PACKAGE_NAME"
ARCHIVE_PATH="$OUTPUT_DIR/$PACKAGE_NAME.zip"

if "$DRY_RUN"; then
  echo "release-dir=$PACKAGE_DIR"
  echo "archive=$ARCHIVE_PATH"
  exit 0
fi

if [[ -e "$PACKAGE_DIR" || -e "$ARCHIVE_PATH" || -e "$ARCHIVE_PATH.sha256" ]]; then
  echo "release output already exists; choose another --output-dir or move the old release:" >&2
  echo "  $PACKAGE_DIR" >&2
  exit 1
fi

cd "$DESKTOP_DIR"
bash "$SCRIPT_DIR/build-release-macos.sh"

APP_PATH="$DESKTOP_DIR/target/release/bundle/macos/WT Media.app"
DMG_PATH="$DESKTOP_DIR/target/release/bundle/dmg/WT Media_${VERSION}_${ARTIFACT_ARCH}.dmg"
SIDECAR_MANIFEST="$DESKTOP_DIR/target/sidecar-manifest.json"
for required in "$APP_PATH" "$DMG_PATH" "$SIDECAR_MANIFEST"; do
  [[ -e "$required" ]] || { echo "release build omitted required artifact: $required" >&2; exit 1; }
done

SIDECAR_BUILD_FILENAME="$(node -p 'require(process.argv[1]).filename' "$SIDECAR_MANIFEST")"
SIDECAR_BUILD_SHA="$(node -p 'require(process.argv[1]).sha256' "$SIDECAR_MANIFEST")"
SIDECAR_VERSION="$(node -p 'require(process.argv[1]).version' "$SIDECAR_MANIFEST")"
SIDECAR_TARGET="$(node -p 'require(process.argv[1]).target' "$SIDECAR_MANIFEST")"
# Tauri resolves `externalBin: ["binaries/wt-media-agent"]` from the
# target-qualified build filename, then installs it under this stable runtime
# name inside the app bundle.
SIDECAR_PATH="$APP_PATH/Contents/MacOS/wt-media-agent"
[[ -f "$SIDECAR_PATH" ]] || { echo "packaged app is missing Local Agent sidecar built from: $SIDECAR_BUILD_FILENAME" >&2; exit 1; }

mkdir -p "$OUTPUT_DIR" "$PACKAGE_DIR"
cp -R "$APP_PATH" "$PACKAGE_DIR/WT Media.app"
cp "$DMG_PATH" "$PACKAGE_DIR/$(basename "$DMG_PATH")"
cp "$SIDECAR_MANIFEST" "$PACKAGE_DIR/agent-build-manifest.json"
codesign --verify --deep --strict --verbose=2 "$PACKAGE_DIR/WT Media.app"

PACKAGED_SIDECAR="$PACKAGE_DIR/WT Media.app/Contents/MacOS/wt-media-agent"
PACKAGED_MANIFEST="$PACKAGE_DIR/WT Media.app/Contents/Resources/sidecar-manifest.json"
[[ -f "$PACKAGED_MANIFEST" ]] || { echo "the packaged app carries no sidecar record: $PACKAGED_MANIFEST" >&2; exit 1; }
PACKAGED_SIDECAR_SHA="$(node -p 'require(process.argv[1]).sha256' "$PACKAGED_MANIFEST")"

# The record the app verifies against has to describe the file that actually
# shipped, and measuring it here is the only place that can be said before a
# customer's launch: the app refuses to start a sidecar that disagrees with its
# record, so a package built with a stale record would install and then not run.
MEASURED_SIDECAR_SHA="$(shasum -a 256 "$PACKAGED_SIDECAR" | awk '{print $1}')"
[[ "$MEASURED_SIDECAR_SHA" == "$PACKAGED_SIDECAR_SHA" ]] || {
  echo "the in-bundle sidecar record does not describe the packaged file:" >&2
  echo "  record=$PACKAGED_SIDECAR_SHA measured=$MEASURED_SIDECAR_SHA" >&2
  exit 1
}
node -e 'const fs=require("fs"); fs.writeFileSync(process.argv[1], JSON.stringify({component:"wt-media-agent",version:process.argv[2],target:process.argv[3],filename:"wt-media-agent",sha256:process.argv[4],build_filename:process.argv[5],build_sha256:process.argv[6]}, null, 2) + "\n")' \
  "$PACKAGE_DIR/sidecar-manifest.json" "$SIDECAR_VERSION" "$SIDECAR_TARGET" "$PACKAGED_SIDECAR_SHA" "$SIDECAR_BUILD_FILENAME" "$SIDECAR_BUILD_SHA"

# The five version classes, lifted out of the bundle so a release folder answers
# "what is in this package" without mounting anything. This is the same record the
# app carries and the same one `--verify` checks against the DMG; it is copied, not
# rewritten, so there is no second version of the truth to drift.
VERSIONS_RECORD="$APP_PATH/Contents/Resources/versions.json"
[[ -f "$VERSIONS_RECORD" ]] || { echo "the packaged app carries no version record: $VERSIONS_RECORD" >&2; exit 1; }
cp "$VERSIONS_RECORD" "$PACKAGE_DIR/versions.json"

printf '%s\n' \
  "WT Media ${VERSION} for macOS ${ARTIFACT_ARCH}" \
  '' \
  'Install using either artifact:' \
  '- Open the DMG and drag WT Media.app to Applications.' \
  '- Or copy the included WT Media.app directly.' \
  '' \
  'The package embeds a native Local Agent sidecar. No customer Python installation is required.' \
  'The app is ad-hoc signed; macOS may require the normal first-open unknown-developer confirmation.' \
  'sidecar-manifest.json records the embedded Agent version, native target, filename, and SHA-256.' \
  'The app checks its sidecar against the copy of that record inside the bundle before starting it.' \
  'versions.json records the five version classes this package was built from: the Desktop' \
  'version, the embedded Agent version, the frontend build (package version + source commit),' \
  'every cross-repository contract revision, and a digest of the components and resources' \
  'the app carries.' \
  > "$PACKAGE_DIR/README.txt"

(cd "$OUTPUT_DIR" && ditto -c -k --sequesterRsrc --keepParent "$PACKAGE_NAME" "$(basename "$ARCHIVE_PATH")")
shasum -a 256 "$ARCHIVE_PATH" > "$ARCHIVE_PATH.sha256"
shasum -a 256 "$PACKAGE_DIR/$(basename "$DMG_PATH")" "$PACKAGE_DIR/sidecar-manifest.json" "$PACKAGE_DIR/versions.json" > "$PACKAGE_DIR/SHA256SUMS"

echo "release-dir=$PACKAGE_DIR"
echo "archive=$ARCHIVE_PATH"
echo "archive-sha256=$ARCHIVE_PATH.sha256"
