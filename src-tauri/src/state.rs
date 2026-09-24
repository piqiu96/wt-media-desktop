//! Native state managed by Tauri and read by the command modules.
//!
//! None of this reaches Vue. `RuntimeBinding` holds the Cloud node credential,
//! which by design never leaves native memory: the bind command returns only
//! `BoundNodeFacts`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tauri_plugin_shell::process::CommandChild;
use uuid::Uuid;

/// Owns the Local Agent child for the lifetime of the Desktop session.
/// Keeping the handle here makes start/stop deterministic in both packaged
/// sidecar mode and the Python fallback used by development builds.
#[derive(Default)]
pub struct AgentProcess(pub Mutex<Option<CommandChild>>);

/// The id of the Agent session Desktop is currently supervising, if any.
///
/// **In-process only**, by ruling (D-10): Desktop sends no `X-Operation-Id`, the
/// Agent consumes no such header, and the value never enters the sidecar's
/// environment. It exists so the records *around* one Agent session — start,
/// health, stop, exit — can be read as one session rather than as a series of
/// unrelated lines, and so two sessions in one desktop run are told apart.
///
/// Held beside `AgentProcess` because the two answer the same question and must
/// not disagree: `start` sets it when it stores a child, `stop` clears it when it
/// takes the child away.
#[derive(Clone, Default)]
pub struct OperationId(Arc<Mutex<Option<String>>>);

impl OperationId {
    /// A fresh id for one Agent session.
    ///
    /// v4, from the dependency `RuntimeToken` already uses. Not derived from the
    /// token, the port or the clock: it is a label for a session, and a label
    /// that could be read back into anything else would be a second way to ask
    /// this process what it is doing.
    pub fn generate() -> String {
        Uuid::new_v4().to_string()
    }

    /// Remember `id` as the supervised session's.
    ///
    /// A poisoned lock is deliberately not an error. The id is diagnostic
    /// metadata, and failing a start because a label could not be written would
    /// trade a missing label for a broken feature; the records then read exactly
    /// as they do outside a session, which is a shape the reader already knows.
    pub fn set(&self, id: String) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(id);
        }
    }

    /// Forget the session — the Agent is no longer being supervised.
    pub fn clear(&self) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = None;
        }
    }

    /// The session's id, or `None` when no session is being supervised.
    ///
    /// Returns an owned `String` because the lock cannot be held across the call
    /// sites: a record is emitted while the guard would still be alive.
    pub fn current(&self) -> Option<String> {
        self.0.lock().ok().and_then(|slot| slot.clone())
    }
}

/// The sidecar's recent output, for diagnostics only.
///
/// Shared rather than owned by the reader so a failed start or health check can
/// report what the sidecar last printed. The buffer's rules (capacity, blank
/// handling, the summary shape) live in `sidecar::drain`; this is only the
/// handle Tauri manages.
#[derive(Clone, Default)]
pub struct SidecarLog(pub Arc<Mutex<VecDeque<String>>>);
#[derive(Default)]
pub struct RuntimeBindingState(pub Mutex<Option<RuntimeBinding>>);
#[derive(Clone, Debug)]
pub struct RuntimeBinding {
    pub node_id: String,
    pub node_credential: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Nothing is supervised until a start says so.
    #[test]
    fn a_fresh_holder_has_no_session() {
        assert_eq!(OperationId::default().current(), None);
    }

    /// Set, read back, clear — and clear is a real change of state, not a
    /// no-op that happens to read as `None` afterwards.
    #[test]
    fn the_session_is_held_until_it_is_cleared() {
        let session = OperationId::default();
        session.set("session-one".to_string());
        assert_eq!(session.current().as_deref(), Some("session-one"));

        session.clear();
        assert_eq!(session.current(), None, "clearing must be observable");

        session.set("session-two".to_string());
        assert_eq!(session.current().as_deref(), Some("session-two"));
    }

    /// Every session gets its own id, and it is a v4 UUID.
    ///
    /// The shape is pinned because "distinct strings" alone would accept a
    /// counter, and a counter in a log line reads as a sequence number rather
    /// than as an identifier. The count of distinct values is what rules out a
    /// constant, which is the mutation this exists for.
    #[test]
    fn each_session_gets_its_own_uuid() {
        let ids: Vec<String> = (0..32).map(|_| OperationId::generate()).collect();

        assert_eq!(
            ids.iter().collect::<BTreeSet<_>>().len(),
            32,
            "ids must not repeat within a run: {ids:?}"
        );
        for id in &ids {
            assert_eq!(id.len(), 36, "not a UUID: {id}");
            assert_eq!(
                id.chars().filter(|c| *c == '-').count(),
                4,
                "not a hyphenated UUID: {id}"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
                "not hexadecimal: {id}"
            );
        }
    }
}
