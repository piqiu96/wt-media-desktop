---
name: release-cross-platform-verification
description: Verify a validated Cloud, Agent, and Desktop release combination.
---
<!-- GENERATED FILE - DO NOT EDIT DIRECTLY -->
<!-- Source: skills/common/release-cross-platform-verification/SKILL.md -->


# Release Cross-Platform Verification

Use this skill when preparing or reviewing a release matrix update.

## Check

- Cloud version and migrations are recorded.
- Local Agent and Cloud Agent versions are recorded.
- Desktop version and target platforms are recorded.
- Contract revisions match provider repositories.
- Windows x64, macOS Intel, and macOS Apple Silicon verification status is explicit.
- External platform e2e tests use isolated accounts and explicit opt-in.

## Output

Update or review `config/release-matrix.yaml` and list residual risks.
