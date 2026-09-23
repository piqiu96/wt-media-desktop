//! Starting and stopping the Local Agent sidecar.
//!
//! Both spawn paths live here so the command layer never holds a `CommandChild`
//! and never has to know which path produced it. The bundled sidecar is the only
//! path a release build can take; the Python fallback sits behind a debug-build
//! opt-in and is unreachable from a release bundle, so customers never need a
//! system Python install.
//!
//! The label each path returns is preserved verbatim from the command bodies
//! this was extracted from (`sidecar_started` / `started` / `already_running` /
//! `not_running`), because the Vue layer matches on them.

use crate::state::SidecarLog;
use tauri::AppHandle;
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

pub mod drain;

/// Start the Local Agent, returning the path label and the child handle.
///
/// A failed bundled-sidecar spawn is not reported: with the fallback enabled it
/// falls through to the Python path, and without it the caller gets the
/// "reinstall" message — which is the message that helps a user either way. The
/// spawn error itself is deliberately not surfaced, as before.
///
/// Both paths hand their event receiver to `drain`, so whichever one ran, its
/// output is readable from `log` afterwards.
pub fn start(
    app: &AppHandle,
    allow_python_fallback: bool,
    log: &SidecarLog,
) -> Result<(&'static str, CommandChild), String> {
    const SIDECAR: &str = "sidecar_started";
    const FALLBACK: &str = "started";

    match app.shell().sidecar("wt-media-agent").map(|cmd| cmd.spawn()) {
        Ok(Ok((events, child))) => {
            drain::follow(events, log.clone(), SIDECAR);
            Ok((SIDECAR, child))
        }
        _ if allow_python_fallback => {
            let shell = app.shell();
            let (events, child) = shell
                .command("python3")
                .args(["-m", "wt_media_agent.local_api.server"])
                .spawn()
                .map_err(|e| format!("agent launch failed: {}", e))?;
            drain::follow(events, log.clone(), FALLBACK);
            Ok((FALLBACK, child))
        }
        _ => Err(
            "未找到或无法启动随应用提供的 Local Agent。请重新安装完整的 WT Media 安装包。".into(),
        ),
    }
}

/// Kill a running sidecar. Takes the handle by value because
/// `CommandChild::kill(self)` consumes it — the caller has already `take`n it
/// out of the managed state, so there is nothing left to hold.
pub fn stop(child: CommandChild) -> Result<String, String> {
    child
        .kill()
        .map(|_| "stopped".into())
        .map_err(|e| format!("agent stop failed: {}", e))
}
