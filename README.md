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

## M1 Local Agent Control

The M1 Desktop scaffold consumes Local Agent API `v1@2026.07.14.5` and Local event schema `status@2026.07.14.5`.

Desktop page code uses a Desktop-owned service and store boundary. It does not directly access Local Agent dynamic ports, tokens, or storage.

## Verification

From this repository:

```text
npm run verify
```
