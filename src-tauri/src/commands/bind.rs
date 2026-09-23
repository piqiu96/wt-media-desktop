//! The bind flow: a one-use Cloud ticket becomes a Cloud node registration,
//! then a Local Agent node write, then a runtime-fact report.

use crate::dto::{
    BindResponse, BindSessionArgs, CloudEnvelope, LocalAgentStatus, RefreshRuntimeArgs,
    RegisterLocalNodeRequest, RegisterLocalNodeResponse,
};
use crate::http::{CloudClient, LocalAgentClient};
use crate::local_agent::BoundNodeFacts;
use crate::preflight::{
    self, NO_BINDING_REFRESH, NO_CLOUD_ADDRESS_BIND, NO_CLOUD_ADDRESS_REFRESH,
};
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
    let cloud_base_url =
        preflight::require_cloud_base_url(&args.cloud_base_url, NO_CLOUD_ADDRESS_BIND)?;

    let status = local_agent_status(client.clone()).await?;
    preflight::require_verified_bitbrowser(&status)?;

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

    preflight::sync_runtime_facts(
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
    let cloud_base_url =
        preflight::require_cloud_base_url(&args.cloud_base_url, NO_CLOUD_ADDRESS_REFRESH)?;
    let binding = preflight::require_binding(&binding_state, NO_BINDING_REFRESH)?;
    let status = local_agent_status(client.clone()).await?;
    preflight::sync_runtime_facts(
        &cloud,
        &cloud_base_url,
        &binding.node_id,
        &binding.node_credential,
        &status,
    )
    .await?;
    Ok(status)
}
