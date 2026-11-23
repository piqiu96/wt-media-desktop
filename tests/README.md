# Desktop Tests

Rust unit tests stay with their modules under `src-tauri/`; longer test modules
may use separate files behind `#[cfg(test)]`. Script regressions live here,
grouped by responsibility rather than language or CHG.

## Layout

| Directory | Scope |
| --- | --- |
| `op/` | Operational entry tests: start/stop/status command dispatch and executable permissions. |
| `pack/` | Packaging identity, release gates, bundle layout, and Windows process/installer acceptance. |

## Default entry

Run `scripts/test.sh` from any directory. The Desktop PR workflow uses this same
entry. It runs the Rust workspace, both groups of `*.test.sh`, and the two Python
checks listed below. Script fixtures use temporary directories and require no
sibling repository checkout. Shell fixtures require Bash and Node; Python checks
require Python 3.11 or newer. Rust/Tauri has its normal platform build dependencies.

| File | Coverage | Execution |
| --- | --- | --- |
| `op/control.test.sh` | Entry tracking, executable permissions, help, verbs and invalid arguments; does not start the app. | Default entry |
| `pack/release-versions.test.sh` | Version sources, resource records and rejection of inconsistent release inputs. | Default entry |
| `pack/package-release-macos.test.sh` | Release directory and archive layout via `--dry-run`; does not build an app. | Default entry |
| `pack/desktop_environment_test.py` | dev/test/prod packaging identity, Cloud routing independence, signing record verification, output paths and NSIS cleanup namespaces. | Default entry; NSIS fixture compilation skips explicitly when `makensis` is absent. |
| `pack/windows_subsystem_test.py` | Conditional Windows GUI subsystem declaration, preserving debug console behavior. | Default entry; static check, runnable on macOS too. |
| `pack/windows_installer_agent_stop.ps1` | Stops only the process at the installed path, preserving another same-name process. | Workspace Windows release job |
| `pack/windows_installer_roundtrip.ps1` | Real install, reinstall and uninstall; verifies Agent shutdown and data preservation during reinstall. | Workspace Windows release job; requires an installer and a clean Windows runner. |

The default entry never invokes the PowerShell acceptance scripts or installs
software. A green default entry does not imply those Windows checks ran; inspect
the Windows release job for their results. Where NSIS coverage is required,
install `makensis` and confirm the fixture test was executed rather than skipped.

## Placement

Put new operation regressions in `op/` and packaging/installer regressions in
`pack/`. Keep the two-level structure until additional categories are needed.
Add new checks to the appropriate entry and update this table. Real artifact
verification tools used during delivery belong in `scripts/verify/`; their
regression tests belong here.
