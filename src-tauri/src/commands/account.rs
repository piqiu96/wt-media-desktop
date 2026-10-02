//! Sensitive-task flows: account check and cookie read. Both take a Cloud
//! permit before touching the Local Agent, and both release it afterwards.

use crate::dto::{
    AccountCheckArgs, AccountCheckResult, CookieReadArgs, CookieReadData, CookieReadResult,
};
use crate::http::{CloudClient, LocalAgentClient};
use crate::preflight::{self, ACCOUNT_CHECK, COOKIE_READ, NO_BINDING_SENSITIVE};
use crate::state::RuntimeBindingState;
use tauri::State;

use super::agent::local_agent_status;

#[tauri::command]
pub async fn local_agent_account_check(
    client: State<'_, LocalAgentClient>,
    cloud: State<'_, CloudClient>,
    binding_state: State<'_, RuntimeBindingState>,
    args: AccountCheckArgs,
) -> Result<AccountCheckResult, String> {
    let cloud_base_url =
        preflight::require_cloud_base_url(&args.cloud_base_url, ACCOUNT_CHECK.no_cloud_address())?;
    let task_id = args.task_id.trim().to_string();
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    let platform = args.platform.trim().to_lowercase();
    if task_id.is_empty() || bit_profile_id.is_empty() || platform.is_empty() {
        return Err("账号检查参数不完整".into());
    }
    let binding = preflight::require_binding(&binding_state, NO_BINDING_SENSITIVE)?;
    let status = local_agent_status(client.clone(), None).await?;
    preflight::sync_runtime_facts(
        &cloud,
        &cloud_base_url,
        &binding.node_id,
        &binding.node_credential,
        &status,
    )
    .await?;

    let preflight =
        preflight::run(&cloud, &cloud_base_url, &task_id, &binding, ACCOUNT_CHECK).await?;

    let account_check_resp = client
        .post("/api/v1/account-check")
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
    let finish_result = preflight::finish_permit(
        &cloud,
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
#[tauri::command]
pub async fn local_agent_cookie_read(
    client: State<'_, LocalAgentClient>,
    cloud: State<'_, CloudClient>,
    binding_state: State<'_, RuntimeBindingState>,
    args: CookieReadArgs,
) -> Result<CookieReadResult, String> {
    let cloud_base_url =
        preflight::require_cloud_base_url(&args.cloud_base_url, COOKIE_READ.no_cloud_address())?;
    let task_id = args.task_id.trim().to_string();
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    if task_id.is_empty() || bit_profile_id.is_empty() {
        return Err("Cookie读取参数不完整".into());
    }
    let binding = preflight::require_binding(&binding_state, NO_BINDING_SENSITIVE)?;
    let status = local_agent_status(client.clone(), None).await?;
    preflight::sync_runtime_facts(
        &cloud,
        &cloud_base_url,
        &binding.node_id,
        &binding.node_credential,
        &status,
    )
    .await?;

    let preflight =
        preflight::run(&cloud, &cloud_base_url, &task_id, &binding, COOKIE_READ).await?;

    let cookie_read_resp = client
        .post("/api/v1/cookie-read")
        .json(&serde_json::json!({"profile_id": bit_profile_id}))
        .send()
        .await;

    let finish_outcome = if cookie_read_resp.is_ok() {
        "completed"
    } else {
        "result_uncertain"
    };
    let finish_result = preflight::finish_permit(
        &cloud,
        &cloud_base_url,
        &binding,
        &preflight.permit_id,
        &preflight.permit_credential,
        finish_outcome,
    )
    .await;

    let cookie_read_resp =
        cookie_read_resp.map_err(|e| format!("Local Agent Cookie读取失败: {}", e))?;
    let status = cookie_read_resp.status();
    let text = cookie_read_resp.text().await.unwrap_or_default();
    if let Err(e) = finish_result {
        return Err(e);
    }
    if !status.is_success() {
        return Err(format!("Local Agent Cookie读取失败: {} {}", status, text));
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Local Agent Cookie读取响应格式错误: {}", e))?;
    let data = value
        .get("data")
        .cloned()
        .ok_or_else(|| "Local Agent Cookie读取响应缺少数据".to_string())?;
    let payload: CookieReadData = serde_json::from_value(data)
        .map_err(|e| format!("Local Agent Cookie读取结果格式错误: {}", e))?;
    Ok(CookieReadResult {
        cookie_count: payload.cookies.len(),
        cookies: payload.cookies,
    })
}
