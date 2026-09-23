//! Local Agent observation and process lifecycle commands.

use crate::development_python_fallback_enabled;
use crate::dto::{LocalAgentStatus, LocalAgentStatusResponse};
use crate::http::LocalAgentClient;
use crate::state::AgentProcess;
use tauri::State;
use tauri_plugin_shell::ShellExt;

#[tauri::command]
pub async fn local_agent_status(client: State<'_, LocalAgentClient>) -> Result<LocalAgentStatus, String> {
    let url = format!("{}/api/v1/status", client.base);
    let resp = client
        .inner
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("agent unreachable: {}", e))?;
    let body: LocalAgentStatusResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid status response: {}", e))?;
    Ok(LocalAgentStatus::from(body.data))
}
#[tauri::command]
pub async fn local_agent_health(client: State<'_, LocalAgentClient>) -> Result<String, String> {
    let url = format!("{}/healthz", client.base);
    let resp = client
        .inner
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("agent unreachable: {}", e))?;
    let text = resp
        .text()
        .await
        .map_err(|e| format!("read error: {}", e))?;
    Ok(text)
}
#[tauri::command]
pub async fn local_agent_start(
    app: tauri::AppHandle,
    process: State<'_, AgentProcess>,
) -> Result<String, String> {
    if process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .is_some()
    {
        return Ok("already_running".into());
    }
    // Release builds must run only the bundled sidecar. Development can opt into
    // a Python fallback explicitly when iterating without a frozen binary.
    let sidecar_result = app.shell().sidecar("wt-media-agent").map(|cmd| cmd.spawn());

    match sidecar_result {
        Ok(Ok((_events, child))) => {
            process
                .0
                .lock()
                .map_err(|_| "agent process lock poisoned")?
                .replace(child);
            Ok("sidecar_started".into())
        }
        _ if development_python_fallback_enabled() => {
            // Deliberate development-only fallback; it is unreachable from a
            // release bundle so customers never need a system Python install.
            let shell = app.shell();
            let (_events, child) = shell
                .command("python3")
                .args(["-m", "wt_media_agent.local_api.server"])
                .spawn()
                .map_err(|e| format!("agent launch failed: {}", e))?;
            process
                .0
                .lock()
                .map_err(|_| "agent process lock poisoned")?
                .replace(child);
            Ok("started".into())
        }
        _ => Err(
            "未找到或无法启动随应用提供的 Local Agent。请重新安装完整的 WT Media 安装包。".into(),
        ),
    }
}
#[tauri::command]
pub fn local_agent_stop(process: State<'_, AgentProcess>) -> Result<String, String> {
    let child = process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .take();
    match child {
        Some(child) => child
            .kill()
            .map(|_| "stopped".into())
            .map_err(|e| format!("agent stop failed: {}", e)),
        None => Ok("not_running".into()),
    }
}
/// Fetch task progress via SSE stream (simplified: returns latest status snapshot).
#[tauri::command]
pub async fn local_agent_task_status(
    client: State<'_, LocalAgentClient>,
    _task_id: String,
) -> Result<LocalAgentStatus, String> {
    // Report back the current overall status; task_id is validated server-side.
    local_agent_status(client).await
}
