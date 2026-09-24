//! Wire types crossing the Rust boundary, grouped by the flow that consumes
//! them: `agent` (Local Agent status and runtime facts), `bind` (Cloud node
//! registration), `account` (account check and cookie read), `profile`
//! (BitBrowser profile operations), `config` (the narrow slice of Desktop's own
//! configuration the page may see), `storage` (the machine's free space, what
//! this app occupies, and the log files it can read back), `cleanup` (what a
//! cleanup did, including what it deliberately did not do), `diagnostic` (what a
//! support bundle holds, and what it left out).
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
mod profile;
mod storage;

pub use account::*;
pub use agent::*;
pub use bind::*;
pub use cleanup::*;
pub use config::*;
pub use diagnostic::*;
pub use profile::*;
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
