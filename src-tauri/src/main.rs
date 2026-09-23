// WT Media Desktop — Tauri v2 shell with real Local Agent HTTP/SSE bridge.
// M1-R5: replaces the M0 mock with real reqwest HTTP calls.

mod bootstrap;
mod commands;
mod config;
mod dto;
mod filesystem;
mod http;
mod local_agent;
mod paths;
mod preflight;
mod secure_store;
mod sidecar;
mod state;
mod system;
mod token;
mod updater;

use http::{CloudClient, LocalAgentClient};
use state::{AgentProcess, RuntimeBindingState, SidecarLog};
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
    eprintln!("[wt-media-desktop] {}", startup.summary());

    // One token per launch, generated here and handed to the client that owns
    // it. Nothing persists it and nothing passes it on a command line: it
    // reaches the Agent through the client's own requests, and (from T-07's
    // sidecar commit) through the child's environment.
    let token = RuntimeToken::generate();

    tauri::Builder::default()
        .manage(LocalAgentClient::new(&startup.config, token))
        .manage(CloudClient::new(&startup.config))
        .manage(AgentProcess::default())
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
            commands::logging::log_js_error,
        ])
        .plugin(tauri_plugin_shell::init())
        .run(context)
        .expect("error while running wt-media-desktop tauri application");
}
