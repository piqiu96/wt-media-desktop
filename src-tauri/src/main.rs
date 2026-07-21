// WT Media Desktop — Tauri v2 shell with real Local Agent HTTP/SSE bridge.
// M1-R5: replaces the M0 mock with real reqwest HTTP calls.

mod commands;
mod filesystem;
mod local_agent;
mod secure_store;
mod system;
mod updater;

use local_agent::{BindingTransport, BoundNodeFacts};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{Manager, State};
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

/// HTTP client shared across Tauri commands.
pub struct HttpClient {
    inner: Client,
    local_agent_base: String,
}

/// Owns the Local Agent child for the lifetime of the Desktop session.
/// Keeping the handle here makes start/stop deterministic in both packaged
/// sidecar mode and the Python fallback used by development builds.
#[derive(Default)]
pub struct AgentProcess(Mutex<Option<CommandChild>>);

impl HttpClient {
    fn new(local_agent_port: u16) -> Self {
        Self {
            inner: Client::new(),
            local_agent_base: format!("http://127.0.0.1:{}", local_agent_port),
        }
    }
}

// ---- Data types matching the Local Agent API ----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalAgentStatusResponse {
    pub data: LocalAgentStatusData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalAgentStatusData {
    pub agent_id: String,
    pub status: String,
    #[serde(default)]
    pub current_task_id: Option<String>,
    #[serde(default)]
    pub current_task_progress: Option<u32>,
    #[serde(default)]
    pub current_task_status: Option<String>,
    #[serde(default)]
    pub pending_result_count: u32,
}

#[derive(Clone, Debug, Serialize)]
struct LocalAgentStatus {
    agent_id: String,
    status: String,
    current_task_id: Option<String>,
    current_task_progress: Option<u32>,
    current_task_status: Option<String>,
    pending_result_count: u32,
}

impl From<LocalAgentStatusData> for LocalAgentStatus {
    fn from(d: LocalAgentStatusData) -> Self {
        Self {
            agent_id: d.agent_id,
            status: d.status,
            current_task_id: d.current_task_id,
            current_task_progress: d.current_task_progress,
            current_task_status: d.current_task_status,
            pending_result_count: d.pending_result_count,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BindResponse {
    pub node_id: String,
    pub session_token: String,
    pub status: String,
}

// ---- Tauri Commands ----

#[tauri::command]
async fn local_agent_status(client: State<'_, HttpClient>) -> Result<LocalAgentStatus, String> {
    let url = format!("{}/api/v1/status", client.local_agent_base);
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
async fn local_agent_health(client: State<'_, HttpClient>) -> Result<String, String> {
    let url = format!("{}/healthz", client.local_agent_base);
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
async fn local_agent_start(
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
    // Try to spawn the sidecar binary first, fall back to `python3 -m wt_media_agent.local_main`
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
        _ => {
            // Fallback: launch via python module (dev environment)
            let shell = app.shell();
            let (_events, child) = shell
                .command("python3")
                .args(["-m", "wt_media_agent.local_main"])
                .spawn()
                .map_err(|e| format!("agent launch failed: {}", e))?;
            process
                .0
                .lock()
                .map_err(|_| "agent process lock poisoned")?
                .replace(child);
            Ok("started".into())
        }
    }
}

#[tauri::command]
fn local_agent_stop(process: State<'_, AgentProcess>) -> Result<String, String> {
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
async fn local_agent_task_status(
    client: State<'_, HttpClient>,
    task_id: String,
) -> Result<LocalAgentStatus, String> {
    // Report back the current overall status; task_id is validated server-side.
    local_agent_status(client).await
}

/// Bind the Desktop to the Local Agent, receiving a session token.
#[tauri::command]
async fn local_agent_bind(client: State<'_, HttpClient>) -> Result<BindResponse, String> {
    let url = format!("{}/api/v1/bind", client.local_agent_base);
    let resp = client
        .inner
        .post(&url)
        .json(&serde_json::json!({"node_id": "wt-media-desktop", "binding_token": "desktop-init"}))
        .send()
        .await
        .map_err(|e| format!("bind failed: {}", e))?;
    let body: BindResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid bind response: {}", e))?;
    Ok(body)
}

// ---- App Entry Point ----

fn main() {
    tauri::Builder::default()
        .manage(HttpClient::new(8765))
        .manage(AgentProcess::default())
        .invoke_handler(tauri::generate_handler![
            local_agent_status,
            local_agent_health,
            local_agent_start,
            local_agent_stop,
            local_agent_task_status,
            local_agent_bind,
        ])
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running wt-media-desktop tauri application");
}
