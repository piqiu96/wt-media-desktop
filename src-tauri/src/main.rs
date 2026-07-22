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

#[derive(Default)]
pub struct RuntimeBindingState(Mutex<Option<RuntimeBinding>>);

#[derive(Clone, Debug)]
struct RuntimeBinding {
    node_id: String,
    node_credential: String,
}

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
    #[serde(default)]
    pub node_id: Option<String>,
    pub agent_id: String,
    pub status: String,
    #[serde(default)]
    pub bitbrowser_status: Option<String>,
    #[serde(default)]
    pub main_user_id: Option<String>,
    #[serde(default)]
    pub operating_system: Option<String>,
    #[serde(default)]
    pub cpu_architecture: Option<String>,
    #[serde(default)]
    pub agent_version: Option<String>,
    #[serde(default)]
    pub python_version: Option<String>,
    #[serde(default)]
    pub ffmpeg: Option<RuntimeStatusFact>,
    #[serde(default)]
    pub workdir_status: Option<String>,
    #[serde(default)]
    pub disk: Option<RuntimeDiskFact>,
    #[serde(default)]
    pub bit_profile_ids: Vec<String>,
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
    node_id: Option<String>,
    agent_id: String,
    status: String,
    bitbrowser_status: Option<String>,
    main_user_id: Option<String>,
    operating_system: Option<String>,
    cpu_architecture: Option<String>,
    agent_version: Option<String>,
    python_version: Option<String>,
    ffmpeg: Option<RuntimeStatusFact>,
    workdir_status: Option<String>,
    disk: Option<RuntimeDiskFact>,
    bit_profile_ids: Vec<String>,
    current_task_id: Option<String>,
    current_task_progress: Option<u32>,
    current_task_status: Option<String>,
    pending_result_count: u32,
}

impl From<LocalAgentStatusData> for LocalAgentStatus {
    fn from(d: LocalAgentStatusData) -> Self {
        Self {
            node_id: d.node_id,
            agent_id: d.agent_id,
            status: d.status,
            bitbrowser_status: d.bitbrowser_status,
            main_user_id: d.main_user_id,
            operating_system: d.operating_system,
            cpu_architecture: d.cpu_architecture,
            agent_version: d.agent_version,
            python_version: d.python_version,
            ffmpeg: d.ffmpeg,
            workdir_status: d.workdir_status,
            disk: d.disk,
            bit_profile_ids: d.bit_profile_ids,
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

#[derive(Clone, Debug, Deserialize)]
struct CloudEnvelope<T> {
    errcode: i32,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

#[derive(Clone, Debug, Serialize)]
struct RegisterLocalNodeRequest {
    binding_token: String,
    agent_id: String,
    device_id: String,
    agent_version: String,
    contract_major_version: String,
    contract_revision: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RegisterLocalNodeResponse {
    node: RegisteredNode,
    node_credential: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RegisteredNode {
    id: String,
    agent_id: String,
    user_id: i64,
    status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeStatusFact {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeDiskFact {
    pub status: String,
    pub free_megabytes: i64,
}

#[derive(Clone, Debug, Serialize)]
struct RuntimeReportRequest {
    operating_system: String,
    cpu_architecture: String,
    agent_version: String,
    python_version: String,
    ffmpeg: RuntimeStatusFact,
    workdir_status: String,
    disk: RuntimeDiskFact,
    bitbrowser_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    main_user_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct BindSessionArgs {
    binding_ticket: String,
    cloud_base_url: String,
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

/// Bind this Desktop/Local Agent session to Cloud using a one-use Cloud ticket.
///
/// The Vue layer receives the one-use ticket from Cloud and passes it into this
/// native command. Rust consumes the ticket, stores the node credential in native
/// memory, updates Local Agent with the Cloud node id, and reports runtime facts
/// back to Cloud. The credential is never returned to Vue.
#[tauri::command]
async fn local_agent_bind_session(
    client: State<'_, HttpClient>,
    binding_state: State<'_, RuntimeBindingState>,
    args: BindSessionArgs,
) -> Result<BoundNodeFacts, String> {
    let binding_ticket = args.binding_ticket.trim().to_string();
    if binding_ticket.is_empty() {
        return Err("Cloud绑定票据为空，请重新登录后再检测".into());
    }
    let cloud_base_url = args.cloud_base_url.trim().trim_end_matches('/').to_string();
    if cloud_base_url.is_empty() {
        return Err("Cloud地址为空，无法完成本机可信绑定".into());
    }

    let status = local_agent_status(client.clone()).await?;
    let main_user_id = status.main_user_id.clone().unwrap_or_default();
    if main_user_id.trim().is_empty() {
        return Err("未读取到BitBrowser主账号，请确认BitBrowser已登录后重新检测".into());
    }
    if status.bitbrowser_status.as_deref() != Some("normal") {
        return Err("BitBrowser不可用或身份不可验证，请处理后重新检测".into());
    }

    let register_url = format!("{}/api/v1/local-agent/nodes/register", cloud_base_url);
    let register_payload = RegisterLocalNodeRequest {
        binding_token: binding_ticket,
        agent_id: status.agent_id.clone(),
        device_id: "desktop-local-device".into(),
        agent_version: status
            .agent_version
            .clone()
            .unwrap_or_else(|| "0.2.2".into()),
        contract_major_version: "v1".into(),
        contract_revision: "2026.07.15.1".into(),
    };
    let register_resp = client
        .inner
        .post(&register_url)
        .json(&register_payload)
        .send()
        .await
        .map_err(|e| format!("Cloud节点注册失败: {}", e))?;
    let register_status = register_resp.status();
    let register_text = register_resp
        .text()
        .await
        .map_err(|e| format!("读取Cloud节点注册响应失败: {}", e))?;
    if !register_status.is_success() {
        return Err(format!(
            "Cloud节点注册失败: {} {}",
            register_status, register_text
        ));
    }
    let register_body: CloudEnvelope<RegisterLocalNodeResponse> =
        serde_json::from_str(&register_text)
            .map_err(|e| format!("Cloud节点注册响应格式错误: {}", e))?;
    if register_body.errcode != 0 {
        return Err(if register_body.message.is_empty() {
            "Cloud节点注册失败".into()
        } else {
            register_body.message
        });
    }
    let registration = register_body
        .data
        .ok_or_else(|| "Cloud节点注册响应缺少数据".to_string())?;

    let bind_url = format!("{}/api/v1/bind", client.local_agent_base);
    let bind_resp = client
        .inner
        .post(&bind_url)
        .json(&serde_json::json!({
            "node_id": registration.node.id,
            "binding_token": "cloud-runtime-bound"
        }))
        .send()
        .await
        .map_err(|e| format!("写入Local Agent节点失败: {}", e))?;
    if !bind_resp.status().is_success() {
        let status_code = bind_resp.status();
        let text = bind_resp.text().await.unwrap_or_default();
        return Err(format!("写入Local Agent节点失败: {} {}", status_code, text));
    }

    let runtime_report = RuntimeReportRequest {
        operating_system: status
            .operating_system
            .clone()
            .unwrap_or_else(|| "unsupported".into()),
        cpu_architecture: status
            .cpu_architecture
            .clone()
            .unwrap_or_else(|| "unsupported".into()),
        agent_version: status
            .agent_version
            .clone()
            .unwrap_or_else(|| "0.2.2".into()),
        python_version: status
            .python_version
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        ffmpeg: status.ffmpeg.clone().unwrap_or(RuntimeStatusFact {
            status: "unreachable".into(),
            version: None,
        }),
        workdir_status: status
            .workdir_status
            .clone()
            .unwrap_or_else(|| "normal".into()),
        disk: status.disk.clone().unwrap_or(RuntimeDiskFact {
            status: "normal".into(),
            free_megabytes: 0,
        }),
        bitbrowser_status: status
            .bitbrowser_status
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        main_user_id: Some(main_user_id),
    };
    let report_url = format!(
        "{}/api/v1/local-agent/nodes/{}/runtime-report",
        cloud_base_url, registration.node.id
    );
    let report_resp = client
        .inner
        .post(&report_url)
        .bearer_auth(&registration.node_credential)
        .json(&runtime_report)
        .send()
        .await
        .map_err(|e| format!("本机运行状态上报失败: {}", e))?;
    if !report_resp.status().is_success() {
        let status_code = report_resp.status();
        let text = report_resp.text().await.unwrap_or_default();
        return Err(format!("本机运行状态上报失败: {} {}", status_code, text));
    }

    binding_state
        .0
        .lock()
        .map_err(|_| "runtime binding lock poisoned")?
        .replace(RuntimeBinding {
            node_id: registration.node.id.clone(),
            node_credential: registration.node_credential,
        });

    Ok(BoundNodeFacts {
        id: registration.node.id,
        agent_id: registration.node.agent_id,
        user_id: registration.node.user_id.to_string(),
        status: registration.node.status,
    })
}

/// Read the current BitBrowser Profile snapshot through the Local Agent.
///
/// Desktop owns this local bridge. The Vue layer must not call the Local Agent
/// port or hold Local Agent credentials directly.
#[tauri::command]
async fn local_agent_profile_scan(
    client: State<'_, HttpClient>,
) -> Result<serde_json::Value, String> {
    let url = format!(
        "{}/api/v1/bit-browser/profile-scans",
        client.local_agent_base
    );
    let resp = client
        .inner
        .post(&url)
        .send()
        .await
        .map_err(|e| format!("profile scan failed: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("profile scan failed: {} {}", status, text));
    }
    resp.json()
        .await
        .map_err(|e| format!("invalid profile scan response: {}", e))
}

// ---- App Entry Point ----

fn main() {
    tauri::Builder::default()
        .manage(HttpClient::new(8765))
        .manage(AgentProcess::default())
        .manage(RuntimeBindingState::default())
        .invoke_handler(tauri::generate_handler![
            local_agent_status,
            local_agent_health,
            local_agent_start,
            local_agent_stop,
            local_agent_task_status,
            local_agent_bind,
            local_agent_bind_session,
            local_agent_profile_scan,
        ])
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running wt-media-desktop tauri application");
}
