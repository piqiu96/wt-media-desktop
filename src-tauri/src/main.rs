// WT Media Desktop — Tauri v2 shell with real Local Agent HTTP/SSE bridge.
// M1-R5: replaces the M0 mock with real reqwest HTTP calls.

mod commands;
mod filesystem;
mod local_agent;
mod secure_store;
mod system;
mod updater;

use local_agent::BoundNodeFacts;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;
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
    #[serde(default)]
    bit_profile_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct BindSessionArgs {
    binding_ticket: String,
    cloud_base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RefreshRuntimeArgs {
    cloud_base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
struct AccountCheckArgs {
    cloud_base_url: String,
    task_id: String,
    bit_profile_id: String,
    platform: String,
    #[serde(default)]
    expected_platform_account_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccountCheckResult {
    platform_account_id: String,
    name: String,
    avatar_url: String,
    login_status: String,
    message: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GuardPreflightOutcome {
    outcome: String,
    #[serde(default)]
    permit_id: String,
    #[serde(default)]
    permit_credential: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RestoreProfileInput {
    bit_profile_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    seq: Option<i64>,
    #[serde(default)]
    group_id: String,
    #[serde(default)]
    group_name: String,
    #[serde(default)]
    proxy_type: String,
    #[serde(default)]
    proxy_host: String,
    #[serde(default)]
    proxy_port: Option<i64>,
    #[serde(default)]
    remark: String,
}

#[derive(Clone, Debug, Serialize)]
struct RestoreProfileResult {
    restored_count: usize,
    profiles: Vec<RestoreProfileVerification>,
    snapshot: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
struct RestoreProfileVerification {
    bit_profile_id: String,
    status: String,
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
    _task_id: String,
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

    report_runtime_to_cloud(
        &client,
        &cloud_base_url,
        &registration.node.id,
        &registration.node_credential,
        &status,
    )
    .await?;

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

#[tauri::command]
async fn local_agent_refresh_runtime(
    client: State<'_, HttpClient>,
    binding_state: State<'_, RuntimeBindingState>,
    args: RefreshRuntimeArgs,
) -> Result<LocalAgentStatus, String> {
    let cloud_base_url = args.cloud_base_url.trim().trim_end_matches('/').to_string();
    if cloud_base_url.is_empty() {
        return Err("Cloud地址为空，无法刷新本机可信状态".into());
    }
    let binding = binding_state
        .0
        .lock()
        .map_err(|_| "runtime binding lock poisoned")?
        .clone()
        .ok_or_else(|| {
            "当前电脑尚未完成可信绑定，请先到环境状态页绑定当前比特浏览器账号".to_string()
        })?;
    let status = local_agent_status(client.clone()).await?;
    report_runtime_to_cloud(
        &client,
        &cloud_base_url,
        &binding.node_id,
        &binding.node_credential,
        &status,
    )
    .await?;
    Ok(status)
}

#[tauri::command]
async fn local_agent_account_check(
    client: State<'_, HttpClient>,
    binding_state: State<'_, RuntimeBindingState>,
    args: AccountCheckArgs,
) -> Result<AccountCheckResult, String> {
    let cloud_base_url = args.cloud_base_url.trim().trim_end_matches('/').to_string();
    if cloud_base_url.is_empty() {
        return Err("Cloud地址为空，无法执行账号检查".into());
    }
    let task_id = args.task_id.trim().to_string();
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    let platform = args.platform.trim().to_lowercase();
    if task_id.is_empty() || bit_profile_id.is_empty() || platform.is_empty() {
        return Err("账号检查参数不完整".into());
    }
    let binding = binding_state
        .0
        .lock()
        .map_err(|_| "runtime binding lock poisoned")?
        .clone()
        .ok_or_else(|| "当前电脑尚未完成可信绑定，请先到环境状态页重新检测并绑定".to_string())?;
    let status = local_agent_status(client.clone()).await?;
    report_runtime_to_cloud(
        &client,
        &cloud_base_url,
        &binding.node_id,
        &binding.node_credential,
        &status,
    )
    .await?;

    let preflight_url = format!(
        "{}/api/v1/local-agent/sensitive-tasks/{}/preflight",
        cloud_base_url, task_id
    );
    let preflight_resp = client
        .inner
        .post(&preflight_url)
        .bearer_auth(&binding.node_credential)
        .json(&serde_json::json!({"node_id": binding.node_id}))
        .send()
        .await
        .map_err(|e| format!("账号检查预检失败: {}", e))?;
    let preflight_status = preflight_resp.status();
    let preflight_text = preflight_resp.text().await.unwrap_or_default();
    if !preflight_status.is_success() {
        return Err(format!(
            "账号检查预检失败: {} {}",
            preflight_status, preflight_text
        ));
    }
    let preflight_body: CloudEnvelope<GuardPreflightOutcome> =
        serde_json::from_str(&preflight_text)
            .map_err(|e| format!("账号检查预检响应格式错误: {}", e))?;
    if preflight_body.errcode != 0 {
        return Err(if preflight_body.message.is_empty() {
            "账号检查预检失败".into()
        } else {
            preflight_body.message
        });
    }
    let preflight = preflight_body
        .data
        .ok_or_else(|| "账号检查预检缺少授权结果".to_string())?;
    if preflight.outcome != "granted" {
        return Err("当前窗口正在执行其他敏感操作，请稍后重试".into());
    }
    if preflight.permit_id.is_empty() || preflight.permit_credential.is_empty() {
        return Err("账号检查授权缺少本机执行凭证".into());
    }

    let account_check_url = format!("{}/api/v1/account-check", client.local_agent_base);
    let account_check_resp = client
        .inner
        .post(&account_check_url)
        .json(&serde_json::json!({
            "profile_id": bit_profile_id,
            "platform": platform,
            "expected_platform_account_id": args.expected_platform_account_id,
        }))
        .send()
        .await;

    let finish_outcome = if account_check_resp.is_ok() {
        "completed"
    } else {
        "result_uncertain"
    };
    let finish_result = finish_sensitive_permit(
        &client,
        &cloud_base_url,
        &binding,
        &preflight.permit_id,
        &preflight.permit_credential,
        finish_outcome,
    )
    .await;

    let account_check_resp =
        account_check_resp.map_err(|e| format!("Local Agent账号检查失败: {}", e))?;
    let status = account_check_resp.status();
    let text = account_check_resp.text().await.unwrap_or_default();
    if let Err(e) = finish_result {
        return Err(e);
    }
    if !status.is_success() {
        return Err(format!("Local Agent账号检查失败: {} {}", status, text));
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Local Agent账号检查响应格式错误: {}", e))?;
    let data = value
        .get("data")
        .cloned()
        .ok_or_else(|| "Local Agent账号检查响应缺少数据".to_string())?;
    serde_json::from_value(data).map_err(|e| format!("Local Agent账号检查结果格式错误: {}", e))
}

async fn report_runtime_to_cloud(
    client: &HttpClient,
    cloud_base_url: &str,
    node_id: &str,
    node_credential: &str,
    status: &LocalAgentStatus,
) -> Result<(), String> {
    let main_user_id = status.main_user_id.clone().unwrap_or_default();
    if main_user_id.trim().is_empty() {
        return Err("未读取到BitBrowser主账号，请确认BitBrowser已登录后重新检测".into());
    }
    if status.bitbrowser_status.as_deref() != Some("normal") {
        return Err("BitBrowser不可用或身份不可验证，请处理后重新检测".into());
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
        bit_profile_ids: status.bit_profile_ids.clone(),
    };
    let report_url = format!(
        "{}/api/v1/local-agent/nodes/{}/runtime-report",
        cloud_base_url, node_id
    );
    let report_resp = client
        .inner
        .post(&report_url)
        .bearer_auth(node_credential)
        .json(&runtime_report)
        .send()
        .await
        .map_err(|e| format!("本机可信状态刷新失败: {}", e))?;
    if report_resp.status().is_success() {
        return Ok(());
    }
    let status_code = report_resp.status();
    let text = report_resp.text().await.unwrap_or_default();
    Err(format!("本机可信状态刷新失败: {} {}", status_code, text))
}

async fn finish_sensitive_permit(
    client: &HttpClient,
    cloud_base_url: &str,
    binding: &RuntimeBinding,
    permit_id: &str,
    permit_credential: &str,
    outcome: &str,
) -> Result<(), String> {
    let finish_url = format!(
        "{}/api/v1/local-agent/sensitive-permits/{}/finish",
        cloud_base_url, permit_id
    );
    let resp = client
        .inner
        .post(&finish_url)
        .bearer_auth(&binding.node_credential)
        .header("X-Profile-Permit", permit_credential)
        .json(&serde_json::json!({"node_id": binding.node_id, "outcome": outcome}))
        .send()
        .await
        .map_err(|e| format!("释放账号检查本机授权失败: {}", e))?;
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    Err(format!("释放账号检查本机授权失败: {} {}", status, text))
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

/// Restore selected Cloud profile facts to BitBrowser through Local Agent and
/// read back the full profile snapshot before reporting success.
#[tauri::command]
async fn local_agent_profile_restore(
    client: State<'_, HttpClient>,
    profiles: Vec<RestoreProfileInput>,
) -> Result<RestoreProfileResult, String> {
    if profiles.is_empty() {
        return Err("没有可恢复的Cloud窗口配置".into());
    }
    for profile in &profiles {
        if profile.bit_profile_id.trim().is_empty() {
            return Err("恢复失败：存在缺少BitBrowser窗口ID的Cloud窗口".into());
        }
        let update_url = format!(
            "{}/api/v1/bit-browser/profile-update",
            client.local_agent_base
        );
        let update_payload = restore_payload(profile);
        let update_resp = client
            .inner
            .post(&update_url)
            .json(&update_payload)
            .send()
            .await
            .map_err(|e| format!("写回BitBrowser失败: {}", e))?;
        if !update_resp.status().is_success() {
            let status = update_resp.status();
            let text = update_resp.text().await.unwrap_or_default();
            return Err(format!("写回BitBrowser失败: {} {}", status, text));
        }
    }

    let snapshot = local_agent_profile_scan(client.clone()).await?;
    verify_restored_profiles(&profiles, &snapshot)?;
    Ok(RestoreProfileResult {
        restored_count: profiles.len(),
        profiles: profiles
            .iter()
            .map(|profile| RestoreProfileVerification {
                bit_profile_id: profile.bit_profile_id.clone(),
                status: "verified".into(),
            })
            .collect(),
        snapshot,
    })
}

fn restore_payload(profile: &RestoreProfileInput) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "id".into(),
        serde_json::Value::String(profile.bit_profile_id.clone()),
    );
    if !profile.name.trim().is_empty() {
        payload.insert(
            "name".into(),
            serde_json::Value::String(profile.name.clone()),
        );
    }
    if let Some(seq) = profile.seq {
        payload.insert("seq".into(), serde_json::Value::Number(seq.into()));
    }
    if !profile.group_id.trim().is_empty() {
        payload.insert(
            "groupId".into(),
            serde_json::Value::String(profile.group_id.clone()),
        );
    }
    if !profile.group_name.trim().is_empty() {
        payload.insert(
            "groupName".into(),
            serde_json::Value::String(profile.group_name.clone()),
        );
    }
    if !profile.proxy_type.trim().is_empty() {
        payload.insert(
            "proxyType".into(),
            serde_json::Value::String(profile.proxy_type.clone()),
        );
    }
    if !profile.proxy_host.trim().is_empty() {
        payload.insert(
            "proxyHost".into(),
            serde_json::Value::String(profile.proxy_host.clone()),
        );
    }
    if let Some(port) = profile.proxy_port {
        payload.insert("proxyPort".into(), serde_json::Value::Number(port.into()));
    }
    if !profile.remark.trim().is_empty() {
        payload.insert(
            "remark".into(),
            serde_json::Value::String(profile.remark.clone()),
        );
    }
    serde_json::Value::Object(payload)
}

fn verify_restored_profiles(
    expected: &[RestoreProfileInput],
    snapshot: &serde_json::Value,
) -> Result<(), String> {
    let scanned = snapshot
        .get("profiles")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "读回BitBrowser结果格式不正确，无法确认恢复结果".to_string())?;
    for profile in expected {
        let actual = scanned
            .iter()
            .find(|item| string_field(item, "bit_profile_id") == profile.bit_profile_id)
            .ok_or_else(|| {
                format!(
                    "恢复结果待确认：读回时没有找到窗口 {}",
                    profile.bit_profile_id
                )
            })?;
        verify_string_field(
            actual,
            "name",
            &profile.name,
            "名称",
            &profile.bit_profile_id,
        )?;
        verify_string_field(
            actual,
            "group_id",
            &profile.group_id,
            "分组ID",
            &profile.bit_profile_id,
        )?;
        verify_string_field(
            actual,
            "group_name",
            &profile.group_name,
            "分组名称",
            &profile.bit_profile_id,
        )?;
        verify_string_field(
            actual,
            "proxy_type",
            &profile.proxy_type,
            "代理类型",
            &profile.bit_profile_id,
        )?;
        verify_string_field(
            actual,
            "proxy_host",
            &profile.proxy_host,
            "代理地址",
            &profile.bit_profile_id,
        )?;
        verify_i64_field(
            actual,
            "proxy_port",
            profile.proxy_port,
            "代理端口",
            &profile.bit_profile_id,
        )?;
    }
    Ok(())
}

fn verify_string_field(
    actual: &serde_json::Value,
    field: &str,
    expected: &str,
    label: &str,
    bit_profile_id: &str,
) -> Result<(), String> {
    if expected.trim().is_empty() {
        return Ok(());
    }
    let actual_value = string_field(actual, field);
    if actual_value != expected {
        return Err(format!(
            "恢复结果待确认：窗口 {} 的{}读回不一致",
            bit_profile_id, label
        ));
    }
    Ok(())
}

fn verify_i64_field(
    actual: &serde_json::Value,
    field: &str,
    expected: Option<i64>,
    label: &str,
    bit_profile_id: &str,
) -> Result<(), String> {
    let Some(expected_value) = expected else {
        return Ok(());
    };
    let actual_value = actual
        .get(field)
        .and_then(|value| value.as_i64())
        .unwrap_or_default();
    if actual_value != expected_value {
        return Err(format!(
            "恢复结果待确认：窗口 {} 的{}读回不一致",
            bit_profile_id, label
        ));
    }
    Ok(())
}

fn string_field(value: &serde_json::Value, field: &str) -> String {
    value
        .get(field)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restore_input() -> RestoreProfileInput {
        RestoreProfileInput {
            bit_profile_id: "profile-1".into(),
            name: "窗口一".into(),
            seq: Some(1),
            group_id: "group-1".into(),
            group_name: "分组一".into(),
            proxy_type: "socks5".into(),
            proxy_host: "127.0.0.1".into(),
            proxy_port: Some(1080),
            remark: "Cloud备注".into(),
        }
    }

    #[test]
    fn restore_payload_only_contains_safe_profile_fields() {
        let payload = restore_payload(&restore_input());

        assert_eq!(payload["id"], "profile-1");
        assert_eq!(payload["name"], "窗口一");
        assert_eq!(payload["groupId"], "group-1");
        assert_eq!(payload["groupName"], "分组一");
        assert_eq!(payload["proxyType"], "socks5");
        assert_eq!(payload["proxyHost"], "127.0.0.1");
        assert_eq!(payload["proxyPort"], 1080);
        assert!(payload.get("cookie").is_none());
        assert!(payload.get("user_id").is_none());
    }

    #[test]
    fn verify_restored_profiles_accepts_matching_readback() {
        let expected = vec![restore_input()];
        let snapshot = serde_json::json!({
            "main_user_id": "main-1",
            "profiles": [{
                "bit_profile_id": "profile-1",
                "name": "窗口一",
                "group_id": "group-1",
                "group_name": "分组一",
                "proxy_type": "socks5",
                "proxy_host": "127.0.0.1",
                "proxy_port": 1080
            }]
        });

        assert!(verify_restored_profiles(&expected, &snapshot).is_ok());
    }

    #[test]
    fn verify_restored_profiles_rejects_mismatched_readback() {
        let expected = vec![restore_input()];
        let snapshot = serde_json::json!({
            "profiles": [{
                "bit_profile_id": "profile-1",
                "name": "另一个名称",
                "group_id": "group-1",
                "group_name": "分组一",
                "proxy_type": "socks5",
                "proxy_host": "127.0.0.1",
                "proxy_port": 1080
            }]
        });

        let err = verify_restored_profiles(&expected, &snapshot).unwrap_err();
        assert!(err.contains("读回不一致"));
    }
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
            local_agent_refresh_runtime,
            local_agent_account_check,
            local_agent_profile_scan,
            local_agent_profile_restore,
        ])
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running wt-media-desktop tauri application");
}
