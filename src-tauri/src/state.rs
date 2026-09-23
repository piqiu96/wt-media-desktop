//! Native state managed by Tauri and read by the command modules.
//!
//! None of this reaches Vue. `RuntimeBinding` holds the Cloud node credential,
//! which by design never leaves native memory: the bind command returns only
//! `BoundNodeFacts`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tauri_plugin_shell::process::CommandChild;

/// Owns the Local Agent child for the lifetime of the Desktop session.
/// Keeping the handle here makes start/stop deterministic in both packaged
/// sidecar mode and the Python fallback used by development builds.
#[derive(Default)]
pub struct AgentProcess(pub Mutex<Option<CommandChild>>);

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
