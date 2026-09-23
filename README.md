# wt-media-desktop

Tauri 2 desktop shell for the modular social media operations platform.

This repository owns the native shell only. The business Vue pages are built in
`../wt-media-cloud/web` and arrive here as `.generated/frontend/`.

## Responsibilities

- Tauri lifecycle: windows, tray, exit, client run state.
- Startup configuration and security: config schema, file location, per-launch
  runtime token, and CSP injection.
- Native bridge under capability control: file selection, secure storage, system
  info, updater.
- Local Agent lifecycle: sidecar start/stop/health/recovery, plus the loopback
  and Cloud HTTP clients.
- Cross-platform packaging for Windows x64, macOS Intel, and macOS Apple Silicon.

## Bootstrap

Requires a Rust/Cargo toolchain. `npm` is **not** used: this repository has no
`package.json`.

```bash
scripts/bootstrap.sh
scripts/test.sh          # cargo test --workspace
scripts/build.sh
```

Some sibling scripts (`build.sh`, `dev.sh`, `start.sh`, `stop.sh`, `health.sh`,
`bootstrap.sh`, `scripts/*.mjs`) still invoke `npm`/`vite` from the M0 layout,
and `.github/workflows/m0-desktop.yml` drives them. That mismatch is registered
in CHG-20260923-056 (T-06) as out of scope: it cannot be fixed by changing the
reference alone, since the frontend build lives in another repository.

## Key Directories

- `src-tauri/src`: the Rust shell, split by responsibility — `main.rs` is the
  launch sequence only. See `DIRECTORY_MAP.md` for the full map.
- `src-tauri/resources/desktop.production.toml`: production configuration,
  compiled into the binary as a fallback.
- `src-tauri/binaries`: bundled sidecar component.
- `src-tauri/capabilities`: Tauri permissions.
- `contracts.lock.json`: consumed contract versions.

There is no `src/` directory in this repository.

## Local Agent Control Boundary

The Desktop boundary consumes Cloud-Agent API `v1@2026.07.15.1`, task schemas
`v1@2026.07.15.1`, Local Agent API `v1@2026.07.14.7`, Profile guard events
`profile-guard@2026.07.14.8`, and local status enums `status@2026.07.14.8`.

Vue code does not reach Local Agent ports or tokens directly. It asks
`get_public_config` for the three non-secret facts it is allowed to see
(`cloud_base_url`, `local_agent_port`, `environment`) and calls the sidecar
through the Rust bridge, which attaches the per-launch runtime token.

The `local_agent_bind_session` native command accepts a short-lived one-use Cloud
ticket. The Vue service passes it only as an invoke argument and normalizes the
response so node/permit credentials are retained by the native/Agent boundary
rather than by application state.

## Verification

```text
cargo test --workspace
```

For the full desktop readiness gate:

```text
cargo test --workspace
cargo build
scripts/start.sh
scripts/health.sh
scripts/stop.sh
```
