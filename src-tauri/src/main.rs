// WT Media Desktop — Tauri v2 shell with a real Local Agent HTTP bridge.
//
// The bridge is HTTP only: the Agent does expose a `text/event-stream` endpoint
// (`local_api/server.py`), but nothing here consumes a stream —
// `local_agent_task_status` reads a status snapshot.
// M1-R5: replaces the M0 mock with real reqwest HTTP calls.

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app_paths;
mod bootstrap;
mod cleanup;
mod commands;
mod config;
mod device_identity;
mod diagnostic;
mod dto;
mod external_links;
mod filesystem;
mod http;
mod local_agent;
mod logging;
mod migration;
mod paths;
mod preflight;
mod saved_files;
mod secure_store;
mod settings;
mod sidecar;
mod state;
mod storage;
mod system;
mod system_paths;
mod token;
mod updater;
// T-07's criterion is a test-only module on purpose: there is no upgrade action
// to change, so what has to exist is the declaration and the arms, not a guard
// with no caller. See the module's header.
#[cfg(test)]
mod upgrade;

use http::{CloudClient, LocalAgentClient};
use state::{AgentProcess, OperationId, RuntimeBindingState, SidecarLog};
use tauri::{Manager, RunEvent};
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
    let system = crate::system_paths::SystemPaths::init_from_env();
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

    // **One list, two readers.** The sink that masks every record and the
    // diagnostic bundle that masks every entry are handed the *same* list, built
    // here once. Two `vec![secret]` literals would be two places to remember, and
    // the one that got forgotten would be the one that writes a credential out.
    // Whether `install` and `DiagnosticHost` between them cover everything that
    // needs masking is not something this file can assert — the launch's own
    // token reaching this list is a wiring line, registered as a boundary in the
    // T-07 evidence rather than claimed as tested.
    let secrets = vec![secret];
    let windows_migration = std::sync::Arc::new(std::sync::Mutex::new(
        Ok(None) as Result<Option<migration::MigrationReport>, String>
    ));
    let setup_migration = std::sync::Arc::clone(&windows_migration);
    let migration_system = system.clone();

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
        // The facts about this launch that only startup knows: which config
        // candidate was used, why a located file was refused, and the values the
        // diagnostic export must mask. Managed here rather than re-derived in the
        // command, for the reason `commands::storage` calls the launch's own
        // resolvers: a second resolution is a second answer.
        .manage(commands::diagnostic::DiagnosticHost::new(
            startup.source,
            startup.rejected.clone(),
            secrets.clone(),
        ))
        .manage(AgentProcess::default())
        .manage(system.clone())
        // Beside the process handle, because the two answer the same question:
        // `start` sets the id when it stores a child, `stop` clears it when it
        // takes the child away. In-process only (D-10).
        .manage(OperationId::default())
        .manage(RuntimeBindingState::default())
        .manage(SidecarLog::default())
        .setup(move |app| {
            // Preserve data written by the first Windows layout before the main
            // window exists. The single-instance plugin has already initialized,
            // so a second launch does not reach this filesystem copy, and no
            // frontend command can observe the old/new roots mid-migration.
            *setup_migration
                .lock()
                .expect("Windows migration report lock poisoned") =
                if migration_system.platform == system_paths::Platform::Windows {
                    let new_data = app_paths::resolve(
                        &migration_system,
                        bootstrap::build_environment(),
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
                    )
                    .map(|paths| paths.data)
                    .map_err(|error| format!("无法确定 Windows Desktop 数据目录: {error}"));
                    migration_system
                        .legacy_windows_desktop_data_dir()
                        .map_err(|error| format!("无法确定旧 Windows Desktop 数据目录: {error}"))
                        .and_then(|old| {
                            let new_data = match new_data {
                                Ok(new_data) => new_data,
                                Err(error) => return Err(error),
                            };
                            migration::migrate_windows_legacy_data(&old, &new_data)
                                .map_err(|error| error.to_string())
                        })
                } else {
                    Ok(None)
                };

            // WebView 报错转发的注册点在**前端**（`web/src/apps/desktop/webviewErrors.js`），
            // 不在这里用 `window.eval`。
            //
            // 原因：应用 CSP 的 script-src 回落到 default-src 'self' 且不含 'unsafe-eval'，
            // `window.eval` 会被 WebView 直接拒绝——这段注入在实际运行中从未生效，
            // 原生端因此一行报错都看不到（2026-09-23 实测确认）。
            // 前端入口本身就是 'self' 加载的脚本，不受该限制；这样也无需为了
            // 日志给 CSP 开 'unsafe-eval'。落点仍是下方 `log_js_error`。
            //
            // The window is built here rather than declared, because the new
            // window handler has to be on its builder and `tauri.conf.json`'s
            // declaration is built before this closure runs. It is the same
            // window — `"create": false` only takes it out of Tauri's loop; see
            // `external_links`' header. A launch that got this far without a
            // window would be an app that looks hung, so a failure here is
            // taken rather than swallowed: it arrives as `Error::Setup`, which
            // is what `build`'s `.expect` below reports.
            //
            // Nothing is logged from here: the sink is installed after
            // `build` returns, which is why the launch summary is the first
            // record in `desktop.log`.
            external_links::install(app.handle())?;
            // Restore the node credential persisted by a previous bind so the
            // Agent can keep authenticating after a Desktop restart. It must
            // happen here rather than at `.manage()`: resolving the app data
            // directory needs an `AppHandle`, which does not exist yet when
            // state is registered. Absence is an unbound machine, not an error.
            app.state::<RuntimeBindingState>().restore(&app.handle());
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
            device_identity::local_device_identity,
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
            // T-05's read-only pair for the 本机设置 page, appended for the same
            // reason: inserting any of them would renumber the assertions above.
            commands::storage::local_storage_usage,
            commands::storage::local_log_files,
            commands::storage::local_log_tail,
            // T-06's pair, appended for the same reason as the three above.
            commands::cleanup::local_cache_cleanup,
            commands::cleanup::local_log_cleanup,
            // T-07's export, appended for the same reason as the five above.
            commands::diagnostic::local_diagnostic_export,
            // T-08's three openings for the 本机设置 page and the log viewer,
            // appended for the same reason as the eight above: `local_settings_set`
            // takes the operator's choice, `local_settings_get` reads it back, and
            // `local_open_place` shows one of this app's own folders in the file
            // manager. None of them takes a path from the page; the log viewer's
            // 「打开日志文件夹」 passes a place label, which is resolved here.
            commands::settings::local_settings_get,
            commands::settings::local_settings_set,
            commands::reveal::local_open_place,
            // T-04's three, appended for the same reason as the eleven above.
            // The picker is the only one that reaches a native surface; the other
            // two are the same loopback HTTP as `local_agent_bind`. Note that
            // `local_open_saved_file` takes a **name**, not a path, so it adds no
            // way for the page to name a file this process may open.
            commands::downloads::local_pick_save_directory,
            commands::downloads::local_push_save_directory,
            commands::downloads::local_open_saved_file,
            // T-08's four, appended for the same reason as the fourteen above.
            // They are the only commands in this app that **destroy** a file the
            // operator has: `local_delete_saved_files` removes downloads and
            // `local_move_saved_files` moves them between directories. Neither
            // takes a path — both take names, matched against the directories
            // `settings.toml` says this machine has written downloads into, which
            // is what keeps 「删掉这个下载」 from being 「删掉这个进程能碰到的任何
            // 东西」.
            commands::saved_files::local_saved_file_states,
            commands::saved_files::local_save_dir_migration_plan,
            commands::saved_files::local_move_saved_files,
            commands::saved_files::local_delete_saved_files,
            // Q-11's picker, appended for the same reason as the eighteen above.
            // It opens the same native dialog as `local_pick_save_directory` and
            // writes into the same history, but it is **not** a second way to set
            // the save directory: it names a place to look in, and new downloads
            // keep going where `save_dir` already points.
            commands::downloads::local_pick_search_directory,
            // T-17's reveal, appended for the same reason as the twenty above.
            // `local_open_saved_file` hands the file to whatever plays it; this
            // opens the folder it is in. It takes a **name** for the same reason
            // that one does, and it opens a directory rather than the file, so it
            // adds no way for the page to reach a path of its own choosing.
            commands::downloads::local_reveal_saved_file,
        ])
        .plugin(tauri_plugin_shell::init())
        // The folder picker. Registered next to the shell plugin rather than
        // before the single-instance guard above: a second instance exits before
        // ever reaching this line, and a plugin that was never initialised cannot
        // fail on a dialog nobody opened. `dialog:allow-open` in
        // `capabilities/default.json` is what the page is allowed to reach --
        // without it the command fails at runtime in a way that reads like a bug
        // in the command itself.
        .plugin(tauri_plugin_dialog::init())
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
        &system,
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    let installed = logging::setup::install(plan, secrets);
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

    // The copy itself ran during `setup`; only the report waits for the sink.
    match windows_migration
        .lock()
        .expect("Windows migration report lock poisoned")
        .clone()
    {
        Ok(Some(report)) if report.copied_files > 0 || report.skipped_files > 0 => {
            tracing::info!(
                target: "desktop.startup",
                copied_files = report.copied_files,
                skipped_files = report.skipped_files,
                "Windows legacy data migration completed"
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(
                target: "desktop.startup",
                error = %error,
                "Windows legacy data migration did not run"
            );
            eprintln!("[wt-media-desktop] {error}");
        }
    }

    app.run(|app, event| {
        // The one hook. `Exit` rather than `ExitRequested`: it fires after the
        // windows are gone and the loop is ending, so there is nothing left to
        // veto -- which is what makes it safe to wait inside. Quitting Desktop
        // used to leave the Agent running, still holding its port and unknown to
        // the next launch that would meet it (CHG-059 T-03).
        if let RunEvent::Exit = event {
            commands::agent::stop_at_exit(app);
        }
    });
}
