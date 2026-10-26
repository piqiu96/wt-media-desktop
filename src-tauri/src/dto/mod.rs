//! Wire types crossing the Rust boundary, grouped by the flow that consumes
//! them: `agent` (Local Agent status), `agent_token` (the session-bound token),
//! `guard` (what a sensitive operation or a BitBrowser binding was allowed to
//! do, and why not), `account` (account check and cookie read), `profile`
//! (BitBrowser profile operations), `config` (the narrow slice of Desktop's own
//! configuration the page may see), `storage` (the machine's free space, what
//! this app occupies, and the log files it can read back), `cleanup` (what a
//! cleanup did, including what it deliberately did not do), `diagnostic` (what a
//! support bundle holds, and what it left out), `settings` (the operator's own
//! choice of save location, as the 本机设置 page reads and writes it),
//! `downloads` (what the Agent says about the folder it was given, which the
//! page reads back unchanged), `saved_files` (where this machine's downloaded
//! files are, and what a move or a delete of them did).
//!
//! Field names and serde attributes are load-bearing: the Vue layer asserts on
//! exact argument objects (`web/src/apps/desktop/features/local-agent/
//! localAgentService.test.js`), so a renamed field is a behaviour change, not a
//! cleanup.

mod account;
mod agent;
mod agent_token;
mod cleanup;
mod config;
mod diagnostic;
mod downloads;
mod guard;
mod profile;
mod saved_files;
mod settings;
mod storage;

pub use account::*;
pub use agent::*;
pub use agent_token::*;
pub use cleanup::*;
pub use config::*;
pub use diagnostic::*;
pub use downloads::*;
pub use guard::*;
pub use profile::*;
pub use saved_files::*;
pub use settings::*;
pub use storage::*;

use serde::Deserialize;

/// Cloud's uniform response envelope: `{errcode, message, data, error}`.
#[derive(Clone, Debug, Deserialize)]
pub struct CloudEnvelope<T> {
    pub errcode: i32,
    #[serde(default)]
    pub message: String,
    pub data: Option<T>,
    #[serde(default)]
    pub error: Option<CloudErrorDetail>,
}

/// Cloud 的机器可读错误：页面按 `code` 分支，不用解析文案。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CloudErrorDetail {
    #[serde(default)]
    pub code: String,
}

/// Agent 的拒绝体：`{"error": {"code": …, "message": …}}`。
///
/// `message` 可能原样转发 Cloud 自己写的句子，只在本机请求没有携带秘密时
/// 才使用。请求里带过凭据的流程只读 `code`：Agent 一旦回显请求内容，
/// 那句话会直接进日志。
#[derive(Clone, Debug, Deserialize)]
pub struct LocalErrorBody {
    pub error: LocalErrorDetail,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LocalErrorDetail {
    pub code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub recovery_action: String,
}

impl LocalErrorBody {
    pub fn code_of(text: &str) -> Option<String> {
        serde_json::from_str::<LocalErrorBody>(text)
            .ok()
            .map(|body| body.error.code)
    }

    pub fn detail_of(text: &str) -> Option<LocalErrorDetail> {
        serde_json::from_str::<LocalErrorBody>(text)
            .ok()
            .map(|body| body.error)
    }
}
