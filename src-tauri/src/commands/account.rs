//! Account-check and cookie-read flows. Cloud authorizes the operation and
//! returns its operation id; Desktop only drives the one-shot Local Agent call.

use crate::dto::{
    AccountCheckArgs, AccountCheckResult, CookieReadArgs, CookieReadData, CookieReadResult,
};
use crate::http::LocalAgentClient;
use tauri::State;

#[tauri::command]
pub async fn local_agent_account_check(
    client: State<'_, LocalAgentClient>,
    args: AccountCheckArgs,
) -> Result<AccountCheckResult, String> {
    let operation_id = args.operation_id.trim();
    let bit_profile_id = args.bit_profile_id.trim();
    let platform = args.platform.trim().to_lowercase();
    if operation_id.is_empty() || bit_profile_id.is_empty() || platform.is_empty() {
        return Err("账号检查参数不完整".into());
    }

    let response = client
        .post("/api/v1/account-check")
        .json(&serde_json::json!({
            "profile_id": bit_profile_id,
            "platform": platform,
            "expected_platform_account_id": args.expected_platform_account_id,
        }))
        .send()
        .await
        .map_err(|e| format!("Local Agent账号检查失败: {}", e))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
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
    args: CookieReadArgs,
) -> Result<CookieReadResult, String> {
    let operation_id = args.operation_id.trim();
    let bit_profile_id = args.bit_profile_id.trim();
    if operation_id.is_empty() || bit_profile_id.is_empty() {
        return Err("Cookie读取参数不完整".into());
    }

    let response = client
        .post("/api/v1/cookie-read")
        .json(&serde_json::json!({"profile_id": bit_profile_id}))
        .send()
        .await
        .map_err(|e| format!("Local Agent Cookie读取失败: {}", e))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
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
