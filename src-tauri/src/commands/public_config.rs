//! The page's read-only view of Desktop's own configuration.

use crate::config::DesktopConfig;
use crate::dto::PublicConfig;
use tauri::State;

/// The config facts the page is allowed to know.
///
/// Every other flow already goes through a command; this closes the last one
/// that did not. `init.js` used to answer "where is Cloud?" with a loopback
/// literal, which is right on the machine it was written on and wrong
/// everywhere else — and it is the value the packaged build got wrong, because
/// its own origin is `tauri.localhost`.
///
/// Reads the config Tauri manages rather than re-resolving it, so there is one
/// answer per launch: the same value `apply_csp` used, and the same one the
/// sidecar will be started with.
#[tauri::command]
pub fn get_public_config(config: State<'_, DesktopConfig>) -> PublicConfig {
    PublicConfig::from_config(&config)
}
