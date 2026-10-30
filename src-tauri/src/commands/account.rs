//! Account-check and scoped Cookie operation flows. Cloud authorizes the operation and
//! returns its operation id; Desktop only drives the one-shot Local Agent call.

use crate::dto::{
    AccountCheckArgs, AccountCheckResult, AccountCookieArgs, AccountCookieResult,
    AccountCookieStage, GuardRefusal,
};
use crate::http::LocalAgentClient;
use tauri::State;

#[tauri::command]
pub async fn local_agent_account_check(
    client: State<'_, LocalAgentClient>,
    args: AccountCheckArgs,
) -> Result<AccountCheckResult, GuardRefusal> {
    let operation_id = args.operation_id.trim();
    let bit_profile_id = args.bit_profile_id.trim();
    let platform = args.platform.trim().to_lowercase();
    if operation_id.is_empty() || bit_profile_id.is_empty() || platform.is_empty() {
        return Err(GuardRefusal {
            code: "account_check_input_invalid".into(),
            message: "账号检查参数不完整".into(),
            ..Default::default()
        });
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
        .map_err(GuardRefusal::unreachable)?;
    let status = response.status();
    let text = response.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))?;
    let data = value
        .get("data")
        .cloned()
        .ok_or_else(|| GuardRefusal::unreadable(status))?;
    serde_json::from_value(data).map_err(|_| GuardRefusal::unreadable(status))
}

#[tauri::command]
pub async fn local_agent_account_cookie(
    client: State<'_, LocalAgentClient>,
    args: AccountCookieArgs,
) -> Result<AccountCookieResult, GuardRefusal> {
    if args.operation_id.trim().is_empty()
        || args.bit_profile_id.trim().is_empty()
        || !["bilibili", "baidu", "douyin"].contains(&args.platform.as_str())
    {
        return Err(GuardRefusal {
            code: "account_cookie_input_invalid".into(),
            message: "Cookie 操作参数不完整".into(),
            ..Default::default()
        });
    }
    let path = match args.stage {
        AccountCookieStage::Prepare => "/api/v1/account-cookie/prepare",
        AccountCookieStage::Apply => "/api/v1/account-cookie/apply",
        AccountCookieStage::Inspect => "/api/v1/account-cookie/inspect",
    };
    let response=client.post(path).json(&serde_json::json!({
  "operation_id":args.operation_id,"profile_id":args.bit_profile_id,"platform":args.platform,
  "expected_platform_account_id":args.expected_platform_account_id,"input_cookie":args.input_cookie,"fingerprint":args.fingerprint,
 })).send().await.map_err(GuardRefusal::unreachable)?;
    let status = response.status();
    let text = response.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))?;
    serde_json::from_value(
        value
            .get("data")
            .cloned()
            .ok_or_else(|| GuardRefusal::unreadable(status))?,
    )
    .map_err(|_| GuardRefusal::unreadable(status))
}
