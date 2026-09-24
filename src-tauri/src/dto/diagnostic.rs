//! What a diagnostic export tells the page.
//!
//! The page's job is to say "it is here, this big, this digest, and here is what
//! is in it" — so this is what the report carries, and nothing else. In
//! particular the report does **not** repeat the bundle's contents: the summary
//! and the manifest are inside the archive, which is where a person is going to
//! read them, and duplicating them on the wire would give the page a second copy
//! to fall out of step with the file.
//!
//! The one part that is repeated is the **entry list**, name and size and whether
//! it is a window — because that is what makes the export auditable without
//! opening anything, and because it is what the page must show before a person
//! attaches a file to a message. Same reasoning as T-06's `kept` list: an action's
//! report is where a person learns what it actually did.
//!
//! The **path** is on the wire, unlike the cleanup commands' reports: the artifact
//! is a file the person has to find, and the alternative — the page constructing
//! a path from parts it knows — is how two components end up disagreeing about
//! where something is. It is a path this process chose, not one a caller supplied.

use crate::diagnostic::{Failures, Logs, Written};
use serde::Serialize;

/// One file inside the archive, as the page lists it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticEntryFact {
    /// `logs/<source>/<file name>` — the name as it is inside the archive.
    pub name: String,
    /// The bytes this entry occupies in the archive.
    pub bytes: u64,
    /// True when a cap stopped the read, so this is the end of the file rather
    /// than all of it. The page says 「尾部」 for these and nothing for the rest.
    pub truncated: bool,
}

/// One file that is **not** in the archive, and why.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticOmissionFact {
    pub name: String,
    /// `name_too_long`, `unreadable` or `archive_full` (`OmittedReason::as_str`).
    pub reason: String,
}

/// The result of one export.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticReport {
    /// The archive, absolute. Never overwritten: a second export at the same
    /// second lands beside this one with `-2` on the end.
    pub path: String,
    /// `sha256sum`-format digest file beside it, `<archive>.sha256`.
    pub checksum_path: String,
    pub bytes: u64,
    /// Hex, lowercase, 64 characters.
    pub sha256: String,
    /// The moment the bundle was made, in the same shape a log line carries.
    pub created_at: String,
    pub entries: Vec<DiagnosticEntryFact>,
    pub omitted: Vec<DiagnosticOmissionFact>,
    /// `answered` or `unreachable`. An unreachable Agent still produces a bundle,
    /// so this is a fact about the export rather than a failure of it.
    pub agent_state: String,
    /// How many warn-and-louder records of the Agent's task log went in.
    pub failed_records: usize,
}

/// Assemble the report from what was written and what went in.
///
/// Takes the writer's own [`Written`] rather than the path it was asked for: the
/// name that survived the collision attempts is not the name that was requested,
/// and reporting the requested one would send a person to a file that is not
/// theirs.
pub fn report(
    written: &Written,
    created_at: &str,
    logs: &Logs,
    failures: &Failures,
    agent_state: &str,
) -> DiagnosticReport {
    DiagnosticReport {
        path: written.path.display().to_string(),
        checksum_path: written.checksum_path.display().to_string(),
        bytes: written.bytes,
        sha256: written.sha256.clone(),
        created_at: created_at.to_string(),
        entries: logs
            .entries
            .iter()
            .map(|entry| DiagnosticEntryFact {
                name: entry.name.clone(),
                bytes: entry.text.len() as u64,
                truncated: entry.truncated,
            })
            .collect(),
        omitted: logs
            .omitted
            .iter()
            .map(|omission| DiagnosticOmissionFact {
                name: omission.name.clone(),
                reason: omission.reason.as_str().to_string(),
            })
            .collect(),
        agent_state: agent_state.to_string(),
        failed_records: failures.records.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::OmittedReason;
    use std::path::PathBuf;

    fn written() -> Written {
        Written {
            path: PathBuf::from(
                "/home/operator/Downloads/wt-media-diagnostic-20260924-225501.tar.gz",
            ),
            checksum_path: PathBuf::from(
                "/home/operator/Downloads/wt-media-diagnostic-20260924-225501.tar.gz.sha256",
            ),
            bytes: 4096,
            sha256: "a".repeat(64),
        }
    }

    /// The exact key set, pinned.
    ///
    /// The page reads these by name, so a rename is a behaviour change rather
    /// than a refactor — the same rule `dto/mod.rs` states for the rest of the
    /// wire. Asserted as a set rather than by reading one field, so a key that
    /// was dropped is as visible as one that was renamed.
    #[test]
    fn the_wire_key_sets_are_pinned() {
        let report = report(
            &written(),
            "2026-09-24T22:55:01",
            &Logs {
                entries: vec![crate::diagnostic::LogEntry {
                    name: "logs/desktop/desktop.log".to_string(),
                    text: "hello\n".to_string(),
                    // Deliberately not the entry's own byte count: the wire
                    // field must be the bytes that went *in*, and a fixture
                    // where the two agree cannot tell those apart.
                    source_bytes: 4096,
                    modified: None,
                    truncated: true,
                }],
                omitted: vec![
                    crate::diagnostic::Omission {
                        name: "logs/agent/unreadable-tree".to_string(),
                        reason: OmittedReason::Unreadable,
                    },
                    crate::diagnostic::Omission {
                        name: "logs/desktop/a-very-long-name.log".to_string(),
                        reason: OmittedReason::NameTooLong,
                    },
                ],
            },
            &Failures {
                source: Some("logs/agent/task.log".to_string()),
                records: vec!["2026-09-24T22:00:00 [ERROR] task.run: nope".to_string()],
                truncated: false,
            },
            "answered",
        );

        let value: serde_json::Value = serde_json::to_value(&report).expect("serializable");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "agent_state",
                "bytes",
                "checksum_path",
                "created_at",
                "entries",
                "failed_records",
                "omitted",
                "path",
                "sha256",
            ]
        );

        let mut entry_keys: Vec<&str> = value["entries"][0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        entry_keys.sort_unstable();
        assert_eq!(entry_keys, vec!["bytes", "name", "truncated"]);

        assert_eq!(
            value["omitted"][1]["reason"], "name_too_long",
            "the other reason a person will see, spelled here like the first"
        );

        let mut omission_keys: Vec<&str> = value["omitted"][0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        omission_keys.sort_unstable();
        assert_eq!(omission_keys, vec!["name", "reason"]);

        // The values the page shows, read from the same document the page would.
        assert_eq!(value["entries"][0]["name"], "logs/desktop/desktop.log");
        assert_eq!(value["entries"][0]["truncated"], true);
        assert_eq!(
            value["entries"][0]["bytes"], 6,
            "the entry's bytes are what went into the archive, not the source file's size"
        );
        assert_eq!(
            value["bytes"], 4096,
            "the archive's size is the writer's own"
        );
        assert_eq!(value["sha256"], "a".repeat(64), "the digest passes through");
        assert!(
            value["path"]
                .as_str()
                .expect("a path")
                .ends_with("wt-media-diagnostic-20260924-225501.tar.gz"),
            "the name reported is the one written: {}",
            value["path"]
        );
        assert_eq!(value["omitted"][0]["reason"], "unreadable");
        assert_eq!(value["failed_records"], 1);
        assert_eq!(report.created_at, "2026-09-24T22:55:01");
    }

    /// An export that carried nothing still carries every key.
    ///
    /// The empty case is the one a page is most likely to render wrongly — a
    /// missing `omitted` reads as "nothing was left out", which is the opposite
    /// of what an empty list means here only if the key is absent rather than
    /// empty. T-06 learned this on `failures`; this is the same rule one command
    /// later, asserted rather than inherited.
    #[test]
    fn an_empty_report_still_carries_every_key() {
        let value: serde_json::Value = serde_json::to_value(report(
            &written(),
            "2026-09-24T22:55:01",
            &Logs::default(),
            &Failures::default(),
            "unreachable",
        ))
        .expect("serializable");

        assert_eq!(value["entries"], serde_json::json!([]));
        assert_eq!(value["omitted"], serde_json::json!([]));
        assert_eq!(value["failed_records"], 0);
        assert_eq!(value["agent_state"], "unreachable");
        assert_eq!(value.as_object().unwrap().len(), 9);
    }
}
