//! Bind flow: the one-use Cloud ticket → Cloud node registration → Local Agent
//! node write → the non-secret facts Vue is allowed to see.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BindResponse {
    pub node_id: String,
    pub session_token: String,
    pub status: String,
    /// What the Agent says it is **holding**, read back from the store rather
    /// than from the request (CHG-061 T-04).
    ///
    /// The one field here Desktop checks instead of passing on: a binding that
    /// carried a credential and answers `false` is a write that was dropped, and
    /// nothing else in this answer can tell that apart from a successful one.
    /// `#[serde(default)]` because the local-only bind sends no credential and
    /// the answer is the same shape — the field is absent from nothing, but a
    /// struct that failed to decode would fail the bind for a reason that is not
    /// the bind's.
    #[serde(default)]
    pub has_node_credential: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BindSessionArgs {
    pub binding_ticket: String,
    pub cloud_base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RefreshRuntimeArgs {
    pub cloud_base_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegisterLocalNodeRequest {
    pub binding_token: String,
    pub agent_id: String,
    pub device_id: String,
    pub agent_version: String,
    pub contract_major_version: String,
    pub contract_revision: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegisterLocalNodeResponse {
    pub node: RegisteredNode,
    pub node_credential: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegisteredNode {
    pub id: String,
    pub agent_id: String,
    pub user_id: i64,
    pub status: String,
}

/// Outcome of Cloud's sensitive-task preflight. Consumed by the account-check
/// and cookie-read flows, which both go through the same Cloud endpoint.
#[derive(Clone, Debug, Deserialize)]
pub struct GuardPreflightOutcome {
    pub outcome: String,
    #[serde(default)]
    pub permit_id: String,
    #[serde(default)]
    pub permit_credential: String,
}
