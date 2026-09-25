//! Keeping the sidecar's output instead of discarding it.
//!
//! Both spawn sites used to bind the event receiver to `_events` and never read
//! it, so every line the Agent wrote was dropped: a sidecar that died during
//! startup left no trace anywhere, and the only symptom was a health check that
//! failed a few seconds later. This keeps the last [`CAPACITY`] lines in memory
//! and reports the tail when the process exits.
//!
//! Where the lines go is the whole point of the split (ruling 八 / D-07): the
//! Agent's own records belong to the Agent's own log, so what arrives here is
//! **buffered and not logged**, with two deliberate exceptions — a read failure,
//! which is Desktop's problem and not the Agent's, and the exit report, which is
//! Desktop managing the process. Put the ordinary case in the log and every line
//! the Agent writes appears twice, burying the lifecycle records that are
//! Desktop's business under the Agent's own output.
//!
//! What this does **not** cover:
//!
//! - The buffer dies with Desktop. A sidecar that outlives Desktop leaves
//!   nothing behind on its way out.
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

/// One command event, reduced to what this module actually acts on.
///
/// The reduction exists so the decisions below are reachable from a test:
/// `CommandEvent` is `#[non_exhaustive]`, so a test cannot build one, and
/// everything inside `follow` would otherwise only be observable by running a
/// real sidecar. What stays there is the three-arm match and nothing else.
enum Heard {
    /// A line the sidecar printed.
    Line(String),
    /// The event stream itself failed.
    Failure(String),
    /// The process is gone.
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
}

/// What one event means, or `None` for one this module ignores.
fn classify(event: CommandEvent) -> Option<Heard> {
    match event {
        CommandEvent::Stdout(bytes) | CommandEvent::Stderr(bytes) => kept(&bytes).map(Heard::Line),
        CommandEvent::Error(reason) => Some(Heard::Failure(reason)),
        CommandEvent::Terminated(TerminatedPayload { code, signal }) => {
            Some(Heard::Exited { code, signal })
        }
        // `CommandEvent` is `#[non_exhaustive]`: a variant added by a future
        // plugin version must not break this build.
        _ => None,
    }
}

/// Act on one event; `true` when the sidecar is done and the loop can end.
fn heard(event: Heard, log: &SidecarLog, path: &str) -> bool {
    match event {
        // The ordinary case, and the one the ruling is about: buffered, and
        // **not** logged. This is where D-07 and AC-09 are kept.
        Heard::Line(line) => {
            push(log, line);
            false
        }
        Heard::Failure(reason) => {
            let line = format!("[读取 sidecar 输出失败] {reason}");
            push(log, line.clone());
            // Also a record, unlike an ordinary line: a read failure is
            // Desktop's problem, not the Agent's, and one that happens while the
            // process is still alive would otherwise stay invisible until it
            // exits — which it may never do.
            tracing::warn!(target: "agent.supervisor", "{line}");
            false
        }
        Heard::Exited { code, signal } => {
            report_exit(path, code, signal, log);
            true
        }
    }
}

/// Follow a sidecar's output until it exits, then report the tail as a record.
///
/// Runs on a detached task: the spawn path is synchronous and must return the
/// child immediately, while reading events is not.
pub fn follow(events: Receiver<CommandEvent>, log: SidecarLog, path: &'static str) {
    tauri::async_runtime::spawn(async move {
        let mut events = events;
        while let Some(event) = events.recv().await {
            if let Some(event) = classify(event) {
                if heard(event, &log, path) {
                    return;
                }
            }
        }
    });
}

/// The text of the exit report.
///
/// Split from emitting it so the wording is testable.
///
/// The wording is unchanged from the `eprintln!` this replaces, `[wt-media-desktop]`
/// prefix and all: three tests pin it to the character, and a reader grepping
/// for it is grepping for the same string as before. What changed is where it
/// goes — the record's own stamp, level and target now frame it.
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

/// Say the sidecar is gone, once, as Desktop's own record.
///
/// The tail is the one place the last [`TAIL_ON_EXIT`] lines of the Agent's
/// output reach Desktop's log (ruling 五): a command error carries the same
/// window back to the caller, and the health path deliberately does not put it
/// in a record at all. One exit, one record, so a reader counting them gets
/// one per sidecar rather than one per health poll.
///
/// `single_line` in the sink turns the report's newlines into `\n` escapes, so
/// the whole report stays a single record however much the sidecar printed.
fn report_exit(path: &str, code: Option<i32>, signal: Option<i32>, log: &SidecarLog) {
    tracing::info!(
        target: "agent.supervisor",
        "{}",
        exit_report(path, code, signal, log)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::test_support::{capture, written};
    use tracing::subscriber::with_default;

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
        assert_eq!(
            held.first().unwrap(),
            "line-5",
            "the five oldest must be gone"
        );
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
        assert_eq!(
            kept(b"no trailing newline").as_deref(),
            Some("no trailing newline")
        );
        assert_eq!(
            kept(b"trailing spaces   ").as_deref(),
            Some("trailing spaces")
        );
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
        assert!(
            !report.contains("| l0"),
            "the oldest must have been evicted"
        );
    }

    /// Ordinary output is buffered and **not** logged (AC-09, D-07).
    ///
    /// The control arm is the point: the same fifty lines must be *in the
    /// buffer*, or "no record" could equally be "the lines never arrived" and
    /// the assertion would hold for a drain that does nothing at all.
    #[test]
    fn ordinary_output_reaches_the_buffer_and_no_record() {
        let (directory, subscriber) = capture("drain-quiet");
        let log = SidecarLog::default();
        with_default(subscriber, || {
            for i in 0..50 {
                let done = heard(Heard::Line(format!("l{i}")), &log, "sidecar_started");
                assert!(!done, "only the exit ends the loop");
            }
        });

        assert_eq!(len(&log), 50, "the control: the lines really are buffered");
        assert_eq!(tail(&log, 1), vec!["l49"]);
        assert_eq!(
            written(&directory.0),
            "",
            "the Agent's own output must not become Desktop's records"
        );
    }

    /// The exit is one record, and it carries the tail (ruling 五).
    ///
    /// `report_exit` is the only place those twenty lines reach the file, which
    /// is what makes "one exit, one record" checkable by counting lines.
    #[test]
    fn the_exit_is_one_record_carrying_the_tail() {
        let (directory, subscriber) = capture("drain-exit");
        let log = SidecarLog::default();
        with_default(subscriber, || {
            for i in 0..50 {
                heard(Heard::Line(format!("l{i}")), &log, "sidecar_started");
            }
            assert!(heard(
                Heard::Exited {
                    code: Some(1),
                    signal: None
                },
                &log,
                "sidecar_started"
            ));
        });

        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "exactly one record: {text}");
        assert!(text.contains("[INFO] agent.supervisor"), "{text}");
        assert!(
            text.contains("缓冲共 50 行，末 20 行输出："),
            "the held and shown counts are both there: {text}"
        );
        // The tail is the newest twenty, and the whole report is one line: the
        // newlines became escapes rather than ending the record early.
        assert!(text.contains("| l30"), "{text}");
        assert!(text.contains("| l49"), "{text}");
        assert!(!text.contains("| l29"), "the window is twenty: {text}");
    }

    /// A read failure is logged as well as buffered — it is Desktop's problem,
    /// and the process it failed on may still be running.
    #[test]
    fn a_read_failure_is_buffered_and_logged() {
        let (directory, subscriber) = capture("drain-read-failure");
        let log = SidecarLog::default();
        with_default(subscriber, || {
            assert!(!heard(
                Heard::Failure("channel closed".to_string()),
                &log,
                "sidecar_started"
            ));
        });

        assert_eq!(
            tail(&log, 1),
            vec!["[读取 sidecar 输出失败] channel closed"]
        );
        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "one failure, one record: {text}");
        assert!(text.contains("[WARN] agent.supervisor"), "{text}");
        assert!(text.contains("channel closed"), "{text}");
    }

    /// The classifier ignores what the module does not act on.
    ///
    /// `CommandEvent` cannot be built from here (`#[non_exhaustive]`), so the
    /// mapping itself is out of a test's reach by construction: what can be
    /// pinned is that the two directions this module does act on are the two it
    /// names, and that the loop's `None` arm stays a no-op.
    #[test]
    fn the_two_directions_are_named_and_the_rest_is_ignored() {
        // `Heard` is what `classify` produces; this is its wording, so a
        // renamed variant cannot silently become a different case.
        let line = Heard::Line("out".to_string());
        assert!(matches!(line, Heard::Line(ref text) if text == "out"));
        let failure = Heard::Failure("why".to_string());
        assert!(matches!(failure, Heard::Failure(ref text) if text == "why"));
        let exited = Heard::Exited {
            code: None,
            signal: Some(9),
        };
        assert!(matches!(
            exited,
            Heard::Exited {
                code: None,
                signal: Some(9)
            }
        ));
    }

    /// The exit report and the error suffix show the same window, so the plan's
    /// "last 20 lines" is one number in one place.
    ///
    /// The comparisons below are against `TAIL_ON_EXIT`, so they cannot detect a
    /// changed number on their own — hence the literal pins first. Without them,
    /// editing the constant to 10 would leave this test green.
    #[test]
    fn the_reported_window_is_the_same_one_either_way() {
        assert_eq!(
            CAPACITY, 200,
            "the plan fixes this capacity; changing it is a deliberate edit"
        );
        assert_eq!(
            TAIL_ON_EXIT, 20,
            "the plan fixes this tail; changing it is a deliberate edit"
        );

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
