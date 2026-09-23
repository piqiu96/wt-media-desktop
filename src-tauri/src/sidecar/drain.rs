//! Keeping the sidecar's output instead of discarding it.
//!
//! Both spawn sites used to bind the event receiver to `_events` and never read
//! it, so every line the Agent wrote was dropped: a sidecar that died during
//! startup left no trace anywhere, and the only symptom was a health check that
//! failed a few seconds later. This keeps the last [`CAPACITY`] lines in memory
//! and reports the tail when the process exits.
//!
//! In memory only — nothing is written to disk, nothing is rotated, nothing is
//! redacted. Redaction belongs to CHG-057, and this is deliberately not a
//! logger.
//!
//! What this does **not** cover:
//!
//! - The buffer dies with Desktop. A sidecar that outlives Desktop leaves
//!   nothing behind.
//! - The shell plugin splits output on newlines rather than into one line per
//!   event, so a single very long line can arrive as several entries and count
//!   against the capacity more than once.
//! - Only the tail is kept: a failure whose cause was printed early and
//!   followed by more than [`CAPACITY`] lines of noise is not recoverable from
//!   here.

use crate::state::SidecarLog;
use std::collections::VecDeque;
use std::sync::MutexGuard;
use tauri_plugin_shell::process::{CommandEvent, TerminatedPayload};
use tokio::sync::mpsc::Receiver;

/// Lines kept in memory.
pub const CAPACITY: usize = 200;
/// Lines reported when the sidecar exits, and appended to a command error.
pub const TAIL_ON_EXIT: usize = 20;

/// The buffer, recovering rather than propagating a poisoned lock.
///
/// This lock guards diagnostics, and losing them is strictly better than taking
/// the app down over them — so a panic elsewhere must not make the sidecar's
/// last words unreadable. The binding lock in `preflight` guards a credential
/// and fails loudly instead; the difference is deliberate, not an oversight.
fn buf(log: &SidecarLog) -> MutexGuard<'_, VecDeque<String>> {
    log.0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Append one line, dropping the oldest once full.
pub fn push(log: &SidecarLog, line: String) {
    let mut buf = buf(log);
    if buf.len() >= CAPACITY {
        buf.pop_front();
    }
    buf.push_back(line);
}

/// The last `n` lines, oldest first.
pub fn tail(log: &SidecarLog, n: usize) -> Vec<String> {
    let buf = buf(log);
    let skip = buf.len().saturating_sub(n);
    buf.iter().skip(skip).cloned().collect()
}

/// How many lines are held right now.
pub fn len(log: &SidecarLog) -> usize {
    buf(log).len()
}

/// Suffix for an existing error message: what the sidecar last printed, or
/// nothing at all when it has said nothing.
///
/// Returns an empty string rather than a "no output" note, so a caller can
/// concatenate unconditionally and a failure that predates any output keeps
/// exactly the message it had before.
pub fn summary(log: &SidecarLog) -> String {
    let lines = tail(log, TAIL_ON_EXIT);
    if lines.is_empty() {
        return String::new();
    }
    format!("\n\nLocal Agent 最近输出：\n{}", lines.join("\n"))
}

/// The part of an output chunk worth keeping, or `None` if there is none.
///
/// Blank chunks are dropped because the plugin splits on `\r` as well as `\n`,
/// so CRLF output arrives as a real line followed by an empty chunk. Keeping
/// those would let a CRLF-heavy startup fill all [`CAPACITY`] slots with
/// nothing, which is exactly the case where the tail matters most.
///
/// Bytes that are not valid UTF-8 are kept lossily rather than dropped: a
/// garbled line still tells you the process was alive and roughly what it said.
fn kept(bytes: &[u8]) -> Option<String> {
    let line = String::from_utf8_lossy(bytes);
    let line = line.trim_end();
    if line.trim().is_empty() {
        return None;
    }
    Some(line.to_string())
}

/// Follow a sidecar's output until it exits, then report the tail on Desktop's
/// own stderr.
///
/// Runs on a detached task: the spawn path is synchronous and must return the
/// child immediately, while reading events is not.
pub fn follow(events: Receiver<CommandEvent>, log: SidecarLog, path: &'static str) {
    tauri::async_runtime::spawn(async move {
        let mut events = events;
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stdout(bytes) | CommandEvent::Stderr(bytes) => {
                    if let Some(line) = kept(&bytes) {
                        push(&log, line);
                    }
                }
                CommandEvent::Error(reason) => {
                    push(&log, format!("[读取 sidecar 输出失败] {reason}"));
                }
                CommandEvent::Terminated(TerminatedPayload { code, signal }) => {
                    report_exit(path, code, signal, &log);
                    return;
                }
                // `CommandEvent` is `#[non_exhaustive]`: a variant added by a
                // future plugin version must not break this build.
                _ => {}
            }
        }
    });
}

/// The text of the exit report.
///
/// Split from printing it so the wording is testable — `eprintln!` is not.
///
/// It reports both how many lines were **held** and how many are shown. The two
/// differ exactly when the buffer overflowed, and that difference is a fact
/// about the diagnosis: if 200 lines were captured and only the last 20 shown,
/// whatever killed the sidecar may have said so earlier, and the reader should
/// know the tail is not the whole story rather than assume it is.
pub fn exit_report(path: &str, code: Option<i32>, signal: Option<i32>, log: &SidecarLog) -> String {
    let how = match (code, signal) {
        (Some(code), _) => format!("退出码 {code}"),
        (None, Some(signal)) => format!("被信号 {signal} 终止"),
        (None, None) => "退出状态未知".to_string(),
    };
    let lines = tail(log, TAIL_ON_EXIT);
    let mut out = format!(
        "[wt-media-desktop] Local Agent（{path}）{how}；缓冲共 {} 行，末 {} 行输出：",
        len(log),
        lines.len()
    );
    for line in &lines {
        out.push_str(&format!("\n[wt-media-desktop] | {line}"));
    }
    out
}

fn report_exit(path: &str, code: Option<i32>, signal: Option<i32>, log: &SidecarLog) {
    eprintln!("{}", exit_report(path, code, signal, log));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_with(lines: &[&str]) -> SidecarLog {
        let log = SidecarLog::default();
        for line in lines {
            push(&log, (*line).to_string());
        }
        log
    }

    #[test]
    fn keeps_the_newest_lines_and_forgets_the_oldest() {
        let log = SidecarLog::default();
        assert_eq!(len(&log), 0);

        for i in 0..CAPACITY + 5 {
            push(&log, format!("line-{i}"));
        }

        assert_eq!(len(&log), CAPACITY, "the buffer must stop growing");
        let held = tail(&log, CAPACITY);
        assert_eq!(held.first().unwrap(), "line-5", "the five oldest must be gone");
        assert_eq!(held.last().unwrap(), &format!("line-{}", CAPACITY + 4));
    }

    /// `tail` is a window on the end, oldest first — not a reversed list.
    #[test]
    fn tail_returns_the_end_in_order() {
        let log = log_with(&["a", "b", "c", "d"]);
        assert_eq!(tail(&log, 2), vec!["c", "d"]);
        assert_eq!(tail(&log, 99), vec!["a", "b", "c", "d"]);
        assert!(tail(&log, 0).is_empty());
    }

    /// A failure that predates any output must keep the message it had before,
    /// so the suffix has to be empty rather than a "no output" note.
    #[test]
    fn summary_is_empty_when_nothing_was_captured() {
        assert_eq!(summary(&SidecarLog::default()), "");
    }

    /// Which chunks become lines. The blank cases are the point: the plugin
    /// splits on `\r` too, so CRLF output yields an empty chunk after every
    /// line, and storing those would fill the buffer with nothing.
    #[test]
    fn blank_chunks_are_dropped_and_garbled_ones_are_kept() {
        assert_eq!(kept(b"starting\n").as_deref(), Some("starting"));
        assert_eq!(kept(b"no trailing newline").as_deref(), Some("no trailing newline"));
        assert_eq!(kept(b"trailing spaces   ").as_deref(), Some("trailing spaces"));
        assert_eq!(kept(b"invalid utf8: \xff\xfe").is_some(), true);

        for blank in [&b""[..], b"\n", b"\r", b"\r\n", b"   ", b"\t\n"] {
            assert_eq!(kept(blank), None, "chunk {blank:?} must not become a line");
        }
    }

    #[test]
    fn summary_appends_the_tail_under_a_header() {
        let log = log_with(&["starting", "bind: 127.0.0.1:8765"]);
        assert_eq!(
            summary(&log),
            "\n\nLocal Agent 最近输出：\nstarting\nbind: 127.0.0.1:8765"
        );
    }

    /// Both counts appear, and the wording is the whole report.
    #[test]
    fn exit_report_names_the_status_held_count_and_the_lines() {
        let log = log_with(&["a", "b"]);
        assert_eq!(
            exit_report("sidecar_started", Some(0), None, &log),
            "[wt-media-desktop] Local Agent（sidecar_started）退出码 0；缓冲共 2 行，末 2 行输出：\
             \n[wt-media-desktop] | a\
             \n[wt-media-desktop] | b"
        );

        // Every status spelling, so a missing branch is visible here.
        assert!(exit_report("started", None, Some(9), &log).contains("被信号 9 终止"));
        assert!(exit_report("started", None, None, &log).contains("退出状态未知"));
        // No output captured is still a report, not a panic or an empty line.
        let report = exit_report("sidecar_started", Some(1), None, &SidecarLog::default());
        assert!(report.contains("缓冲共 0 行，末 0 行输出："), "{report}");
    }

    /// The held count must be the buffer's, not the tail's — otherwise a
    /// truncated tail looks like the whole story.
    #[test]
    fn exit_report_distinguishes_held_lines_from_shown_lines() {
        let log = SidecarLog::default();
        for i in 0..CAPACITY {
            push(&log, format!("l{i}"));
        }
        let report = exit_report("sidecar_started", Some(1), None, &log);
        assert!(
            report.contains(&format!("缓冲共 {CAPACITY} 行，末 {TAIL_ON_EXIT} 行输出")),
            "a full buffer must report {CAPACITY} held and {TAIL_ON_EXIT} shown: {report}"
        );
        assert!(report.contains("| l199"), "the newest line must be shown");
        assert!(!report.contains("| l0"), "the oldest must have been evicted");
    }

    /// The exit report and the error suffix show the same window, so the plan's
    /// "last 20 lines" is one number in one place.
    ///
    /// The comparisons below are against `TAIL_ON_EXIT`, so they cannot detect a
    /// changed number on their own — hence the literal pins first. Without them,
    /// editing the constant to 10 would leave this test green.
    #[test]
    fn the_reported_window_is_the_same_one_either_way() {
        assert_eq!(CAPACITY, 200, "the plan fixes this capacity; changing it is a deliberate edit");
        assert_eq!(TAIL_ON_EXIT, 20, "the plan fixes this tail; changing it is a deliberate edit");

        let log = SidecarLog::default();
        for i in 0..50 {
            push(&log, format!("l{i}"));
        }
        let tail_lines = tail(&log, TAIL_ON_EXIT);
        assert_eq!(tail_lines.len(), TAIL_ON_EXIT);
        assert_eq!(tail_lines.first().unwrap(), "l30");
        assert!(summary(&log).ends_with(&tail_lines.join("\n")));
    }
}
