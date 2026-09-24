//! WebView diagnostics forwarded to the native process's stdout.

// Temporary diagnostics: forwards WebView console/error output to stdout so
// packaged-app JS failures can be captured without opening devtools.
#[tauri::command]
pub fn log_js_error(message: String, stack: String) {
    println!("[WEBVIEW] {}", message);
    if !stack.is_empty() {
        println!("[WEBVIEW-STACK] {}", stack);
    }
}
