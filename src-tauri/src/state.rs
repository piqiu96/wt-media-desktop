//! Native state managed by Tauri and read by the command modules.
//!
//! None of this reaches Vue. `RuntimeBinding` holds the Cloud node credential,
//! which by design never enters the WebView: the bind command returns only
//! `BoundNodeFacts`. It is persisted to native app data (0600, like the device
//! key) so a Desktop restart can restore it without re-binding.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::Manager;
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

impl RuntimeBindingState {
    /// Persist the current binding (if any) to native app data.
    ///
    /// Called by the bind command after it replaces the in-memory binding. A
    /// failure only degrades the next launch to "not bound" — the current run
    /// keeps working — so it is logged, not propagated.
    pub fn persist(&self, app: &tauri::AppHandle) {
        let Some(binding) = self.0.lock().ok().and_then(|slot| slot.clone()) else {
            return;
        };
        let directory = match app.path().app_data_dir() {
            Ok(dir) => dir,
            Err(err) => {
                tracing::warn!(target: "desktop.binding", "resolve app data dir: {err}");
                return;
            }
        };
        if let Err(err) = write_binding(&directory, &binding) {
            tracing::warn!(target: "desktop.binding", "persist binding: {err}");
        }
    }

    /// Restore the persisted binding (if any) into native memory.
    ///
    /// Called once in `.setup()`, where an `AppHandle` exists (it does not at
    /// `.manage()` time). Absence is not an error — it is an unbound machine —
    /// and neither is a corrupt file: treat it as no binding rather than
    /// refusing to start.
    pub fn restore(&self, app: &tauri::AppHandle) {
        let Ok(directory) = app.path().app_data_dir() else {
            return;
        };
        if let Some(binding) = read_binding(&directory) {
            if let Ok(mut slot) = self.0.lock() {
                *slot = Some(binding);
            }
        }
    }
}

/// A Cloud node credential, persisted beside `device-identity.pk8` with the
/// same 0600 protection. Never serialized to the WebView.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeBinding {
    pub node_id: String,
    pub node_credential: String,
}

const BINDING_FILE: &str = "runtime-binding.json";

/// Write the binding atomically (temp file + rename) so a crash mid-write
/// cannot leave a truncated file that later reads as a binding. Mirror of the
/// device-identity write path, including the 0600 mode on unix.
fn write_binding(directory: &Path, binding: &RuntimeBinding) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|_| "无法创建应用数据目录".to_string())?;
    let path = directory.join(BINDING_FILE);
    let temp = directory.join(format!("{BINDING_FILE}.tmp"));
    let json = serde_json::to_vec(binding).map_err(|_| "无法序列化运行绑定".to_string())?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|_| "无法安全写入运行绑定".to_string())?;
    file.write_all(&json)
        .map_err(|_| "无法写入运行绑定".to_string())?;
    file.sync_all()
        .map_err(|_| "无法保存运行绑定".to_string())?;
    fs::rename(&temp, &path).map_err(|_| "无法完成运行绑定保存".to_string())?;
    Ok(())
}

fn read_binding(directory: &Path) -> Option<RuntimeBinding> {
    let json = fs::read(directory.join(BINDING_FILE)).ok()?;
    serde_json::from_slice(&json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The binding written in one process must read back intact in the next —
    /// that is the whole point of persistence — and the file must not be
    /// world-readable. The unix mode is asserted because a credential file
    /// that anyone can read is a credential that might as well not be
    /// protected; on the platforms where 0600 is meaningful, it is the rule.
    #[test]
    fn a_binding_written_reads_back_with_0600_permissions() {
        let directory =
            std::env::temp_dir().join(format!("wt-media-binding-{}", uuid::Uuid::new_v4()));
        let original = RuntimeBinding {
            node_id: "node-1".into(),
            node_credential: "credential-1".into(),
        };
        write_binding(&directory, &original).unwrap();

        let restored = read_binding(&directory).expect("binding must survive restart");
        assert_eq!(restored.node_id, original.node_id);
        assert_eq!(restored.node_credential, original.node_credential);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(directory.join(BINDING_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "credential file must be 0600");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    /// A machine that never bound, and a corrupt binding file, both read as
    /// absence — never as an error that would block startup.
    #[test]
    fn a_missing_or_corrupt_binding_reads_as_absence() {
        let directory =
            std::env::temp_dir().join(format!("wt-media-binding-{}", uuid::Uuid::new_v4()));
        assert!(read_binding(&directory).is_none(), "no file, no binding");

        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(BINDING_FILE), b"not json").unwrap();
        assert!(
            read_binding(&directory).is_none(),
            "corrupt file, no binding"
        );
        fs::remove_dir_all(directory).unwrap();
    }

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
