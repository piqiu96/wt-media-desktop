---
name: common-architecture-review
description: Review changes against WT Media architecture boundaries before modifying runtime repositories.
---
<!-- GENERATED FILE - DO NOT EDIT DIRECTLY -->
<!-- Source: skills/common/common-architecture-review/SKILL.md -->


# Common Architecture Review

Use this skill before architecture-sensitive changes in any repository.

## Check

- Cloud remains the business fact center.
- Agent remains the only execution entry for external platforms, files, FFmpeg, and browser automation.
- Desktop remains a local shell and does not duplicate Cloud business Web.
- Runtime repositories do not depend on Workspace at build or runtime.
- No microservices, MQ, dynamic plugins, or workflow engines are introduced without a decision record under `docs/decisions`.

## Output

Summarize affected boundaries, required contracts, and verification steps.
