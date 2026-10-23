//! Agent Token：用当前 Desktop 会话向 Cloud 换一张限权凭据，交给本机 Agent 持有。
//!
//! Token 不落盘、不进页面。换取走 Cloud，写入走本机 Agent；两条请求各自只带
//! 该带的那一个凭据。

use crate::dto::{
    AgentTokenAck, AgentTokenFacts, CloudEnvelope, ConfigureTokenArgs, IssuedAgentToken,
    LocalErrorBody,
};
use crate::http::{CloudClient, LocalAgentClient};
use crate::state::{AgentToken, AgentTokenState};
use tauri::State;

/// Cloud 地址为空时的句子。换取 Token 的唯一用途就是找 Cloud，与其他流程不同。
pub const NO_CLOUD_ADDRESS_TOKEN: &str = "Cloud地址为空，无法获取本机执行凭证";

/// 还没有会话就没有东西可换。
pub const NO_SESSION_TOKEN: &str = "还没有登录运营平台，无法获取本机执行凭证";

fn cloud_base_url(raw: &str) -> Result<String, String> {
    let url = raw.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return Err(NO_CLOUD_ADDRESS_TOKEN.to_string());
    }
    Ok(url)
}

/// 页面能看到的全部：有没有、何时过期。
pub fn facts(tokens: &AgentTokenState) -> AgentTokenFacts {
    match tokens.get() {
        Some(held) => AgentTokenFacts {
            configured: true,
            expires_at: held.expires_at,
        },
        None => AgentTokenFacts {
            configured: false,
            expires_at: None,
        },
    }
}

/// Cloud 的拒绝句：优先 Cloud 自己写的消息，其次按码给一句。
fn exchange_failure(status: reqwest::StatusCode, text: &str) -> String {
    if let Ok(body) = serde_json::from_str::<CloudEnvelope<serde_json::Value>>(text) {
        if !body.message.trim().is_empty() {
            return body.message;
        }
        if let Some(error) = body.error {
            if !error.code.is_empty() {
                return format!("获取本机执行凭证失败（{}）", error.code);
            }
        }
    }
    format!("获取本机执行凭证失败: {}", status)
}

async fn exchange(
    cloud: &CloudClient,
    cloud_base_url: &str,
    session_token: &str,
) -> Result<IssuedAgentToken, String> {
    let url = format!("{}/api/v1/local-agent/agent-token", cloud_base_url);
    let response = cloud
        .post(&url)
        .header("X-Session-Token", session_token)
        .send()
        .await
        .map_err(|e| format!("获取本机执行凭证失败: {}", e))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body: CloudEnvelope<IssuedAgentToken> =
        serde_json::from_str(&text).map_err(|e| format!("获取本机执行凭证响应格式错误: {}", e))?;
    if !status.is_success() || body.errcode != 0 {
        return Err(exchange_failure(status, &text));
    }
    body.data
        .ok_or_else(|| "获取本机执行凭证响应缺少令牌".to_string())
}

/// 把 Token 交给本机 Agent。
///
/// 这个请求里带着 Token，所以失败时只回传状态与码，绝不回传应答正文：
/// Agent 一旦回显请求内容，那句话会直接进日志和页面。
pub(crate) async fn push(client: &LocalAgentClient, token: &AgentToken) -> Result<(), String> {
    let response = client
        .post("/api/v1/agent-token")
        .json(&serde_json::json!({"token": token.token, "expires_at": token.expires_at}))
        .send()
        .await
        .map_err(|e| format!("写入Local Agent执行凭证失败: {}", e))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(match LocalErrorBody::code_of(&text) {
            Some(code) => format!("写入Local Agent执行凭证失败: {}（{}）", status, code),
            None => format!("写入Local Agent执行凭证失败: {}", status),
        });
    }
    let answer: AgentTokenAck = serde_json::from_str(&text)
        .map_err(|e| format!("解析Local Agent执行凭证响应失败: {}", e))?;
    if !answer.data.has_agent_token {
        return Err("Local Agent 没有保存执行凭证，任务将无法领取".into());
    }
    Ok(())
}

async fn configure(
    client: &LocalAgentClient,
    cloud: &CloudClient,
    tokens: &AgentTokenState,
    args: &ConfigureTokenArgs,
) -> Result<AgentTokenFacts, String> {
    let cloud_base_url = cloud_base_url(&args.cloud_base_url)?;
    let session_token = args.session_token.trim();
    if session_token.is_empty() {
        return Err(NO_SESSION_TOKEN.to_string());
    }
    let issued = exchange(cloud, &cloud_base_url, session_token).await?;
    // 先持有再推送：推送失败时下一次启动还能从这里重推，而已经换到手的
    // Token 在过期前仍可用于上报在途任务。
    let token = AgentToken {
        token: issued.token,
        expires_at: issued.expires_at,
    };
    tokens.set(token.clone());
    push(client, &token).await?;
    Ok(facts(tokens))
}

#[tauri::command]
pub async fn local_agent_configure_token(
    client: State<'_, LocalAgentClient>,
    cloud: State<'_, CloudClient>,
    tokens: State<'_, AgentTokenState>,
    args: ConfigureTokenArgs,
) -> Result<AgentTokenFacts, String> {
    configure(&client, &cloud, &tokens, &args).await
}

/// 只读：本进程现在有没有一张执行凭证。页面用它显示「执行凭证」那一行，
/// 不触发任何请求。
#[tauri::command]
pub fn local_agent_token_facts(tokens: State<'_, AgentTokenState>) -> AgentTokenFacts {
    facts(&tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{arrivals, requests, serving, silent_port};
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;

    const ISSUED: &str = "v1.payload.signature";

    fn config_to(port: u16) -> crate::config::DesktopConfig {
        let text = PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}"));
        load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config")
    }

    fn clients(cloud_port: u16, agent_port: u16) -> (CloudClient, LocalAgentClient) {
        (
            CloudClient::new(&config_to(cloud_port)),
            LocalAgentClient::new(&config_to(agent_port), RuntimeToken::generate()),
        )
    }

    fn issued(token: &str) -> String {
        format!(
            r#"{{"errcode":0,"message":"","data":{{"token":"{token}","expires_at":1800000000}}}}"#
        )
    }

    fn held(has_token: bool) -> String {
        format!(r#"{{"data":{{"has_agent_token":{has_token},"expires_at":1800000000}}}}"#)
    }

    fn args(cloud_port: u16, session: &str) -> ConfigureTokenArgs {
        ConfigureTokenArgs {
            cloud_base_url: format!("http://127.0.0.1:{cloud_port}/"),
            session_token: session.to_string(),
        }
    }

    /// 换到手、推给 Agent、页面只看到事实。
    ///
    /// 三个方向一起断言：会话凭据只去 Cloud，Agent Token 只去 Agent，返回给
    /// 页面的结构里一个 Token 都没有。
    #[test]
    fn a_configured_token_is_held_and_pushed_to_the_agent() {
        let (cloud_port, cloud_seen) = serving(|_| ("200 OK".into(), issued(ISSUED)));
        let (agent_port, agent_seen) = serving(|_| ("200 OK".into(), held(true)));
        let (cloud, agent) = clients(cloud_port, agent_port);
        let tokens = AgentTokenState::default();

        let facts = tauri::async_runtime::block_on(configure(
            &agent,
            &cloud,
            &tokens,
            &args(cloud_port, "session-abc"),
        ))
        .expect("both halves answer");

        assert_eq!(
            facts,
            AgentTokenFacts {
                configured: true,
                expires_at: Some(1_800_000_000),
            }
        );
        assert_eq!(
            serde_json::to_string(&facts).expect("serializable"),
            r#"{"configured":true,"expires_at":1800000000}"#
        );

        let cloud_request = requests(&cloud_seen).remove(0);
        assert!(cloud_request.starts_with("POST /api/v1/local-agent/agent-token "));
        assert!(
            cloud_request
                .to_lowercase()
                .contains("x-session-token: session-abc"),
            "the session is what Cloud exchanges: {cloud_request}"
        );
        assert!(
            !cloud_request.contains(ISSUED),
            "the issued token is the answer, not part of the question: {cloud_request}"
        );

        let agent_request = requests(&agent_seen).remove(0);
        assert!(agent_request.starts_with("POST /api/v1/agent-token "));
        assert!(
            agent_request.contains(ISSUED),
            "the Agent has to receive the token: {agent_request}"
        );
        assert!(
            !agent_request.contains("session-abc"),
            "the session stays with Cloud: {agent_request}"
        );
        assert!(
            !serde_json::to_string(&facts).unwrap().contains(ISSUED),
            "nothing returned to the page can carry the token"
        );
    }

    /// Agent 的拒绝按码命名，且句子由状态与码组成——没有 Token。
    #[test]
    fn a_refused_push_is_named_by_code_and_never_echoes_the_token() {
        let (agent_port, agent_seen) = serving(|_| {
            (
                "401 Unauthorized".into(),
                r#"{"error":{"code":"agent_token_required"}}"#.into(),
            )
        });
        let agent = LocalAgentClient::new(&config_to(agent_port), RuntimeToken::generate());
        let tokens = AgentTokenState::default();
        tokens.set(AgentToken {
            token: ISSUED.into(),
            expires_at: None,
        });

        let message = tauri::async_runtime::block_on(push(&agent, &tokens.get().expect("held")))
            .expect_err("a 401 is a failure");

        assert!(message.contains("agent_token_required"), "{message}");
        assert!(
            !message.contains(ISSUED),
            "the token echoed back: {message}"
        );
        assert_eq!(arrivals(&agent_seen, "/api/v1/agent-token"), 1);
    }

    /// Agent 说没存住就是失败：页面不能以为任务会跑。
    #[test]
    fn an_agent_that_did_not_store_the_token_is_a_failure() {
        let (agent_port, _seen) = serving(|_| ("200 OK".into(), held(false)));
        let agent = LocalAgentClient::new(&config_to(agent_port), RuntimeToken::generate());

        let message = tauri::async_runtime::block_on(push(
            &agent,
            &AgentToken {
                token: ISSUED.into(),
                expires_at: None,
            },
        ))
        .expect_err("an unheld token is not a success");

        assert!(message.contains("没有保存执行凭证"), "{message}");
    }

    /// 推送失败不会把已经换到的 Token 丢掉：下一次启动从这里重推。
    #[test]
    fn a_failed_push_leaves_the_token_held_for_the_next_start() {
        let (cloud_port, _cloud_seen) = serving(|_| ("200 OK".into(), issued(ISSUED)));
        let (agent_port, _agent_seen) =
            serving(|_| ("503 Service Unavailable".into(), String::new()));
        let (cloud, agent) = clients(cloud_port, agent_port);
        let tokens = AgentTokenState::default();

        tauri::async_runtime::block_on(configure(
            &agent,
            &cloud,
            &tokens,
            &args(cloud_port, "session-abc"),
        ))
        .expect_err("the push failed");

        assert_eq!(tokens.get().expect("still held").token, ISSUED);
    }

    /// Cloud 的拒绝句子原样交给页面。
    #[test]
    fn a_cloud_refusal_keeps_clouds_own_words() {
        let (cloud_port, _seen) = serving(|_| {
            (
                "409 Conflict".into(),
                r#"{"errcode":20010,"message":"你的账号已在另一台电脑登录","error":{"code":"DESKTOP_LOGIN_CONFLICT"}}"#
                    .into(),
            )
        });
        let (cloud, agent) = clients(cloud_port, silent_port());
        let tokens = AgentTokenState::default();

        let message = tauri::async_runtime::block_on(configure(
            &agent,
            &cloud,
            &tokens,
            &args(cloud_port, "session-abc"),
        ))
        .expect_err("a refusal is not a token");

        assert_eq!(message, "你的账号已在另一台电脑登录");
    }

    /// 没有地址或没有会话：一个请求都不发。
    ///
    /// 两个端口都没人听；如果哪一步先发了请求，错误会是连接失败而不是这两句。
    #[test]
    fn an_incomplete_exchange_never_leaves_the_process() {
        let (cloud, agent) = clients(silent_port(), silent_port());
        let tokens = AgentTokenState::default();

        for (case, args) in [
            (
                "no address",
                ConfigureTokenArgs {
                    cloud_base_url: "   ".into(),
                    session_token: "session-abc".into(),
                },
            ),
            (
                "no session",
                ConfigureTokenArgs {
                    cloud_base_url: "http://127.0.0.1:1".into(),
                    session_token: String::new(),
                },
            ),
        ] {
            let error = tauri::async_runtime::block_on(configure(&agent, &cloud, &tokens, &args))
                .expect_err(case);
            assert!(
                error == NO_CLOUD_ADDRESS_TOKEN || error == NO_SESSION_TOKEN,
                "{case}: {error}"
            );
        }
        assert!(tokens.get().is_none());
    }

    /// 命令都注册了，且没有多余的。
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
                        .strip_prefix("commands::agent_token::")
                        .map(str::to_string)
                })
                .collect()
        }

        let mut declared = declared(include_str!("agent_token.rs"));
        let mut registered = registered(include_str!("../main.rs"));
        declared.sort();
        registered.sort();

        assert_eq!(declared, registered);
        assert_eq!(declared.len(), 2, "the denominator: {declared:?}");
    }

    /// 只读的「有没有凭证」：空状态如实回答没有，有凭证时只给过期时间。
    #[test]
    fn the_facts_say_whether_a_token_is_held_and_never_what_it_is() {
        let tokens = AgentTokenState::default();
        assert_eq!(
            serde_json::to_string(&facts(&tokens)).unwrap(),
            r#"{"configured":false,"expires_at":null}"#
        );

        tokens.set(AgentToken {
            token: ISSUED.into(),
            expires_at: Some(1_800_000_000),
        });
        let answer = serde_json::to_string(&facts(&tokens)).unwrap();
        assert_eq!(answer, r#"{"configured":true,"expires_at":1800000000}"#);
        assert!(!answer.contains(ISSUED), "the token leaked: {answer}");
    }
}
