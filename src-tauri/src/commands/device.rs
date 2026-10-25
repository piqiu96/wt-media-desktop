//! Read-only device label for the signed-in operator's personal page.

#[tauri::command]
pub fn local_device_id() -> Result<String, String> {
    crate::device_identity::display_id()
}
