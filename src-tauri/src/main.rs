// WT Media Desktop — Tauri v2 shell with a real Local Agent HTTP bridge.
//
// The bridge is HTTP only: the Agent does expose a `text/event-stream` endpoint
// (`local_api/server.py`), but nothing here consumes a stream —
// `local_agent_task_status` reads a status snapshot.
// M1-R5: replaces the M0 mock with real reqwest HTTP calls.

mod app_paths;
mod bootstrap;
mod commands;
mod config;
mod dto;
mod filesystem;
mod http;
mod local_agent;
mod logging;
mod paths;
mod preflight;
mod secure_store;
mod settings;
mod sidecar;
mod state;
mod system;
mod token;
mod updater;

use http::{CloudClient, LocalAgentClient};
use state::{AgentProcess, OperationId, RuntimeBindingState, SidecarLog};
use std::path::PathBuf;
use tauri::Manager;
use token::RuntimeToken;

fn python_fallback_allowed(debug_build: bool, explicitly_enabled: bool) -> bool {
    debug_build && explicitly_enabled
}

pub(crate) fn development_python_fallback_enabled() -> bool {
    python_fallback_allowed(
        cfg!(debug_assertions),
        matches!(
            std::env::var("WT_MEDIA_DESKTOP_ALLOW_PYTHON_FALLBACK").as_deref(),
            Ok("1")
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_fallback_requires_debug_build_and_explicit_opt_in() {
        assert!(!python_fallback_allowed(false, true));
        assert!(!python_fallback_allowed(true, false));
        assert!(python_fallback_allowed(true, true));
    }
}

// ---- App Entry Point ----

fn main() {
    // The config is resolved *before* the app is built, because the CSP has to
    // be set on the `Context`: Tauri copies the context's config into the
    // `AppManager`, and that copy is what it reads when it serves the window.
    // Once `Builder::run` has been handed the context there is no way back to
    // it — see `bootstrap`'s header for the call chain.
    let mut context = tauri::generate_context!();
    let startup = bootstrap::resolve(context.package_info());
    bootstrap::apply_csp(context.config_mut(), &startup.config);

    // One token per launch, generated here and handed to the client that owns
    // it. Nothing persists it and nothing passes it on a command line: it
    // reaches the Agent through the client's own requests, and (from T-07's
    // sidecar commit) through the child's environment.
    //
    // Generated before logging is installed because the sink needs it: the token
    // is named by no key and matches no shape rule, so the only way it can be
    // kept out of a log line is for the mask to hold the value itself.
    //
    // The value is copied out here rather than read at the sink, because the
    // builder below hands the token itself to the client and the sink is only
    // installed after the builder's plugin phase has run.
    let token = RuntimeToken::generate();
    let secret = token.expose().to_string();

    // ---- The builder, up to and including its plugin phase ----
    //
    // This is a `build` and not a `run`, and the single-instance guard below is
    // the reason (CHG-058 §6 D-04). Plugins are initialised inside
    // `Builder::build` — tauri 2.11.5 `app.rs:2440`, `initialize_plugins` — and
    // this plugin answers a second launch with `std::process::exit(0)` from its
    // own `setup`. So a one-call `Builder::run(context)` would let a *doomed*
    // process get all the way to the logging sink below and write a record
    // before the guard ever ran, which is precisely the damage the guard is
    // here to prevent:
    //
    // `desktop.log` is a stable name that `file-rotate` renames on every hourly
    // roll, on its documented assumption that no other process moves files in
    // the log directory. A second instance breaks that assumption badly — it
    // appends to the same live file, and when its write lands in an hour other
    // than the file's mtime it *rolls*: it renames `desktop.log` to the archive
    // and creates a fresh one, while the first instance's handle still points at
    // the inode that just moved. The first instance then keeps writing real
    // records into an archive, and the file named `desktop.log` holds one stray
    // line. There is no multi-instance scenario we want: the guard forbids a bad
    // state, it does not enable a capability.
    //
    // Splitting the chain puts the guard in front of everything that touches the
    // log directory. `run`'s own doc names the split as the supported way to get
    // at this seam ("for more flexibility, consider using those functions
    // manually"). The residual race is the plugin's and is its own: two launches
    // inside the same instant can both find no socket and both claim singleton
    // (`macos.rs` says as much). Sequential launches — what a user does — are
    // the case this covers.
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // The second launch raises the running window instead of starting a
            // second copy. A window the user has closed is destroyed rather than
            // hidden, so `get_webview_window` finds nothing and there is nothing
            // to raise.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .manage(LocalAgentClient::new(&startup.config, token))
        .manage(CloudClient::new(&startup.config))
        // The config is managed as its own state as well as being consumed
        // during startup: `get_public_config` answers from this copy, so the
        // page and `apply_csp` can never be reading two different configs.
        .manage(startup.config.clone())
        .manage(AgentProcess::default())
        // Beside the process handle, because the two answer the same question:
        // `start` sets the id when it stores a child, `stop` clears it when it
        // takes the child away. In-process only (D-10).
        .manage(OperationId::default())
        .manage(RuntimeBindingState::default())
        .manage(SidecarLog::default())
        .setup(|_app| {
            // WebView 报错转发的注册点在**前端**（`web/src/apps/desktop/webviewErrors.js`），
            // 不在这里用 `window.eval`。
            //
            // 原因：应用 CSP 的 script-src 回落到 default-src 'self' 且不含 'unsafe-eval'，
            // `window.eval` 会被 WebView 直接拒绝——这段注入在实际运行中从未生效，
            // 原生端因此一行报错都看不到（2026-09-23 实测确认）。
            // 前端入口本身就是 'self' 加载的脚本，不受该限制；这样也无需为了
            // 日志给 CSP 开 'unsafe-eval'。落点仍是下方 `log_js_error`。
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::agent::local_agent_status,
            commands::agent::local_agent_health,
            commands::agent::local_agent_start,
            commands::agent::local_agent_stop,
            commands::agent::local_agent_task_status,
            commands::bind::local_agent_bind,
            commands::bind::local_agent_bind_session,
            commands::bind::local_agent_refresh_runtime,
            commands::account::local_agent_account_check,
            commands::account::local_agent_cookie_read,
            commands::profile::local_agent_profile_scan,
            commands::profile::local_agent_profile_groups,
            commands::profile::local_agent_profile_open,
            commands::profile::local_agent_profile_close,
            commands::profile::local_agent_profile_create,
            commands::profile::local_agent_profile_restore,
            commands::webview::log_js_error,
            // Appended, not inserted: `localAgentService.test.js` asserts on exact
            // argument objects, and the existing seventeen keep their positions.
            commands::public_config::get_public_config,
        ])
        .plugin(tauri_plugin_shell::init())
        .build(context)
        .expect("error while building wt-media-desktop tauri application");

    // ---- Past the guard: this process is the only one ----
    //
    // Nothing has been logged before this point and nothing wants to: the
    // config layer reports through the summary it returns, and there is no
    // `log`-crate bridge for Tauri's own internals to arrive through. So the
    // launch summary below is still the **first record** in `desktop.log` —
    // now from a process that is also the only one writing that file.
    //
    // The environment in the plan is the build's, not
    // `startup.config.environment`'s: the two disagree on every ordinary debug
    // launch (the shipped file says production and `load_with` takes the
    // stricter of the two), and the directory has to follow the build or the
    // development layout would never be used. Both are named in the summary,
    // which is the line a reader starts from.
    let plan = logging::setup::plan(
        &startup.config,
        bootstrap::build_environment(),
        // `HOME` is read here and injected downward: `logging::paths` never
        // touches the environment, so its layout rules stay askable.
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    let installed = logging::setup::install(plan, vec![secret]);
    let summary = format!("{}；{}", startup.summary(), installed.summary());
    if installed.installed {
        // stderr as before: `assemble` always attaches the terminal layer, and
        // the file is where the same line becomes a record.
        tracing::info!(target: "desktop.startup", "{summary}");
    } else {
        // A subscriber was already installed, so the record would go nowhere.
        // The summary is the one line a launch must not lose, so it falls back
        // to the plain write this replaces.
        eprintln!("[wt-media-desktop] {summary}");
    }

    app.run(|_app, _event| {});
}
