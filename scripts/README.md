# Desktop Scripts

This directory is the script index for `wt-media-desktop`. It states the
classification and the placement rule only — no per-file walkthrough, no runtime
parameters.

## Classification

| Purpose | What the category answers | Scripts (file names only) |
| --- | --- | --- |
| Operations | Bring the local shell up / confirm it is up / stop it | see [`../bin/control.sh`](../bin/control.sh) |
| Development | Rebuild / regenerate / stage / package after a change | `prepare-release-sidecar.sh`, `stage-release-config.sh`, `repair-macos-signing.sh`, `build-release-macos.sh`, `package-release-macos.sh`, `release-versions.sh`, `test.sh` |
| Verification | Prove something holds, or does not | `verify-release-macos.sh`, `test.sh`, `release-versions.sh`, `package-release-macos.sh` |

Multiple membership is a property, not a mistake: `test.sh` builds and asserts
(the Rust workspace plus the suites under `../tests/`), so it is both;
`release-versions.sh` stamps versions when preparing a release and checks them
when gating one; `package-release-macos.sh` builds the bundle and verifies its
signature and embedded sidecar in the same run.

The Operations row is the one that moved: process start/stop and liveness live in
`bin/`, not here.

## Where a new script goes

- A new **development** script → `scripts/dev/`.
- A new **verification / acceptance** script → `scripts/verify/`.
- **Start/stop and health checks always go to [`bin/`](../bin/)**; do not add them
  under `scripts/`.
- **Existing scripts do not move.** The flat files in this directory are the
  historical landing spots and are left alone. `dev/` and `verify/` take scripts
  written from here on — they are not a target shape to migrate the current files
  into.

Both subdirectories currently hold a single `.gitkeep` each.

## What this file does not own

Per-file facts are not here. Directory facts and the no-scan zones belong to
[`../DIRECTORY_MAP.md`](../DIRECTORY_MAP.md); what CI runs belongs to
[`../.github/workflows/`](../.github/workflows/); the release-gate commands belong
to `config/release-matrix.yaml` in `wt-media-workspace`.

**Numeric runtime parameters — ports, addresses, credentials, timeout and
retention defaults — have their single landing point in configuration files.**
Neither this file nor [`../bin/control.sh`](../bin/control.sh) restates them:
`bin/control.sh` dispatches verbs only, and the address it probes is a fact of
the configuration it reads, not of that file.
