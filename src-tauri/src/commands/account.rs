//! Sensitive-task flows: account check and cookie read. Both take a Cloud
//! permit before touching the Local Agent, and both release it afterwards.

use crate::dto::{
    AccountCheckArgs, AccountCheckResult, CloudEnvelope, CookieReadArgs, CookieReadData,
    CookieReadResult, GuardPreflightOutcome,
};
use crate::http::{CloudClient, LocalAgentClient};
use crate::state::{RuntimeBinding, RuntimeBindingState};
use tauri::State;

use super::agent::local_agent_status;
use super::bind::report_runtime_to_cloud;

#[tauri::command]
pub async fn local_agent_account_check(
    client: State<'_, LocalAgentClient>,
    cloud: State<'_, CloudClient>,
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
        &cloud,
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
    let preflight_resp = cloud
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

    let account_check_url = format!("{}/api/v1/account-check", client.base);
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
    let cloud_base_url = args.cloud_base_url.trim().trim_end_matches('/').to_string();
    if cloud_base_url.is_empty() {
        return Err("Cloud地址为空，无法读取Cookie".into());
    }
    let task_id = args.task_id.trim().to_string();
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    if task_id.is_empty() || bit_profile_id.is_empty() {
        return Err("Cookie读取参数不完整".into());
    }
    let binding = binding_state
        .0
        .lock()
        .map_err(|_| "runtime binding lock poisoned")?
        .clone()
        .ok_or_else(|| "当前电脑尚未完成可信绑定，请先到环境状态页重新检测并绑定".to_string())?;
    let status = local_agent_status(client.clone()).await?;
    report_runtime_to_cloud(
        &cloud,
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
    let preflight_resp = cloud
        .inner
        .post(&preflight_url)
        .bearer_auth(&binding.node_credential)
        .json(&serde_json::json!({"node_id": binding.node_id}))
        .send()
        .await
        .map_err(|e| format!("Cookie读取预检失败: {}", e))?;
    let preflight_status = preflight_resp.status();
    let preflight_text = preflight_resp.text().await.unwrap_or_default();
    if !preflight_status.is_success() {
        return Err(format!(
            "Cookie读取预检失败: {} {}",
            preflight_status, preflight_text
        ));
    }
    let preflight_body: CloudEnvelope<GuardPreflightOutcome> =
        serde_json::from_str(&preflight_text)
            .map_err(|e| format!("Cookie读取预检响应格式错误: {}", e))?;
    if preflight_body.errcode != 0 {
        return Err(if preflight_body.message.is_empty() {
            "Cookie读取预检失败".into()
        } else {
            preflight_body.message
        });
    }
    let preflight = preflight_body
        .data
        .ok_or_else(|| "Cookie读取预检缺少授权结果".to_string())?;
    if preflight.outcome != "granted" {
        return Err("当前窗口正在执行其他敏感操作，请稍后重试".into());
    }
    if preflight.permit_id.is_empty() || preflight.permit_credential.is_empty() {
        return Err("Cookie读取授权缺少本机执行凭证".into());
    }

    let cookie_read_url = format!("{}/api/v1/cookie-read", client.base);
    let cookie_read_resp = client
        .inner
        .post(&cookie_read_url)
        .json(&serde_json::json!({"profile_id": bit_profile_id}))
        .send()
        .await;

    let finish_outcome = if cookie_read_resp.is_ok() {
        "completed"
    } else {
        "result_uncertain"
    };
    let finish_result = finish_sensitive_permit(
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
pub async fn finish_sensitive_permit(
    client: &CloudClient,
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
