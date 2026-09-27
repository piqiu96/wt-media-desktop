//! Wire types crossing the Rust boundary, grouped by the flow that consumes
//! them: `agent` (Local Agent status and runtime facts), `bind` (Cloud node
//! registration), `account` (account check and cookie read), `profile`
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
mod bind;
mod cleanup;
mod config;
mod diagnostic;
mod downloads;
mod profile;
mod saved_files;
mod settings;
mod storage;

pub use account::*;
pub use agent::*;
pub use bind::*;
pub use cleanup::*;
pub use config::*;
pub use diagnostic::*;
pub use downloads::*;
pub use profile::*;
pub use saved_files::*;
pub use settings::*;
pub use storage::*;

use serde::Deserialize;

/// Cloud's uniform response envelope: `{errcode, message, data}`.
///
/// Lives here rather than in `bind` or `account` because both flows read it.
/// Putting it in either one would make the other import from a sibling it does
/// not own.
#[derive(Clone, Debug, Deserialize)]
pub struct CloudEnvelope<T> {
    pub errcode: i32,
    #[serde(default)]
    pub message: String,
    pub data: Option<T>,
}

/// The Local Agent's refusal body: `{"error": {"code": …}}`.
///
/// Beside `CloudEnvelope` for the same reason it is here rather than in one
/// flow: three flows read it (`bind`, `downloads`, and every one to come). It is
/// also the **narrower** of the two on purpose — the Agent writes a code and,
/// sometimes, a `message` that is another service's own error text, and this
/// struct reads one field. `code_of` is the whole of what a caller needs, and it
/// returns a code rather than the body so that no caller can decide to pass a
/// peer's text on: `contracts/local-error-codes/v1/transfer.yaml` puts
/// `node_credential` in its `secret_policy.forbidden_fields`, and the refusal to
/// a request that carried a credential is the one place a peer echoing its input
/// would reach a log.
#[derive(Clone, Debug, Deserialize)]
pub struct LocalErrorBody {
    pub error: LocalErrorDetail,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LocalErrorDetail {
    pub code: String,
}

impl LocalErrorBody {
    /// The code in `text`, or `None` when `text` is not a Local Agent refusal.
    ///
    /// `None` rather than a placeholder: the caller has to say something about a
    /// body it did not recognise, and 「拒绝，但没说为什么」is a different sentence
    /// from a code it can name.
    pub fn code_of(text: &str) -> Option<String> {
        serde_json::from_str::<LocalErrorBody>(text)
            .ok()
            .map(|body| body.error.code)
    }
}
