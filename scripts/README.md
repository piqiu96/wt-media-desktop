# Desktop Scripts

Expose common commands with stable names:

- `bootstrap.sh`: install locked npm dependencies with `npm ci`.
- `test.sh`: run Desktop verification (`npm test`).
- `build.sh`: run Vite build and Tauri build.
- `dev.sh`: run the foreground Vite dev server.
- `start.sh`: start the Vite dev server in the background.
- `health.sh`: verify static Desktop health and the running dev page.
- `stop.sh`: stop the background Vite dev server.

The package scripts also expose `bootstrap`, `dev`, `start`, `stop`, `test`, `lint`, `generate`, `build`, `package`, `verify`, and `health`.
