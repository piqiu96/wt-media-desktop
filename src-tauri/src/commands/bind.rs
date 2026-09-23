//! The bind flow: a one-use Cloud ticket becomes a Cloud node registration,
//! then a Local Agent node write, then a runtime-fact report.

use crate::dto::{
    BindResponse, BindSessionArgs, CloudEnvelope, LocalAgentStatus, RefreshRuntimeArgs,
    RegisterLocalNodeRequest, RegisterLocalNodeResponse, RuntimeDiskFact, RuntimeReportRequest,
    RuntimeStatusFact,
};
use crate::http::{CloudClient, LocalAgentClient};
use crate::local_agent::BoundNodeFacts;
use crate::state::{RuntimeBinding, RuntimeBindingState};
use tauri::State;

use super::agent::local_agent_status;

/// Bind the Desktop to the Local Agent, receiving a session token.
#[tauri::command]
pub async fn local_agent_bind(client: State<'_, LocalAgentClient>) -> Result<BindResponse, String> {
    let url = format!("{}/api/v1/bind", client.base);
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
pub async fn local_agent_bind_session(
    client: State<'_, LocalAgentClient>,
    cloud: State<'_, CloudClient>,
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
    let register_resp = cloud
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

    let bind_url = format!("{}/api/v1/bind", client.base);
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
        &cloud,
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
pub async fn local_agent_refresh_runtime(
    client: State<'_, LocalAgentClient>,
    cloud: State<'_, CloudClient>,
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
        &cloud,
        &cloud_base_url,
        &binding.node_id,
        &binding.node_credential,
        &status,
    )
    .await?;
    Ok(status)
}
pub async fn report_runtime_to_cloud(
    client: &CloudClient,
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
