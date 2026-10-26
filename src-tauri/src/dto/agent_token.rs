//! Agent Token 在本机的形状：换取它只需要会话，交回证据时只回答是否持有。
//!
//! Token 本身不出现在任何返回给页面的结构里。它能代表这台电脑当前的 Desktop
//! 登录行事，所以 `AgentTokenFacts` 只有「配置没有」和「何时过期」两件事。

use serde::{Deserialize, Serialize};

/// 换取 Agent Token 的参数。
#[derive(Clone, Debug, Deserialize)]
pub struct ConfigureTokenArgs {
    pub cloud_base_url: String,
    pub session_token: String,
}

/// Cloud 签发应答里的令牌与过期时间（Unix 秒）。
#[derive(Clone, Debug, Deserialize)]
pub struct IssuedAgentToken {
    pub token: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// 页面能看到的全部：配置没有、何时过期。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentTokenFacts {
    pub configured: bool,
    pub expires_at: Option<i64>,
}

/// Agent 本地 `/api/v1/agent-token` 的应答。
#[derive(Clone, Debug, Deserialize)]
pub struct AgentTokenAck {
    pub data: AgentTokenAckData,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AgentTokenAckData {
    pub has_agent_token: bool,
}
