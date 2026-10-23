//! 敏感任务的本机预检握手，以及用完后交还许可。
//!
//! 检查本身（BitBrowser 实读、主账号一致、Profile 作用域、平台登录态、本地文件）
//! 由 Local Agent 与 Cloud 完成；Desktop 只送任务 id 与 Profile id，不再自己读
//! 主账号，也不再向 Cloud 同步运行时事实。
//!
//! 两项流程的句子只差一个名字，所以名字走 [`PreflightSpec`]，句子留在本模块：
//! 释放失败要说明是哪一个操作，用户才知道刚刚白等的是什么。

use crate::dto::{GuardPreflightAck, GuardPreflightOutcome, LocalErrorBody};
use crate::http::LocalAgentClient;
use reqwest::StatusCode;

/// 哪一个敏感操作在申请许可。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreflightSpec {
    noun: &'static str,
}

pub const ACCOUNT_CHECK: PreflightSpec = PreflightSpec {
    noun: "账号检查"
};
pub const COOKIE_READ: PreflightSpec = PreflightSpec {
    noun: "Cookie读取"
};

/// 另一个敏感操作占着窗口。许可不是排队凭证，等下去没有意义。
pub const WINDOW_BUSY: &str = "当前窗口正在执行其他敏感操作，请稍后重试";

/// Agent 的拒绝句。
///
/// 能读到 `message` 就用它——那句来自 Cloud，用户看得懂；否则按码说明。
/// 预检请求里只有任务与 Profile id，Agent 回显请求内容也不会带出凭据。
fn refusal_message(spec: PreflightSpec, status: StatusCode, text: &str) -> String {
    match LocalErrorBody::detail_of(text) {
        Some(detail) if !detail.message.trim().is_empty() => detail.message,
        Some(detail) if !detail.code.is_empty() => {
            format!("{}预检失败（{}）", spec.noun, detail.code)
        }
        _ => format!("{}预检失败: {} {}", spec.noun, status, text),
    }
}

/// 向 Local Agent 申请一次许可；只有 `granted` 会返回带 `permit_id` 的结果。
///
/// `waiting` 时 Cloud 会把**别人**正在用的许可号一起带回来，所以这里不能把它
/// 当作自己的许可返回，更不能在收尾时释放它。
pub async fn run(
    client: &LocalAgentClient,
    task_id: &str,
    profile_id: &str,
    spec: PreflightSpec,
) -> Result<GuardPreflightOutcome, String> {
    let response = client
        .post(&format!("/api/v1/sensitive-tasks/{}/preflight", task_id))
        .json(&serde_json::json!({"profile_id": profile_id}))
        .send()
        .await
        .map_err(|e| format!("{}预检失败: {}", spec.noun, e))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(refusal_message(spec, status, &text));
    }
    let permit = serde_json::from_str::<GuardPreflightAck>(&text)
        .map_err(|e| format!("{}预检响应格式错误: {}", spec.noun, e))?
        .data;
    if permit.outcome != "granted" {
        return Err(WINDOW_BUSY.to_string());
    }
    if permit.permit_id.is_empty() {
        return Err(format!("{}预检缺少许可", spec.noun));
    }
    Ok(permit)
}

fn release_failure(spec: PreflightSpec, detail: impl std::fmt::Display) -> String {
    format!("释放{}本机授权失败: {}", spec.noun, detail)
}

/// 交还许可。本地操作失败时同样要交还，并标记 `result_uncertain`。
pub async fn finish_permit(
    client: &LocalAgentClient,
    spec: PreflightSpec,
    permit_id: &str,
    outcome: &str,
) -> Result<(), String> {
    let response = client
        .post(&format!("/api/v1/sensitive-permits/{}/finish", permit_id))
        .json(&serde_json::json!({"outcome": outcome}))
        .send()
        .await
        .map_err(|e| release_failure(spec, e))?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    Err(release_failure(spec, format!("{} {}", status, text)))
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

    /// 拿到许可，且请求里只有 Profile。
    #[test]
    fn a_granted_permit_is_returned_and_only_the_profile_travels() {
        let (port, seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"data":{"outcome":"granted","permit_id":"permit-1","profile_id":"profile-1","expires_at":"2026-10-23T10:01:00Z"}}"#
                    .into(),
            )
        });

        let permit = tauri::async_runtime::block_on(run(
            &client_to(port),
            "task-1",
            "profile-1",
            ACCOUNT_CHECK,
        ))
        .expect("granted");

        assert_eq!(permit.permit_id, "permit-1");
        assert_eq!(permit.profile_id, "profile-1");
        let request = requests(&seen).remove(0);
        assert!(
            request.starts_with("POST /api/v1/sensitive-tasks/task-1/preflight "),
            "{request}"
        );
        assert!(request.contains(r#""profile_id":"profile-1""#), "{request}");
    }

    /// 窗口被占：一句共享的句子，且不把别人的许可号当自己的。
    #[test]
    fn a_waiting_window_is_refused_with_the_shared_sentence() {
        let (port, _seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"data":{"outcome":"waiting","permit_id":"someone-elses","profile_id":"profile-1"}}"#
                    .into(),
            )
        });

        let error = tauri::async_runtime::block_on(run(
            &client_to(port),
            "task-1",
            "profile-1",
            COOKIE_READ,
        ))
        .expect_err("waiting is not granted");

        assert_eq!(error, WINDOW_BUSY);
    }

    /// `granted` 没有许可号也不能继续：收尾时没有东西可交还。
    #[test]
    fn a_granted_permit_without_an_id_is_refused() {
        let (port, _seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"data":{"outcome":"granted","profile_id":"profile-1"}}"#.into(),
            )
        });

        let error = tauri::async_runtime::block_on(run(
            &client_to(port),
            "task-1",
            "profile-1",
            ACCOUNT_CHECK,
        ))
        .expect_err("no permit");

        assert_eq!(error, "账号检查预检缺少许可");
    }

    /// Agent 带回的句子优先于它的码：那句话是给用户看的。
    #[test]
    fn an_agents_message_is_preferred_over_its_code() {
        let (port, _seen) = serving(|_| {
            (
                "409 Conflict".into(),
                r#"{"error":{"code":"PROFILE_NOT_FOUND","message":"未找到对应的浏览器环境","retryable":false,"recovery_action":"choose_profile"}}"#
                    .into(),
            )
        });

        let error = tauri::async_runtime::block_on(run(
            &client_to(port),
            "task-1",
            "profile-1",
            ACCOUNT_CHECK,
        ))
        .expect_err("refused");

        assert_eq!(error, "未找到对应的浏览器环境");
    }

    /// 没有句子时按码命名，页面仍能据此分支。
    #[test]
    fn a_refusal_without_a_message_still_names_its_code() {
        let (port, _seen) = serving(|_| {
            (
                "503 Service Unavailable".into(),
                r#"{"error":{"code":"agent_token_required"}}"#.into(),
            )
        });

        let error = tauri::async_runtime::block_on(run(
            &client_to(port),
            "task-1",
            "profile-1",
            COOKIE_READ,
        ))
        .expect_err("refused");

        assert_eq!(error, "Cookie读取预检失败（agent_token_required）");
    }

    /// 收尾把结果交给许可路由，204 是成功。
    #[test]
    fn finishing_sends_the_outcome_to_the_permit_route() {
        let (port, seen) = serving(|_| ("204 No Content".into(), String::new()));

        tauri::async_runtime::block_on(finish_permit(
            &client_to(port),
            ACCOUNT_CHECK,
            "permit-1",
            "result_uncertain",
        ))
        .expect("204 is done");

        assert_eq!(
            arrivals(&seen, "/api/v1/sensitive-permits/permit-1/finish"),
            1
        );
        assert!(requests(&seen)[0].contains(r#""outcome":"result_uncertain""#));
    }

    /// 交还失败要说是哪一个操作没释放。
    #[test]
    fn a_failed_finish_names_the_flow_it_could_not_release() {
        let (port, _seen) = serving(|_| ("409 Conflict".into(), "busy".into()));

        let error = tauri::async_runtime::block_on(finish_permit(
            &client_to(port),
            COOKIE_READ,
            "permit-1",
            "completed",
        ))
        .expect_err("a 409 is not released");

        assert!(error.starts_with("释放Cookie读取本机授权失败: "), "{error}");
        assert!(error.contains("409"), "{error}");
    }
}
