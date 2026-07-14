# wt-media-desktop

Tauri desktop shell skeleton for the modular social media operations platform.

## Responsibilities

- Desktop Tauri/Vue project boundary.
- Rust bridge placeholders for file selection, secure storage, system info, and updater.
- Desktop-only local control pages.
- Cross-platform packaging for Windows, macOS Intel, and macOS Apple Silicon.

## Bootstrap

This repository is scaffolded as a Tauri 2 + Vue 3 shell. Node, Rust, and Tauri dependencies are not installed yet.

## Key Directories

- `src`: Desktop local Vue pages and frontend services.
- `src-tauri/src`: Rust bridge modules.
- `src-tauri/binaries`: future sidecar component placeholders.
- `src-tauri/capabilities`: Tauri permissions.
- `contracts.lock.json`: consumed contract versions.

## M0 Verification

From this repository:

```text
npm run verify
```
