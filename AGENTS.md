# WT Media Desktop

## Responsibility

Desktop owns the Tauri shell, local control pages, Rust system bridge, Local Agent lifecycle, secure storage, file selection, updater, and platform packaging.

## Structure

- `src`: Vue local control pages and frontend services.
- `src-tauri/src`: Rust bridge modules.
- `src-tauri/binaries`: sidecar component placeholders.
- `src-tauri/capabilities`: Tauri permission capabilities.
- `contracts.lock.json`: consumed contract version lock.
- `packaging`: installers and release assets.

## Rules

- Desktop does not duplicate the Cloud business Web.
- Vue does not directly access Local Agent dynamic ports or tokens.
- Rust proxies Local Agent HTTP and SSE.
- Sensitive tokens go through OS secure storage, not localStorage.
- Platform-specific behavior belongs in Rust system bridge modules, not scattered business UI code.
