#!/usr/bin/env bash
set -euo pipefail

# The five version classes a WT Media release has to be able to answer for, and
# the one place that answers them.
#
#   1. desktop_version        src-tauri/tauri.conf.json -- Tauri's `version`,
#                             which is what the DMG and the release folder are
#                             named after
#   2. agent_version          target/sidecar-manifest.json, written by
#                             wt-media-agent/scripts/build_desktop_sidecar.py from
#                             the Agent's pyproject.toml
#   3. frontend_build_version .generated/frontend/frontend-build.json -- stamped by
#                             `--stamp-frontend` from the wt-media-cloud/web
#                             checkout the frontend was built from, so it names the
#                             source commit as well as the package version
#   4. contract_versions      ../wt-media-workspace/config/contract-map.yaml, the
#                             cross-repository authority for contract revisions
#                             (AGENT-INDEX.md)
#   5. components_resources_version
#                             a content digest of what the artifact carries: every
#                             file under `Contents/Resources`, excluding this
#                             script's own record. An identity, not a number
#                             anybody bumps -- it changes exactly when the shipped
#                             set changes, and it cannot go stale because it is
#                             computed from the bytes. The record and the frontend
#                             marker keep the per-file hashes behind that digest
#                             so a mismatch can name the file that moved.
#
# `--check` is the release gate: three agreements, each refusal naming the pair
# that disagreed.
#
#   * src-tauri/agent-compat.json == the built sidecar's version. **This is the
#     Desktop <-> sidecar compatibility check.** It is a review gate, not a proof
#     -- it cannot know whether a new Agent's local API still suits this Desktop,
#     so it forces the question at the only moment anybody can answer it: when the
#     Agent version changes and the package would otherwise ship that change
#     silently. Updating the pin is the review.
#   * the Agent's pyproject.toml == its runtime/version.py, and both == the built
#     manifest. Those are declarations of one fact; apart, the manifest and the
#     Agent's own `/healthz` would disagree about which Agent is installed and
#     each would look right on its own.
#   * the frontend marker == the frontend tree it describes, so a marker left over
#     from an earlier build is refused instead of being copied into the record.
#
# It runs after `cargo tauri build` (the sidecar manifest and the frontend marker
# only exist by then) and before any release output does, and the record itself is
# written by `repair-macos-signing.sh` between the sidecar's signature and the
# app's -- so it is covered by the seal, for the same reason the sidecar manifest
# is (CHG-20260923-059 D-15, `evidence/task-04-packaging.out`).
#
# usage: release-versions.sh [--desktop-dir D] [--agent-dir A] [--workspace-dir W]
#                            [--web-dir W] [--app APP]
#        release-versions.sh ... --check
#        release-versions.sh ... --record APP
#        release-versions.sh ... --verify APP
#        release-versions.sh ... --stamp-frontend DIR
DESKTOP_DIR="$(cd "$(dirname "$0")/.." && pwd)"
AGENT_DIR=""
WORKSPACE_DIR=""
WEB_DIR=""
APP_PATH=""
MODE="print"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --desktop-dir) DESKTOP_DIR="$2"; shift 2 ;;
    --agent-dir) AGENT_DIR="$2"; shift 2 ;;
    --workspace-dir) WORKSPACE_DIR="$2"; shift 2 ;;
    --web-dir) WEB_DIR="$2"; shift 2 ;;
    --app) APP_PATH="$2"; shift 2 ;;
    --check) MODE="check"; shift ;;
    --record) MODE="record"; APP_PATH="$2"; shift 2 ;;
    --verify) MODE="verify"; APP_PATH="$2"; shift 2 ;;
    --stamp-frontend) MODE="stamp"; STAMP_DIR="$2"; shift 2 ;;
    -h|--help) sed -n '2,/^[^#]/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
DESKTOP_DIR="$(cd "$DESKTOP_DIR" && pwd)"
ROOT_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"
AGENT_DIR="${AGENT_DIR:-$ROOT_DIR/wt-media-agent}"
WORKSPACE_DIR="${WORKSPACE_DIR:-$ROOT_DIR/wt-media-workspace}"
WEB_DIR="${WEB_DIR:-$ROOT_DIR/wt-media-cloud/web}"
MANIFEST_PATH="$DESKTOP_DIR/target/sidecar-manifest.json"
PIN_PATH="$DESKTOP_DIR/src-tauri/agent-compat.json"
FRONTEND_DIR="$DESKTOP_DIR/.generated/frontend"
FRONTEND_MARKER="$FRONTEND_DIR/frontend-build.json"
FRONTEND_MARKER_NAME="frontend-build.json"
CONTRACT_MAP="$WORKSPACE_DIR/config/contract-map.yaml"
# A digest never covers the file that holds it: the record excludes itself, and so
# does the marker. Otherwise neither value could ever be reproduced.
RECORD_NAME="versions.json"

refuse() {
  echo "$*" >&2
  exit 1
}

# json_field <file> <suffix>: `node -p` over a JSON file, or the literal
# `missing`/`unreadable`, so a caller can say which of the two happened.
json_field() {
  local file="$1" suffix="$2"
  if [[ ! -f "$file" ]]; then
    echo "missing"
    return 0
  fi
  node -p "require(process.argv[1])$suffix" "$file" 2>/dev/null || echo "unreadable"
}

# resource_files <dir> <exclude-name>: the denominator. A digest of nothing must
# not be able to pass as "no differences".
resource_files() {
  find "$1" -type f ! -name "$2" | wc -l | tr -d ' '
}

# sha256_digest <path|->: the digest only, from either platform's tool.
#
# macOS has `shasum`; GitHub's Windows Git Bash has `sha256sum` but not the Perl
# wrapper. Refusing to silently emit an empty digest is part of the release gate.
sha256_digest() {
  local path="${1:--}" output
  if command -v shasum >/dev/null 2>&1; then
    if [[ "$path" == "-" ]]; then
      output="$(shasum -a 256)" || return 127
    else
      output="$(shasum -a 256 "$path")" || return 127
    fi
  elif command -v sha256sum >/dev/null 2>&1; then
    if [[ "$path" == "-" ]]; then
      output="$(sha256sum)" || return 127
    else
      output="$(sha256sum "$path")" || return 127
    fi
  else
    echo "neither shasum nor sha256sum is available" >&2
    return 127
  fi
  printf '%s\n' "$output" | awk '{print $1}'
}

# resource_manifest <dir> <exclude-name>: `<relative path> <sha256>` per file,
# sorted. Content only -- no timestamps, no inode facts -- so the same set of bytes
# gives the same lines on any machine. Paths are ours, so a filename with a newline
# in it is out of scope (registered in the CHG's evidence).
resource_manifest() {
  local dir="$1" exclude="$2"
  (
    cd "$dir"
    find . -type f ! -name "$exclude" | LC_ALL=C sort | while IFS= read -r file; do
      printf '%s %s\n' "${file#./}" "$(sha256_digest "$file")"
    done
  )
}

# resource_digest <dir> <exclude-name>: one sha256 over those lines, so an artifact
# carries one identity that changes exactly when the set changes.
resource_digest() {
  resource_manifest "$1" "$2" | sha256_digest -
}

# name_changes <json file> <field>: reads manifest lines on stdin and prints the
# files that differ from the manifest stored at `<field>` in that JSON, so a
# refusal can say *which* file moved instead of only that the digest did. An empty
# result means the JSON carries no manifest (an artifact from an older record), and
# the caller falls back to a message that names no file.
name_changes() {
  node -e '
    const fs = require("fs");
    const [jsonPath, field] = process.argv.slice(1);
    let recorded = {};
    try {
      let node = JSON.parse(fs.readFileSync(jsonPath, "utf8"));
      for (const part of field.split(".")) {
        node = node === null || node === undefined ? undefined : node[part];
      }
      if (node && typeof node === "object" && !Array.isArray(node)) recorded = node;
    } catch (error) {
      recorded = {};
    }
    const current = new Map();
    for (const line of fs.readFileSync(0, "utf8").split("\n")) {
      if (!line) continue;
      const space = line.indexOf(" ");
      current.set(line.slice(0, space), line.slice(space + 1));
    }
    const names = [];
    for (const [name, digest] of current) {
      if (recorded[name] !== digest) names.push(name + (name in recorded ? "" : " (added)"));
    }
    for (const name of Object.keys(recorded)) {
      if (!current.has(name)) names.push(name + " (removed)");
    }
    names.sort();
    if (names.length > 5) {
      process.stdout.write(names.slice(0, 5).join(", ") + " and " + (names.length - 5) + " more");
    } else {
      process.stdout.write(names.join(", "));
    }
  ' "$1" "$2"
}

# agent_module_version: the Agent's own declaration, the one `/healthz` reports.
agent_module_version() {
  local path="$AGENT_DIR/src/wt_media_agent/runtime/version.py"
  if [[ ! -f "$path" ]]; then
    refuse "the Agent checkout has no $path, so its runtime version cannot be read"
  fi
  sed -n 's/^__version__ = "\(.*\)"$/\1/p' "$path" | head -1
}

# agent_package_version: what the build reads out of pyproject.toml, which is what
# lands in the sidecar manifest.
agent_package_version() {
  local path="$AGENT_DIR/pyproject.toml"
  if [[ ! -f "$path" ]]; then
    refuse "the Agent checkout has no $path, so its package version cannot be read"
  fi
  awk '
    /^\[/ { section = $0 }
    section == "[project]" && /^version *= *"/ {
      value = $0; sub(/^version *= *"/, "", value); sub(/".*$/, "", value); print value; exit
    }
  ' "$path"
}

# contract_versions: `<name>=<revision>` per contract, sorted. The revision key is
# whatever the map calls it (`contract_revision`, `schema_revision`, ...), and
# `minimum_*` is a floor rather than the revision itself, so it is skipped.
contract_versions() {
  if [[ ! -f "$CONTRACT_MAP" ]]; then
    refuse "the workspace has no $CONTRACT_MAP, so no contract revision can be cited"
  fi
  awk '
    /^contracts:/ { inside = 1; next }
    inside && /^[^ ]/ { inside = 0 }
    inside && /^  [a-z0-9_]+:$/ { name = $1; sub(/:$/, "", name); next }
    inside && /^    [a-z0-9_]*revision: *"/ && $1 !~ /minimum/ {
      value = $0; sub(/^    [a-z0-9_]*revision: *"/, "", value); sub(/".*$/, "", value)
      if (name != "") { print name "=" value }
    }
  ' "$CONTRACT_MAP" | LC_ALL=C sort
}

frontend_version_string() {
  local dir="$1"
  local marker="$dir/$FRONTEND_MARKER_NAME"
  if [[ ! -f "$marker" ]]; then
    refuse "$marker is missing: the frontend build has to be stamped first ($0 --stamp-frontend $dir)"
  fi
  local package_version commit dirty value
  package_version="$(json_field "$marker" '.package_version')"
  commit="$(json_field "$marker" '.source_commit')"
  dirty="$(json_field "$marker" '.source_dirty')"
  value="$package_version+$commit"
  if [[ "$dirty" == "true" ]]; then
    value="$value.dirty"
  fi
  echo "$value"
}

# check_frontend_marker: the marker must describe the tree it sits in.
check_frontend_marker() {
  if [[ ! -f "$FRONTEND_MARKER" ]]; then
    refuse "$FRONTEND_MARKER is missing: the frontend build has to be stamped before a release can name it ($0 --stamp-frontend $FRONTEND_DIR)"
  fi
  local recorded actual
  recorded="$(json_field "$FRONTEND_MARKER" '.digest')"
  actual="$(resource_digest "$FRONTEND_DIR" "$FRONTEND_MARKER_NAME")"
  if [[ "$recorded" != "$actual" ]]; then
    local changed
    changed="$(resource_manifest "$FRONTEND_DIR" "$FRONTEND_MARKER_NAME" | name_changes "$FRONTEND_MARKER" manifest)"
    refuse "$FRONTEND_MARKER does not describe $FRONTEND_DIR: ${changed:-the tree} changed since it was stamped (it records digest $recorded and the tree now hashes to $actual). A marker left over from an earlier build would put a version in the record that no file in the build has"
  fi
}

check_agreements() {
  if [[ ! -f "$PIN_PATH" ]]; then
    refuse "$PIN_PATH is missing: nothing states which Agent build this Desktop release is released against"
  fi
  local pin package_version module_version
  pin="$(json_field "$PIN_PATH" '.agent_version')"
  if [[ "$AGENT_VERSION" == "missing" || "$AGENT_VERSION" == "unreadable" ]]; then
    refuse "$MANIFEST_PATH is missing: the sidecar has to be built before its version can be compared with the pin in $PIN_PATH ($pin)"
  fi
  if [[ "$pin" != "$AGENT_VERSION" ]]; then
    refuse "the packaged Agent is $AGENT_VERSION but this Desktop release pins $pin ($PIN_PATH): review the Agent's local API changes against this Desktop and update the pin, or build the pinned Agent"
  fi
  package_version="$(agent_package_version)"
  module_version="$(agent_module_version)"
  if [[ "$package_version" != "$module_version" ]]; then
    refuse "the Agent states two versions: pyproject.toml says $package_version and runtime/version.py says $module_version, so the manifest and the Agent's own /healthz would disagree about which Agent is installed"
  fi
  if [[ "$package_version" != "$AGENT_VERSION" ]]; then
    refuse "$MANIFEST_PATH says the built Agent is $AGENT_VERSION but the Agent source is $package_version: the sidecar in target/ was built from another revision"
  fi
  check_frontend_marker
}

resolve_classes() {
  DESKTOP_VERSION="$(json_field "$DESKTOP_DIR/src-tauri/tauri.conf.json" '.version')"
  if [[ "$DESKTOP_VERSION" == "missing" || "$DESKTOP_VERSION" == "unreadable" ]]; then
    refuse "cannot read the Desktop version out of $DESKTOP_DIR/src-tauri/tauri.conf.json (got '$DESKTOP_VERSION')"
  fi
  AGENT_VERSION="$(json_field "$MANIFEST_PATH" '.version')"
  FRONTEND_VERSION="$(frontend_version_string "$FRONTEND_DIR")"
  CONTRACTS="$(contract_versions)"
  CONTRACT_COUNT="$(printf '%s\n' "$CONTRACTS" | grep -c '=' || true)"
  if [[ -z "$CONTRACTS" || "$CONTRACT_COUNT" -eq 0 ]]; then
    refuse "$CONTRACT_MAP lists contracts but no revision could be read out of it, so citing them would prove nothing"
  fi
  CONTRACT_LINE="$(printf '%s' "$CONTRACTS" | tr '\n' ',')"
  CONTRACT_LINE="${CONTRACT_LINE%,}"
  if [[ -n "$APP_PATH" ]]; then
    RESOURCES_DIR="$APP_PATH/Contents/Resources"
    if [[ ! -d "$RESOURCES_DIR" ]]; then
      refuse "$APP_PATH carries no $RESOURCES_DIR, so its components and resources cannot be identified"
    fi
    RESOURCE_COUNT="$(resource_files "$RESOURCES_DIR" "$RECORD_NAME")"
    if [[ "$RESOURCE_COUNT" -eq 0 ]]; then
      refuse "$RESOURCES_DIR has no files, so a digest of it would prove nothing"
    fi
    RESOURCE_DIGEST="$(resource_digest "$RESOURCES_DIR" "$RECORD_NAME")"
  fi
}

print_classes() {
  echo "desktop_version=$DESKTOP_VERSION"
  echo "agent_version=$AGENT_VERSION"
  echo "frontend_build_version=$FRONTEND_VERSION"
  echo "contract_versions=$CONTRACT_LINE"
  if [[ -n "$APP_PATH" ]]; then
    echo "components_resources_version=sha256:$RESOURCE_DIGEST"
    echo "components_resources_files=$RESOURCE_COUNT"
  else
    echo "components_resources_version=unavailable (pass --app: it is a digest of the artifact)"
  fi
  echo "contracts=$CONTRACT_COUNT"
}

write_record() {
  local record="$APP_PATH/Contents/Resources/$RECORD_NAME"
  resource_manifest "$RESOURCES_DIR" "$RECORD_NAME" | node -e '
    const fs = require("fs");
    const [path, desktop, agent, frontend, contracts, digest, files] = process.argv.slice(1);
    const manifest = {};
    for (const line of fs.readFileSync(0, "utf8").split("\n")) {
      if (!line) continue;
      const space = line.indexOf(" ");
      manifest[line.slice(0, space)] = line.slice(space + 1);
    }
    const map = {};
    for (const pair of contracts.split(",")) {
      const [name, revision] = pair.split("=");
      map[name] = revision;
    }
    fs.writeFileSync(path, JSON.stringify({
      schema_version: 1,
      desktop_version: desktop,
      agent_version: agent,
      frontend_build_version: frontend,
      contract_versions: map,
      components_resources_version: "sha256:" + digest,
      components_resources_files: Number(files),
      components_resources_manifest: manifest,
    }, null, 2) + "\n");
  ' "$record" "$DESKTOP_VERSION" "$AGENT_VERSION" "$FRONTEND_VERSION" "$CONTRACT_LINE" "$RESOURCE_DIGEST" "$RESOURCE_COUNT"
}

verify_record() {
  local record="$APP_PATH/Contents/Resources/$RECORD_NAME"
  if [[ ! -f "$record" ]]; then
    refuse "$APP_PATH carries no $record: the artifact does not say which versions it was built from"
  fi
  local recorded expected
  recorded="$(json_field "$record" '.components_resources_version')"
  expected="sha256:$RESOURCE_DIGEST"
  if [[ "$recorded" != "$expected" ]]; then
    local changed
    changed="$(resource_manifest "$RESOURCES_DIR" "$RECORD_NAME" | name_changes "$record" components_resources_manifest)"
    refuse "$record does not describe this artifact: ${changed:-a component or resource} changed after the record was written (it records $recorded and the artifact now hashes to $expected)"
  fi
  local key value found
  for key in desktop_version agent_version frontend_build_version; do
    case "$key" in
      desktop_version) value="$DESKTOP_VERSION" ;;
      agent_version) value="$AGENT_VERSION" ;;
      frontend_build_version) value="$FRONTEND_VERSION" ;;
    esac
    found="$(json_field "$record" ".$key")"
    if [[ "$found" != "$value" ]]; then
      refuse "$record says $key=$found but this build has $value: the record and the build disagree"
    fi
  done
}

if [[ "$MODE" == "stamp" ]]; then
  files="$(resource_files "$STAMP_DIR" "$FRONTEND_MARKER_NAME")"
  if [[ "$files" -eq 0 ]]; then
    refuse "the frontend at $STAMP_DIR has no files, so there is nothing to identify"
  fi
  digest="$(resource_digest "$STAMP_DIR" "$FRONTEND_MARKER_NAME")"
  package_version="$(json_field "$WEB_DIR/package.json" '.version')"
  if [[ "$package_version" == "missing" || "$package_version" == "unreadable" ]]; then
    refuse "cannot read the frontend package version out of $WEB_DIR/package.json (got '$package_version')"
  fi
  if ! git -C "$WEB_DIR" rev-parse --short HEAD >/dev/null 2>&1; then
    refuse "cannot identify the frontend source: $WEB_DIR is not a git checkout, so no build of it can be attributed"
  fi
  commit="$(git -C "$WEB_DIR" rev-parse --short HEAD)"
  dirty=false
  if [[ -n "$(git -C "$WEB_DIR" status --porcelain -- .)" ]]; then
    dirty=true
  fi
  resource_manifest "$STAMP_DIR" "$FRONTEND_MARKER_NAME" | node -e '
    const fs = require("fs");
    const [path, packageVersion, commit, dirty, files, digest] = process.argv.slice(1);
    const manifest = {};
    for (const line of fs.readFileSync(0, "utf8").split("\n")) {
      if (!line) continue;
      const space = line.indexOf(" ");
      manifest[line.slice(0, space)] = line.slice(space + 1);
    }
    fs.writeFileSync(path, JSON.stringify({
      source: "wt-media-cloud/web",
      package_version: packageVersion,
      source_commit: commit,
      source_dirty: dirty === "true",
      files: Number(files),
      digest,
      manifest,
    }, null, 2) + "\n");
  ' "$STAMP_DIR/$FRONTEND_MARKER_NAME" "$package_version" "$commit" "$dirty" "$files" "$digest"
  echo "stamped=$STAMP_DIR/$FRONTEND_MARKER_NAME version=$package_version+$commit files=$files digest=$digest"
  exit 0
fi

resolve_classes

case "$MODE" in
  print)
    print_classes
    ;;
  check)
    check_agreements
    print_classes
    if [[ "$FRONTEND_VERSION" == *.dirty ]]; then
      echo "warning: the frontend was built from a dirty wt-media-cloud/web tree, so its commit does not describe the bytes" >&2
    fi
    echo "versions ok"
    ;;
  record)
    check_agreements
    print_classes
    write_record
    echo "record=$APP_PATH/Contents/Resources/$RECORD_NAME"
    ;;
  verify)
    check_agreements
    print_classes
    verify_record
    echo "versions verified: $APP_PATH"
    ;;
esac
