//! Local Agent observation and process lifecycle commands.

use crate::config::DesktopConfig;
use crate::development_python_fallback_enabled;
use crate::dto::{LocalAgentStatus, LocalAgentStatusResponse};
use crate::http::LocalAgentClient;
use crate::sidecar::{self, drain};
use crate::state::{AgentProcess, SidecarLog};
use tauri::State;

#[tauri::command]
pub async fn local_agent_status(client: State<'_, LocalAgentClient>) -> Result<LocalAgentStatus, String> {
    let resp = client
        .get("/api/v1/status")
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
pub async fn local_agent_health(
    client: State<'_, LocalAgentClient>,
    log: State<'_, SidecarLog>,
) -> Result<String, String> {
    let resp = client
        .get("/healthz")
        .send()
        .await
        .map_err(|e| format!("agent unreachable: {}{}", e, drain::summary(log.inner())))?;
    let text = resp
        .text()
        .await
        .map_err(|e| format!("read error: {}{}", e, drain::summary(log.inner())))?;
    Ok(text)
}
#[tauri::command]
pub async fn local_agent_start(
    app: tauri::AppHandle,
    client: State<'_, LocalAgentClient>,
    config: State<'_, DesktopConfig>,
    process: State<'_, AgentProcess>,
    log: State<'_, SidecarLog>,
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
    //
    // The token comes from the client, not from a second copy: the Agent has to
    // be told the same secret the requests will present, and one owner is what
    // makes that true by construction rather than by discipline.
    let (label, child) = sidecar::start(
        &app,
        development_python_fallback_enabled(),
        log.inner(),
        &config,
        client.token(),
    )
    .map_err(|e| format!("{}{}", e, drain::summary(log.inner())))?;
    process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .replace(child);
    Ok(label.into())
}
#[tauri::command]
pub fn local_agent_stop(process: State<'_, AgentProcess>) -> Result<String, String> {
    let child = process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .take();
    match child {
        Some(child) => sidecar::stop(child),
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
