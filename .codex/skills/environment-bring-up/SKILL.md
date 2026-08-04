---
name: environment-bring-up
description: Deterministically bring up and verify the WT Media local runtime environment (Cloud, Local Agent, BitBrowser, packaged Desktop DMG) from latest source before any M2-B acceptance or manual verification session. Use whenever a session needs a known-good, current environment and must not trust leftover or stale processes.
---
<!-- GENERATED FILE - DO NOT EDIT DIRECTLY -->
<!-- Source: skills/common/environment-bring-up/SKILL.md -->


# WT Media Environment Bring-Up

Use this skill at the start of any acceptance, manual verification, or manual-fix session that needs the local WT Media runtime. The environment must be **deterministic**: latest source, forced restarts, one fixed database, and every gate green before a human touches the packaged Desktop app.

## Why deterministic

Leftover processes and stale builds cause non-reproducible failures ("works in API, fails in the packaged app", "different error every login"). The two concrete hazards this skill eliminates:

- `scripts/m2b_local_acceptance.py` historically returned early when a service was already healthy, so **an old Cloud/Agent process kept running after new commits**.
- Multiple historical databases and ad-hoc DSNs make runs non-identical.

## Preconditions

- All four repos present: `wt-media-cloud`, `wt-media-agent`, `wt-media-desktop`, `wt-media-workspace`.
- MySQL reachable at `127.0.0.1:3306` (DSN `root:root123@tcp(127.0.0.1:3306)/wt_media_cloud?...`).
- BitBrowser API reachable at `127.0.0.1:54345`.
- Toolchains: Go, Python venv (`wt-media-agent/.venv`), Cargo/Tauri.
- One fixed DSN: `wt_media_cloud`. Do not switch to `wt-media-cloud`, `wt_media_acceptance`, or any other schema.

## Procedure

Run the acceptance harness end to end: `wt-media-workspace/scripts/m2b-local-acceptance.sh all`.

Before relying on any process, enforce freshness:

1. **Force-stop stale processes.** Kill any running Cloud (`127.0.0.1:18080`) and Local Agent (`127.0.0.1:8765`) and remove their PID files. A "healthy" process is NOT fresh just because it answers health checks.
2. **Rebuild from latest source.** In order:
   - Cloud migrations (`wt-media-cloud/scripts/migrate.sh`);
   - Cloud server (`go run ./cmd/server`);
   - Local Agent (`wt-media-agent/.venv/bin/python -m wt_media_agent.local_api.server`);
   - Desktop frontend via `wt-media-workspace/scripts/build-desktop.sh` and DMG via `cargo tauri build --bundles dmg --no-sign`;
   - Regenerate Cloud desktop artifacts (`npm run build:desktop`).
3. **Freshness gate.** Confirm each running artifact was built after the latest source commit it contains (compare process start time and build artifact mtime against `git log -1 --format=%ci` in the owning repo). If any artifact is older than its source, restart/re-build it. Do not proceed with a stale Cloud or Agent.
4. **Mount and launch** the freshly built DMG at `/Volumes/WT Media/WT Media.app`.
5. **Verification gates** — all must pass, no partial-pass shortcut:
   - Cloud `GET /api/v1/health` → `{"errcode":0}`;
   - Agent `GET /healthz` and `GET /api/v1/status` → `bitbrowser_status == "normal"`;
   - Desktop assets present and the built chunk contains the local Cloud API base (`127.0.0.1:18080/api/v1`);
   - DMG exists, non-empty, freshly built, and mounted;
   - CORS preflight from `Origin: http://tauri.localhost` returns `Access-Control-Allow-Origin: http://tauri.localhost` + credentials;
   - **Login smoke** `POST /api/v1/auth/login` with a known-good account and `replace_existing: true` → `errcode 0`. Known convention: `admin/admin123`; acceptance operator is `operator01` with password reset to `operator01`. If the credential is unknown, reset it via the admin `POST /api/v1/users/:user_id/reset-password` flow before continuing.

## Known packaged Desktop login failure patterns

Reuse these prior conclusions instead of re-debugging them:

- UI shows `服务器返回格式错误` / `服务器响应格式错误`: the packaged Desktop resolved a relative `/api/v1` against the Tauri asset protocol instead of Cloud. Fix: packaged builds must embed the absolute base `http://127.0.0.1:18080/api/v1` in the generated `http-*.js` chunk.
- Wrong password / account disabled returns `errcode 11001` with message `请先登录或凭证已过期` — this is the generic auth-failed message, NOT a session message. If the operator cannot log in, the credential is wrong or disabled; reset via admin `POST /api/v1/users/:user_id/reset-password` (convention: reset password equals the username, e.g. `operator01`/`operator01`).
- Valid credentials with an existing session return `errcode 20010 当前账号已在其他位置登录`; the client must resend with `replace_existing: true`. The Desktop `LoginPage` already handles this — verify the flow, do not treat 20010 as a hard failure.
- Login "works from curl but fails in the packaged app": check the embedded API base, CORS preflight from `http://tauri.localhost`, and that the running Cloud is the latest build (stale `go run` process serves old routes).
- Before reopening a rebuilt DMG: kill old `wt-media-desktop-shell` processes and detach old `/Volumes/WT Media*` volumes.
- Environment non-determinism from multiple historical databases: always use the single fixed DSN `wt_media_cloud` and never switch to `wt-media-cloud`, `wt_media_acceptance`, or other schemas.

## Failure handling

If any gate fails: **stop**, collect the exact error (Cloud/Agent log under the harness runtime dir, page console, HTTP response), fix the root cause, then re-run the full `all` flow from the top. Never declare the environment ready when a gate is skipped or faked; never reuse a process that was only partially re-verified.

## Evidence

Record the bring-up run (commands, expected vs actual per gate, PASS/FAIL, related commit/diff) under `wt-media-workspace/delivery/active/<CHG>/evidence/` as acceptance evidence, matching the CHG evidence format.

## Boundaries

- This skill prepares and verifies the environment; it does not perform product acceptance or runtime implementation.
- It is tooling-only and never changes runtime code beyond what the owned harness scripts do.
