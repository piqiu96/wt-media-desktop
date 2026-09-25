//! WebView diagnostics forwarded to Desktop's own log.
//!
//! These are the only records under the `webview` target, and they are the
//! reason that target is Desktop's: a JS exception inside the packaged app is
//! the app failing, not the Agent, and it used to go to stdout where a packaged
//! build had nowhere to show it.
//!
//! Two records, not one: the message and the stack are separate arguments from
//! the frontend and they are read separately — a stack on its own is noise, and
//! a message on its own does not say where. An empty stack is no record at all,
//! which is the common case for a caught error reported without one.
#[tauri::command]
pub fn log_js_error(message: String, stack: String) {
    tracing::error!(target: "webview", "{message}");
    if !stack.is_empty() {
        tracing::error!(target: "webview", "[stack] {stack}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::test_support::{capture, written};
    use tracing::subscriber::with_default;

    /// Both halves arrive, under this target, at a level production keeps.
    ///
    /// The target is the assertion, not the level: `webview` is one of the three
    /// targets the log owns, and a record filed under any other name is a record
    /// the reader looking for JS failures will not find.
    #[test]
    fn the_stack_and_the_message_are_two_records_under_this_target() {
        let (directory, subscriber) = capture("webview-error");
        with_default(subscriber, || {
            log_js_error("boom".into(), "at handler (app.js:1:2)".into())
        });

        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 2, "message and stack: {text}");
        assert!(text.contains("[ERROR] webview: boom"), "{text}");
        assert!(
            text.contains("[ERROR] webview: [stack] at handler (app.js:1:2)"),
            "{text}"
        );
    }

    /// An empty stack is one record, not two: the frontend sends `""` for every
    /// error it reports without one.
    #[test]
    fn an_empty_stack_is_not_a_record() {
        let (directory, subscriber) = capture("webview-no-stack");
        with_default(subscriber, || log_js_error("boom".into(), String::new()));

        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("[ERROR] webview: boom"), "{text}");
        assert!(!text.contains("[stack]"), "{text}");
    }
}
