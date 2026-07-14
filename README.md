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

The M2 Desktop boundary consumes Cloud-Agent API `v1@2026.07.14.6`, Local Agent API `v1@2026.07.14.6`, and Profile guard events `2026.07.14.8`.

Desktop page code uses a Desktop-owned service and store boundary. It does not directly access Local Agent dynamic ports, tokens, or storage.

The `local_agent_bind_session` native command accepts a short-lived one-use Cloud ticket. The Vue service passes it only as an invoke argument and normalizes the response so node/permit credentials are retained by the native/Agent boundary rather than application state.

## Verification

From this repository:

```text
npm run verify
```
