//! The Local Agent boundary that is still a pure contract: the non-secret facts
//! a completed binding returns to the Vue layer.
//!
//! `BoundNodeFacts` lives here rather than in `dto/bind.rs` alongside the other
//! bind-flow payloads, and carries no credential field by construction — that
//! is what makes it safe to hand across the IPC boundary. Recorded as a residue:
//! if it moves, it moves with the rest of the bind DTOs, not on its own.

use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BoundNodeFacts {
    pub id: String,
    pub agent_id: String,
    pub user_id: String,
    pub status: String,
}
