# Desktop Tests

Desktop unit, integration, contract, e2e, and smoke tests belong here.

## Shell tests

These run from `scripts/test.sh` (which CI calls) and need no sibling checkout:
every arm fabricates its own trees under `/private/tmp`.

| File | What it covers |
| --- | --- |
| `release-versions.test.sh` | The five version classes and the release gate in `scripts/release-versions.sh`: each class has a readable source, and a package whose Agent build, Agent declaration, frontend marker, contract map or resource digest disagrees with the Desktop is refused by name. |
| `package-release-macos.test.sh` | The release folder layout `scripts/package-release-macos.sh` would write (`--dry-run`). |

