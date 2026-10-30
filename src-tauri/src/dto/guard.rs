//! 本机操作与 BitBrowser 绑定共用的拒绝结果。
//!
//! 页面按 `code` 分支：「主账号不一致」要提示换绑，「BitBrowser 没开」要提示启动，
//! 两者不能靠解析文案区分。

use super::LocalErrorBody;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

/// Agent 本地绑定路由的统一应答。主账号全文留在 Rust 与 Agent 之间。
#[derive(Clone, Debug, Deserialize)]
pub struct BitAccountBindAck {
    pub data: Option<serde_json::Value>,
}

/// 绑定结果的可见部分：成功即已绑定，页面回读 Cloud 的掩码视图。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BitAccountBindResult {
    pub bound: bool,
}

/// 页面需要按码分支的拒绝。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GuardRefusal {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub recovery_action: String,
}

impl GuardRefusal {
    /// Agent 明确拒绝：码、可否重试和下一步都以它的回答为准。
    ///
    /// 绑定请求只带 `rebind`，回显的 `message` 不可能是请求里的秘密。
    pub fn from_agent(status: StatusCode, text: &str) -> Self {
        match LocalErrorBody::detail_of(text) {
            Some(detail) => Self {
                code: if detail.code.is_empty() {
                    "cloud_request_failed".into()
                } else {
                    detail.code
                },
                message: if detail.message.trim().is_empty() {
                    format!("请求被拒绝: {status}")
                } else {
                    detail.message
                },
                retryable: detail.retryable,
                recovery_action: detail.recovery_action,
            },
            None => Self::unreadable(status),
        }
    }

    /// 请求没有到达 Agent：值得重试，且没有别的地方可去。
    pub fn unreachable(reason: impl std::fmt::Display) -> Self {
        Self {
            code: "agent_unreachable".into(),
            message: format!("Local Agent 不可达: {reason}"),
            retryable: true,
            recovery_action: String::new(),
        }
    }

    /// 应答读不懂：状态码是唯一的事实。
    pub fn unreadable(status: StatusCode) -> Self {
        Self {
            code: "cloud_request_failed".into(),
            message: format!("请求失败: {status}"),
            retryable: false,
            recovery_action: String::new(),
        }
    }

    /// 200 但缺少结果，不能当作成功。
    pub fn missing_result() -> Self {
        Self {
            code: "cloud_request_failed".into(),
            message: "绑定应答缺少结果".into(),
            retryable: true,
            recovery_action: String::new(),
        }
    }
}
