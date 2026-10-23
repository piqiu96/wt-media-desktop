//! BitBrowser 主账号绑定：主账号由 Local Agent 实读，页面只决定是否确认换绑。
//!
//! 首次绑定、同一账号的再次确认和正式换绑都是这一个入口。主账号全文留在
//! Agent 与 Cloud 之间，页面读 Cloud 的掩码视图。

use crate::dto::{BitAccountBindAck, BitAccountBindResult, GuardRefusal};
use crate::http::LocalAgentClient;
use tauri::State;

/// 让 Agent 实读主账号并请 Cloud 绑定。
///
/// `rebind` 只在用户明确确认「换绑」时为真；临时不一致会以
/// `BIT_MAIN_USER_MISMATCH` 被拒，不会自动迁移任何 Profile。
pub(crate) async fn bind(
    client: &LocalAgentClient,
    rebind: bool,
) -> Result<BitAccountBindResult, GuardRefusal> {
    let response = client
        .post("/api/v1/bit-account/bind")
        .json(&serde_json::json!({"rebind": rebind}))
        .send()
        .await
        .map_err(GuardRefusal::unreachable)?;
    let status = response.status();
    let text = response.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    let answer: BitAccountBindAck =
        serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))?;
    if answer.data.is_none() {
        return Err(GuardRefusal::missing_result());
    }
    Ok(BitAccountBindResult { bound: true })
}

#[tauri::command]
pub async fn local_agent_bit_account_bind(
    client: State<'_, LocalAgentClient>,
    rebind: Option<bool>,
) -> Result<BitAccountBindResult, GuardRefusal> {
    bind(&client, rebind.unwrap_or(false)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{arrivals, requests, serving};
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;

    fn config_to(port: u16) -> crate::config::DesktopConfig {
        let text = PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}"));
        load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config")
    }

    fn client_to(port: u16) -> LocalAgentClient {
        LocalAgentClient::new(&config_to(port), RuntimeToken::generate())
    }

    /// 成功时页面只得到「已绑定」，请求里没有主账号。
    #[test]
    fn a_granted_bind_answers_only_that_it_is_bound() {
        let (port, seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"data":{"user_id":"user-1","main_user_id":"main-secret-full","status":"bound","bound_at":"2026-10-23T10:00:00Z","last_verified_at":"2026-10-23T10:00:00Z"}}"#
                    .into(),
            )
        });

        let result = tauri::async_runtime::block_on(bind(&client_to(port), false))
            .expect("the Agent bound it");

        assert_eq!(result, BitAccountBindResult { bound: true });
        let request = requests(&seen).remove(0);
        assert!(request.starts_with("POST /api/v1/bit-account/bind "));
        assert!(
            request.contains(r#""rebind":false"#),
            "the caller only says whether it confirms: {request}"
        );
        assert!(
            !request.contains("main-secret-full"),
            "the main account is read by the Agent, never sent by Desktop: {request}"
        );
    }

    /// 换绑意图原样到达 Agent。
    #[test]
    fn a_rebind_is_carried_as_the_agents_own_flag() {
        let (port, seen) =
            serving(|_| ("200 OK".into(), r#"{"data":{"main_user_id":"m"}}"#.into()));

        tauri::async_runtime::block_on(bind(&client_to(port), true)).expect("bound");

        assert_eq!(arrivals(&seen, "/api/v1/bit-account/bind"), 1);
        assert!(requests(&seen)[0].contains(r#""rebind":true"#));
    }

    /// 主账号不一致是一个可按码分支的拒绝，下一步是换绑而不是重新登录。
    #[test]
    fn a_mismatch_keeps_its_code_and_next_step() {
        let (port, _seen) = serving(|_| {
            (
                "409 Conflict".into(),
                r#"{"error":{"code":"BIT_MAIN_USER_MISMATCH","message":"当前比特浏览器登录账号与系统绑定账号不一致","retryable":false,"recovery_action":"rebind_bitbrowser"}}"#
                    .into(),
            )
        });

        let refusal = tauri::async_runtime::block_on(bind(&client_to(port), false))
            .expect_err("a mismatch is not a binding");

        assert_eq!(refusal.code, "BIT_MAIN_USER_MISMATCH");
        assert_eq!(refusal.recovery_action, "rebind_bitbrowser");
        assert!(!refusal.retryable);
    }

    /// BitBrowser 没开：可重试，下一步是启动它。
    #[test]
    fn an_offline_bitbrowser_is_retryable_and_names_the_next_step() {
        let (port, _seen) = serving(|_| {
            (
                "503 Service Unavailable".into(),
                r#"{"error":{"code":"bit_browser_offline","message":"BitBrowser 未运行或不可达","retryable":true,"recovery_action":"start_bitbrowser"}}"#
                    .into(),
            )
        });

        let refusal = tauri::async_runtime::block_on(bind(&client_to(port), false))
            .expect_err("offline is not a binding");

        assert_eq!(refusal.code, "bit_browser_offline");
        assert!(refusal.retryable);
        assert_eq!(refusal.recovery_action, "start_bitbrowser");
    }

    /// 200 但没有结果不算绑定成功。
    #[test]
    fn a_bind_answer_without_a_result_is_a_failure() {
        let (port, _seen) = serving(|_| ("200 OK".into(), "{}".into()));

        let refusal = tauri::async_runtime::block_on(bind(&client_to(port), false))
            .expect_err("nothing was bound");

        assert_eq!(refusal.code, "cloud_request_failed");
    }

    /// 命令注册在案。
    #[test]
    fn every_command_in_this_module_is_registered() {
        fn declared(source: &str) -> Vec<String> {
            let lines: Vec<&str> = source.lines().collect();
            let mut names = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if line.trim() != "#[tauri::command]" {
                    continue;
                }
                let signature = lines[index..]
                    .iter()
                    .find(|line| line.contains("fn "))
                    .expect("a signature after the attribute");
                names.push(
                    signature
                        .split("fn ")
                        .nth(1)
                        .expect("a name")
                        .split('(')
                        .next()
                        .expect("an open paren")
                        .trim()
                        .to_string(),
                );
            }
            names
        }

        fn registered(main: &str) -> Vec<String> {
            let start = main.find("generate_handler![").expect("the handler list");
            let rest = &main[start..];
            let end = rest.find("])").expect("the end of the handler list");
            rest[..end]
                .lines()
                .filter_map(|line| {
                    line.trim()
                        .trim_end_matches(',')
                        .strip_prefix("commands::bit_account::")
                        .map(str::to_string)
                })
                .collect()
        }

        let mut declared = declared(include_str!("bit_account.rs"));
        let mut registered = registered(include_str!("../main.rs"));
        declared.sort();
        registered.sort();

        assert_eq!(declared, registered);
        assert_eq!(declared.len(), 1, "the denominator: {declared:?}");
    }
}
