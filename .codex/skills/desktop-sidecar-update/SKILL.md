---
name: desktop-sidecar-update
description: Update Desktop sidecars, local Agent packaging, or platform-specific binaries.
---
<!-- GENERATED FILE - DO NOT EDIT DIRECTLY -->
<!-- Source: skills/desktop/desktop-sidecar-update/SKILL.md -->


# Desktop Sidecar Update

Use this skill when changing Desktop packaged components.

## Check

- Component version and hash are recorded.
- Windows x64, macOS Intel, and macOS Apple Silicon handling is explicit.
- User data directory is not overwritten.
- Vue does not gain direct access to Local Agent token or port.
- Rust remains the bridge for system capabilities.

## Output

Summarize component changes, affected platforms, rollback, and verification.
