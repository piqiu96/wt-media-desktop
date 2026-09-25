#!/usr/bin/env bash
#
# The one entry point for the local development shell: start, stop, restart and
# report on `cargo tauri dev`.
#
# No numeric runtime parameter lives in this file. The dev server address is a
# fact of `src-tauri/tauri.conf.json` (`build.devUrl`) and is read from there at
# run time; repeating it here would be a second source of truth for the same
# port. Timeouts come from the environment (`WT_MEDIA_DESKTOP_START_TIMEOUT`).
#
# Scope boundary against `AGENT-INDEX.md` §6 ("modify Agent Sidecar lifecycle →
# Desktop"): the sidecar's lifecycle is owned by the Rust shell
# (`src-tauri/src/sidecar/`). This file governs the local development process
# only. Two different owners, two different things.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
DESKTOP_DIR="$(pwd)"
CONF="$DESKTOP_DIR/src-tauri/tauri.conf.json"
RUNTIME_DIR="$DESKTOP_DIR/.runtime"
PID_FILE="$RUNTIME_DIR/desktop.pid"
LOG_FILE="$RUNTIME_DIR/desktop.log"
START_TIMEOUT="${WT_MEDIA_DESKTOP_START_TIMEOUT:-300}"

usage() {
  cat <<'EOF'
Usage: bin/control.sh <start|stop|restart|status|help>

  start    Run `cargo tauri dev` in the background and wait for the dev server.
  stop     Stop the process started by `start` (and its children).
  restart  Stop it, then start it again.
  status   Report whether that process is alive and whether the dev server answers.
  help     Show this help.
EOF
}

# The address is read, never spelled out. `require` needs a path that resolves
# as a module: an absolute path (or one with a leading `./`) does, a bare
# relative path does not. The `|| ""` matters: `node -p` prints `undefined` and
# exits 0 for a missing key, which would sail past the empty check below and be
# reported as a URL.
dev_url() {
  local url
  if [[ ! -f "$CONF" ]]; then
    echo "missing config: ${CONF#"$DESKTOP_DIR"/}" >&2
    return 1
  fi
  url="$(node -p 'require(process.argv[1]).build.devUrl || ""' "$CONF" 2>/dev/null || true)"
  if [[ -z "$url" ]]; then
    echo "cannot read build.devUrl from ${CONF#"$DESKTOP_DIR"/}" >&2
    return 1
  fi
  printf '%s' "$url"
}

running_pid() {
  [[ -f "$PID_FILE" ]] || return 1
  local pid
  pid="$(cat "$PID_FILE")"
  [[ -n "$pid" ]] && kill -0 "$pid" >/dev/null 2>&1 || return 1
  printf '%s' "$pid"
}

wait_for_dev_server() {
  local url="$1" pid="$2" waited=0
  while (( waited < START_TIMEOUT )); do
    if ! kill -0 "$pid" >/dev/null 2>&1; then
      echo "cargo tauri dev exited before the dev server answered" >&2
      tail -n 40 "$LOG_FILE" >&2 || true
      rm -f "$PID_FILE"
      return 1
    fi
    if curl --silent --fail --max-time 2 "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "no answer from $url after ${START_TIMEOUT}s; the process is still running" >&2
  echo "the first run compiles the Rust workspace and can take longer than that" >&2
  echo "log: ${LOG_FILE#"$DESKTOP_DIR"/} -- re-check with: bin/control.sh status" >&2
  return 1
}

do_start() {
  local pid
  if pid="$(running_pid)"; then
    echo "cargo tauri dev already running: pid=$pid"
    return 0
  fi

  if ! cargo tauri --version >/dev/null 2>&1; then
    echo "cargo tauri not available; install it with: cargo install tauri-cli" >&2
    return 1
  fi

  mkdir -p "$RUNTIME_DIR"
  # Job control puts the background job in its own process group, so `stop` can
  # signal the dev server and the shell command it spawns together instead of
  # orphaning them. `nohup` alone would leave them sharing this shell's group.
  set -m
  nohup cargo tauri dev >"$LOG_FILE" 2>&1 &
  pid=$!
  set +m
  echo "$pid" > "$PID_FILE"

  local url
  url="$(dev_url)"
  echo "cargo tauri dev started: pid=$pid log=${LOG_FILE#"$DESKTOP_DIR"/}"
  wait_for_dev_server "$url" "$pid" || return 1
  echo "dev server answering: $url"
}

do_stop() {
  local pid
  if ! pid="$(running_pid)"; then
    echo "cargo tauri dev not running"
    rm -f "$PID_FILE"
    return 0
  fi
  # Negative pid targets the whole process group; the fallback covers a pid file
  # left by a run whose group is no longer addressable.
  kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
  for _ in {1..50}; do
    kill -0 "$pid" >/dev/null 2>&1 || break
    sleep 0.1
  done
  if kill -0 "$pid" >/dev/null 2>&1; then
    kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
  fi
  rm -f "$PID_FILE"
  echo "cargo tauri dev stopped"
}

do_status() {
  local pid url alive=no health=down
  pid="$(running_pid || true)"
  url="$(dev_url)" || return 1
  [[ -n "$pid" ]] && alive=yes
  if curl --silent --fail --max-time 2 "$url" >/dev/null 2>&1; then
    health=ok
  fi
  # `alive` and `health` answer different questions and are printed apart: a
  # process can be up while the dev server is still compiling, and a dev server
  # can answer while this file has no pid for it (started by hand).
  echo "desktop: pid=${pid:--} alive=$alive health=$health url=$url"
  [[ "$alive" == yes && "$health" == ok ]]
}

case "${1:-help}" in
  start)   do_start ;;
  stop)    do_stop ;;
  restart) do_stop && do_start ;;
  status)  do_status ;;
  help|-h|--help) usage ;;
  *) echo "unknown command: $1" >&2; usage >&2; exit 2 ;;
esac
