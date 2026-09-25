#!/usr/bin/env bash
# Arms for `scripts/release-versions.sh`.
#
# Every arm builds its own tree under /tmp -- desktop, agent, workspace and the
# wt-media-cloud/web checkout the frontend marker is stamped from -- and runs the
# real script against it through `--desktop-dir`/`--agent-dir`/`--workspace-dir`.
# So no arm needs the sibling checkouts to be present (this file has to run in CI,
# where only this repository is checked out) and none of them can pass by reading
# this machine's real versions.
#
# usage: tests/release-versions.test.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT="$DESKTOP_DIR/scripts/release-versions.sh"
TEMP_ROOT="$(mktemp -d /private/tmp/wt-media-versions-test.XXXXXX)"
trap 'rm -rf "$TEMP_ROOT"' EXIT

PASSED=0
FAILED=0

pass() {
  PASSED=$((PASSED + 1))
  echo "ok   $1"
}

fail() {
  FAILED=$((FAILED + 1))
  echo "FAIL $1"
  echo "     $2"
}

# expect_ok <label> <args...>: the script must succeed.
expect_ok() {
  local label="$1"
  shift
  local status=0
  "$@" >"$TEMP_ROOT/out" 2>"$TEMP_ROOT/err" || status=$?
  if [[ "$status" -eq 0 ]]; then
    pass "$label"
  else
    fail "$label" "expected success, got exit=$status: $(head -2 "$TEMP_ROOT/err" | tr '\n' ' ')"
  fi
}

# expect_refused <label> <needle> <args...>: the script must fail *and say why*.
# A refusal that does not name the thing it refused is not a usable gate.
expect_refused() {
  local label="$1" needle="$2"
  shift 2
  local status=0
  "$@" >"$TEMP_ROOT/out" 2>"$TEMP_ROOT/err" || status=$?
  if [[ "$status" -eq 0 ]]; then
    fail "$label" "expected a refusal, the script succeeded"
    return
  fi
  if grep -qF "$needle" "$TEMP_ROOT/err"; then
    pass "$label"
  else
    fail "$label" "refused (exit=$status) but never named '$needle': $(head -2 "$TEMP_ROOT/err" | tr '\n' ' ')"
  fi
}

# build_tree <name> [--agent-version V] [--pyproject-version V] [--module-version V]
#            [--no-frontend-marker] [--contracts N] [--contracts-without-revision]
#            [--frontend-files N] [--empty-resources] [--web-not-a-checkout] [--no-pin]
#
# Prints the root. The tree is <root>/desktop, <root>/agent, <root>/workspace,
# <root>/app and <root>/wt-media-cloud/web (a real one-commit git repository, so
# the frontend marker's source commit comes from git rather than from a flag).
build_tree() {
  local name="$1"
  shift
  local agent_version="0.2.2" pyproject_version="0.2.2" module_version="0.2.2"
  local frontend_marker=true contracts=2 revision_ok=true frontend_files=1
  local resources=true web_git=true pin=true
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --agent-version) agent_version="$2"; shift 2 ;;
      --pyproject-version) pyproject_version="$2"; shift 2 ;;
      --module-version) module_version="$2"; shift 2 ;;
      --no-frontend-marker) frontend_marker=false; shift ;;
      --contracts) contracts="$2"; shift 2 ;;
      --contracts-without-revision) revision_ok=false; shift ;;
      --frontend-files) frontend_files="$2"; shift 2 ;;
      --empty-resources) resources=false; shift ;;
      --web-not-a-checkout) web_git=false; frontend_marker=false; shift ;;
      --no-pin) pin=false; shift ;;
      *) echo "build_tree: unknown argument $1" >&2; exit 2 ;;
    esac
  done

  local root="$TEMP_ROOT/$name"
  mkdir -p "$root/desktop/src-tauri" "$root/desktop/target" \
    "$root/desktop/.generated/frontend" "$root/agent/src/wt_media_agent/runtime" \
    "$root/workspace/config" "$root/app/Contents/Resources/resources" \
    "$root/wt-media-cloud/web/src"

  echo '{"version":"0.1.0"}' >"$root/desktop/src-tauri/tauri.conf.json"
  echo "{\"schema_version\":1,\"agent_version\":\"$agent_version\"}" \
    >"$root/desktop/src-tauri/agent-compat.json"
  if [[ "$pin" == false ]]; then
    rm -f "$root/desktop/src-tauri/agent-compat.json"
  fi
  echo "{\"component\":\"wt-media-agent\",\"version\":\"${SIDECAR_VERSION:-$agent_version}\",\"target\":\"aarch64-apple-darwin\"}" \
    >"$root/desktop/target/sidecar-manifest.json"

  printf '[project]\nname = "wt-media-agent"\nversion = "%s"\n' "$pyproject_version" \
    >"$root/agent/pyproject.toml"
  printf '"""Single source of truth for the Agent version string."""\n\n__version__ = "%s"\n' \
    "$module_version" >"$root/agent/src/wt_media_agent/runtime/version.py"

  {
    echo "schema_version: 1"
    echo ""
    echo "contracts:"
    local index=1
    while [[ "$index" -le "$contracts" ]]; do
      echo "  contract_$index:"
      echo "    owner: wt-media-cloud"
      echo "    state: active"
      if [[ "$revision_ok" == true ]]; then
        echo "    contract_revision: \"2026.07.1$index.7\""
      fi
      index=$((index + 1))
    done
  } >"$root/workspace/config/contract-map.yaml"

  local file=1
  while [[ "$file" -le "$frontend_files" ]]; do
    echo "<html>built $file</html>" >"$root/desktop/.generated/frontend/asset-$file.html"
    file=$((file + 1))
  done

  echo '{"name":"wt-media-cloud-web","version":"0.1.0"}' >"$root/wt-media-cloud/web/package.json"
  echo 'export const app = 1;' >"$root/wt-media-cloud/web/src/main.js"
  if [[ "$web_git" == true ]]; then
    git -C "$root/wt-media-cloud/web" init -q
    git -C "$root/wt-media-cloud/web" -c user.email=test@example.com -c user.name=test \
      add -A
    git -C "$root/wt-media-cloud/web" -c user.email=test@example.com -c user.name=test \
      commit -q -m "frontend source"
  fi

  if [[ "$frontend_marker" == true ]]; then
    bash "$SCRIPT" --stamp-frontend "$root/desktop/.generated/frontend" \
      --desktop-dir "$root/desktop" >/dev/null
  fi

  if [[ "$resources" == true ]]; then
    echo 'environment = "production"' >"$root/app/Contents/Resources/resources/desktop.toml"
    echo 'port = 8765' >"$root/app/Contents/Resources/stand-in.toml"
  fi

  echo "$root"
}

# The three directories the script has to be pointed at.
against() {
  local root="$1"
  echo "--desktop-dir" "$root/desktop" "--agent-dir" "$root/agent" "--workspace-dir" "$root/workspace"
}

record_field() {
  local record="$1" field="$2"
  [[ -f "$record" ]] || { echo "no-record"; return 0; }
  node -p "require(process.argv[1])$field" "$record" 2>/dev/null || echo "unreadable"
}

echo "=== the five classes ==="

ROOT="$(build_tree classes)"
MARKER="$ROOT/desktop/.generated/frontend/frontend-build.json"
EXPECTED_FRONTEND="$(record_field "$MARKER" '.package_version')+$(record_field "$MARKER" '.source_commit')"
if bash "$SCRIPT" $(against "$ROOT") --app "$ROOT/app" >"$TEMP_ROOT/out" 2>"$TEMP_ROOT/err"; then
  if grep -qE '^desktop_version=0\.1\.0$' "$TEMP_ROOT/out" &&
    grep -qE '^agent_version=0\.2\.2$' "$TEMP_ROOT/out" &&
    grep -qF "frontend_build_version=$EXPECTED_FRONTEND" "$TEMP_ROOT/out" &&
    grep -qE '^contract_versions=contract_1=2026\.07\.11\.7' "$TEMP_ROOT/out" &&
    grep -qE '^components_resources_version=sha256:[0-9a-f]{64}$' "$TEMP_ROOT/out" &&
    grep -qE '^components_resources_files=2$' "$TEMP_ROOT/out"; then
    pass "A1 the five classes are answered, with a denominator"
  else
    fail "A1 the five classes are answered, with a denominator" \
      "output was: $(tr '\n' ' | ' <"$TEMP_ROOT/out")"
  fi
else
  fail "A1 the five classes are answered, with a denominator" \
    "exit=$?: $(head -3 "$TEMP_ROOT/err" | tr '\n' ' ')"
fi

echo
echo "=== Desktop <-> sidecar compatibility ==="

ROOT="$(build_tree matched)"
expect_ok "A2 the pinned Agent version is accepted" \
  bash "$SCRIPT" $(against "$ROOT") --check

# The needle is the pin file, not the version: a manifest built from another Agent
# revision is refused too, and that refusal also prints 0.2.3. Only the pin refusal
# names the pin file, so only this needle proves *this* gate stopped the package.
ROOT="$(SIDECAR_VERSION=0.2.3 build_tree drifted)"
expect_refused "A3 a sidecar reporting another version is refused" "agent-compat.json" \
  bash "$SCRIPT" $(against "$ROOT") --check

echo
echo "=== the Agent's own declarations ==="

ROOT="$(build_tree agent_split --module-version 0.2.3)"
expect_refused "A4 an Agent whose two declarations disagree is refused" "0.2.3" \
  bash "$SCRIPT" $(against "$ROOT") --check

ROOT="$(build_tree agent_stale --pyproject-version 0.2.4 --module-version 0.2.4)"
expect_refused "A5 a sidecar built from another Agent revision is refused" "0.2.4" \
  bash "$SCRIPT" $(against "$ROOT") --check

echo
echo "=== the frontend build marker ==="

ROOT="$(build_tree no_marker --no-frontend-marker)"
expect_refused "A6 a frontend with no build marker is refused" "frontend-build.json" \
  bash "$SCRIPT" $(against "$ROOT") --check

ROOT="$(build_tree stale_marker)"
echo 'html { margin: 0; }' >>"$ROOT/desktop/.generated/frontend/asset-1.html"
expect_refused "A7 a marker that no longer describes the build is refused" "asset-1.html" \
  bash "$SCRIPT" $(against "$ROOT") --check

ROOT="$(build_tree dirty_frontend)"
echo 'export const app = 2;' >"$ROOT/wt-media-cloud/web/src/main.js"
bash "$SCRIPT" --stamp-frontend "$ROOT/desktop/.generated/frontend" --desktop-dir "$ROOT/desktop" \
  >"$TEMP_ROOT/out" 2>"$TEMP_ROOT/err"
if grep -qF "stamped=" "$TEMP_ROOT/out" &&
  bash "$SCRIPT" $(against "$ROOT") --check >"$TEMP_ROOT/out" 2>"$TEMP_ROOT/err" &&
  grep -qF "dirty" "$TEMP_ROOT/err" &&
  grep -qF ".dirty" "$TEMP_ROOT/out"; then
  pass "A8 a frontend built from a dirty source is marked, not refused"
else
  fail "A8 a frontend built from a dirty source is marked, not refused" \
    "$(tr '\n' ' | ' <"$TEMP_ROOT/out")$(tr '\n' ' | ' <"$TEMP_ROOT/err")"
fi

echo
echo "=== the contract source ==="

ROOT="$(build_tree contracts_ok --contracts 2)"
if bash "$SCRIPT" $(against "$ROOT") --check >"$TEMP_ROOT/out" 2>"$TEMP_ROOT/err" &&
  grep -qF "contracts=2" "$TEMP_ROOT/out" &&
  grep -qF "contract_2=2026.07.12.7" "$TEMP_ROOT/out"; then
  pass "A9 a readable contract map passes and reports how many it read"
else
  fail "A9 a readable contract map passes and reports how many it read" \
    "$(tr '\n' ' | ' <"$TEMP_ROOT/out")$(tr '\n' ' | ' <"$TEMP_ROOT/err")"
fi

ROOT="$(build_tree contracts_empty --contracts-without-revision)"
expect_refused "A10 a contract map that yields no revision is refused" "contract" \
  bash "$SCRIPT" $(against "$ROOT") --check

echo
echo "=== the artifact record ==="

ROOT="$(build_tree recorded)"
RECORD="$ROOT/app/Contents/Resources/versions.json"
if bash "$SCRIPT" --record "$ROOT/app" $(against "$ROOT") >/dev/null 2>"$TEMP_ROOT/err" &&
  node -e 'const r=require(process.argv[1]); process.exit(["desktop_version","agent_version","frontend_build_version","contract_versions","components_resources_version"].every(k=>k in r) && r.components_resources_files>0 ? 0 : 1)' "$RECORD" 2>/dev/null; then
  pass "A11 a record naming all five classes is written"
else
  fail "A11 a record naming all five classes is written" \
    "$(head -2 "$TEMP_ROOT/err" | tr '\n' ' ') $(cat "$RECORD" 2>/dev/null | tr '\n' ' ')"
fi

expect_ok "A12 the record verifies against the app it describes" \
  bash "$SCRIPT" --verify "$ROOT/app" $(against "$ROOT")

FIRST_DIGEST="$(record_field "$RECORD" '.components_resources_version')"
bash "$SCRIPT" --record "$ROOT/app" $(against "$ROOT") >/dev/null 2>&1
SECOND_DIGEST="$(record_field "$RECORD" '.components_resources_version')"
if [[ "$FIRST_DIGEST" == "$SECOND_DIGEST" && "$FIRST_DIGEST" != "no-record" ]]; then
  pass "A13 re-recording does not change the digest (the record excludes itself)"
else
  fail "A13 re-recording does not change the digest (the record excludes itself)" \
    "$FIRST_DIGEST != $SECOND_DIGEST"
fi

echo 'environment = "staging"' >"$ROOT/app/Contents/Resources/resources/desktop.toml"
expect_refused "A14 a resource changed after the record was written is refused" "desktop.toml" \
  bash "$SCRIPT" --verify "$ROOT/app" $(against "$ROOT")

# "versions.json" alone would also match the digest-mismatch refusal, so this
# needle is the wording only the absent-record guard produces.
ROOT="$(build_tree no_record)"
expect_refused "A15 an app carrying no record is refused" "carries no" \
  bash "$SCRIPT" --verify "$ROOT/app" $(against "$ROOT")

echo
echo "=== stamping the frontend marker ==="

EMPTY="$(mktemp -d "$TEMP_ROOT/empty-frontend.XXXXXX")"
expect_refused "A16 stamping a frontend with no files is refused" "no files" \
  bash "$SCRIPT" --stamp-frontend "$EMPTY" --desktop-dir "$EMPTY"

echo
echo "=== the guards around the sources ==="

ROOT="$(build_tree empty_resources --empty-resources)"
expect_refused "A17 an artifact carrying no components or resources is refused" "no files" \
  bash "$SCRIPT" $(against "$ROOT") --app "$ROOT/app"

ROOT="$(build_tree record_disagrees)"
RECORD="$ROOT/app/Contents/Resources/versions.json"
bash "$SCRIPT" --record "$ROOT/app" $(against "$ROOT") >/dev/null 2>&1
node -e '
  const fs = require("fs");
  const path = process.argv[1];
  const record = JSON.parse(fs.readFileSync(path, "utf8"));
  record.desktop_version = "9.9.9";
  fs.writeFileSync(path, JSON.stringify(record, null, 2) + "\n");
' "$RECORD"
expect_refused "A18 a record that disagrees with the build is refused" "desktop_version" \
  bash "$SCRIPT" --verify "$ROOT/app" $(against "$ROOT")

ROOT="$(build_tree web_not_a_checkout --web-not-a-checkout)"
expect_refused "A19 a frontend source that is not a git checkout is refused" "git checkout" \
  bash "$SCRIPT" --stamp-frontend "$ROOT/desktop/.generated/frontend" \
  --desktop-dir "$ROOT/desktop" --web-dir "$ROOT/wt-media-cloud/web"

# As with A15: the missing-manifest refusal also names the pin path, so the needle
# has to be the wording only the missing-pin guard produces.
ROOT="$(build_tree no_pin --no-pin)"
expect_refused "A20 a Desktop release that pins no Agent build is refused" "nothing states" \
  bash "$SCRIPT" $(against "$ROOT") --check

echo
echo "release-versions tests: $PASSED passed, $FAILED failed"
[[ "$FAILED" -eq 0 ]]
