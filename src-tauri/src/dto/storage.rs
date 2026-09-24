//! What the 本机设置 page is told about the machine's storage and its logs.
//!
//! Three read-only answers, and the shape of each is decided by the same rule the
//! commands follow: **a measurement that could not be taken is not in the
//! answer**. There is no `Option<u64>` here that a page could render as `0 MB`,
//! and no `[]` that could stand in for "I could not look". A reading that fails
//! fails the whole command (AC-06, the milestone's 读取失败必须是错误而非 0 MB), so
//! every number in these types is one that was actually measured.
//!
//! Field names are load-bearing: the Vue layer reads them by name, and the key
//! sets are pinned by the tests at the bottom of this module rather than left to
//! be discovered by a page that silently shows `undefined`.

use serde::Serialize;

/// Free space, what the cache occupies, and what each log tree occupies.
#[derive(Clone, Debug, Serialize)]
pub struct StorageUsage {
    /// Bytes available on the volume that would hold the data root.
    ///
    /// The volume, not the directory: the reading is taken from the nearest
    /// existing ancestor so that a first launch — whose data root has not been
    /// created yet — still gets a number (`storage::available_bytes_for`).
    pub available_bytes: u64,
    /// The bytes under the cache root. Zero means the cache is empty, which is
    /// what an absent cache root means too.
    pub cache_bytes: u64,
    /// One entry per component's log tree, in the order the page shows them.
    pub logs: Vec<LogTreeUsage>,
}

/// One component's log tree, measured.
#[derive(Clone, Debug, Serialize)]
pub struct LogTreeUsage {
    /// `"desktop"` or `"agent"` — [`crate::logging::reader::Source::label`].
    pub source: String,
    /// The directory that was measured. Shown, because "which directory is the
    /// Agent writing to" is the question an empty tree raises, and the answer is
    /// not always the one in this document.
    pub directory: String,
    /// The sum of the files in [`LogTreeUsage::files`] — the same files the
    /// listing command returns, so a total and a list can never disagree.
    pub bytes: u64,
    /// How many files that sum is over.
    pub files: usize,
}

/// Every log file this launch can see, both components.
#[derive(Clone, Debug, Serialize)]
pub struct LogListing {
    pub trees: Vec<LogTreeFiles>,
}

/// One component's log files.
#[derive(Clone, Debug, Serialize)]
pub struct LogTreeFiles {
    /// `"desktop"` or `"agent"`.
    pub source: String,
    /// The directory that was listed, whether or not anything was in it. An
    /// absent directory lists as empty rather than as an error — an Agent that
    /// never ran has no log directory, and that is a fact, not a failure — so
    /// the path is what tells the page whether it is looking in the right place.
    pub directory: String,
    /// Ordered by the reader: the live file, then archives newest first, then
    /// anything it does not recognise.
    pub files: Vec<LogFileFact>,
}

/// One log file.
#[derive(Clone, Debug, Serialize)]
pub struct LogFileFact {
    /// The file name, as it is on disk. This is the handle the tail command
    /// takes, and the listing is what makes it a legal one.
    pub name: String,
    /// `"live"`, `"archive"` or `"other"`.
    pub kind: String,
    pub bytes: u64,
    /// Seconds since the epoch, or `null` when the filesystem would not say.
    ///
    /// A number rather than a formatted string: the page renders it in the
    /// machine's own timezone (the user's ruling), which is a thing the WebView
    /// can do and this side should not.
    pub modified_seconds: Option<u64>,
}

/// The end of one log file, as asked for.
#[derive(Clone, Debug, Serialize)]
pub struct LogTailResult {
    pub source: String,
    pub name: String,
    /// The full path that was read. Repeated from the listing on purpose: a page
    /// that showed only the tail could be showing a file from another directory
    /// without saying so.
    pub path: String,
    /// The level filter this answer is under, in the page's own spelling, or
    /// `null` for no filter.
    pub min_level: Option<String>,
    /// How many lines were read before filtering, so the page can say that a
    /// filter is hiding something rather than looking empty.
    pub lines_read: usize,
    /// How many of them are in [`LogTailResult::lines`].
    pub lines_shown: usize,
    /// True when the byte ceiling — not the start of the file — is what ended the
    /// read, so the page can say the view is a window.
    pub truncated: bool,
    pub lines: Vec<LogEntryFact>,
}

/// One line of a log, with the level that applies to it.
#[derive(Clone, Debug, Serialize)]
pub struct LogEntryFact {
    /// `"trace"`…`"error"`, or `null` for a line before the first record that
    /// carried a level.
    pub level: Option<String>,
    /// True when this line continues the record above it rather than being a
    /// record of its own, so the page can indent it.
    pub continues: bool,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_of<T: Serialize>(value: &T) -> Vec<String> {
        let json = serde_json::to_value(value).expect("the DTO must serialize");
        let mut keys: Vec<String> = json
            .as_object()
            .expect("the DTO must serialize to an object")
            .keys()
            .cloned()
            .collect();
        keys.sort_unstable();
        keys
    }

    fn listing() -> LogListing {
        LogListing {
            trees: vec![LogTreeFiles {
                source: "desktop".to_string(),
                directory: "/home/operator/Library/Logs/WTMedia/Desktop".to_string(),
                files: vec![LogFileFact {
                    name: "desktop.log".to_string(),
                    kind: "live".to_string(),
                    bytes: 12,
                    modified_seconds: Some(1_774_000_000),
                }],
            }],
        }
    }

    fn tail() -> LogTailResult {
        LogTailResult {
            source: "agent".to_string(),
            name: "agent.log".to_string(),
            path: "/home/operator/Library/Logs/WTMedia/Agent/agent.log".to_string(),
            min_level: Some("warn".to_string()),
            lines_read: 3,
            lines_shown: 1,
            truncated: false,
            lines: vec![LogEntryFact {
                level: Some("warn".to_string()),
                continues: false,
                text: "a record".to_string(),
            }],
        }
    }

    /// The key sets, pinned. A field renamed or added tomorrow fails here, which
    /// is the intended cost: the Vue layer reads these by name, so a change to
    /// them is a change to the page.
    #[test]
    fn the_wire_key_sets_are_pinned() {
        assert_eq!(
            keys_of(&StorageUsage {
                available_bytes: 1,
                cache_bytes: 2,
                logs: Vec::new(),
            }),
            ["available_bytes", "cache_bytes", "logs"]
        );
        assert_eq!(
            keys_of(&LogTreeUsage {
                source: "desktop".into(),
                directory: "/d".into(),
                bytes: 1,
                files: 2,
            }),
            ["bytes", "directory", "files", "source"]
        );
        assert_eq!(keys_of(&listing()), ["trees"]);
        let tree = serde_json::to_value(listing()).expect("serialize");
        let mut tree_keys: Vec<&str> = tree["trees"][0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        tree_keys.sort_unstable();
        assert_eq!(tree_keys, ["directory", "files", "source"]);
        let mut file_keys: Vec<&str> = tree["trees"][0]["files"][0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        file_keys.sort_unstable();
        assert_eq!(file_keys, ["bytes", "kind", "modified_seconds", "name"]);

        assert_eq!(
            keys_of(&tail()),
            [
                "lines",
                "lines_read",
                "lines_shown",
                "min_level",
                "name",
                "path",
                "source",
                "truncated",
            ]
        );
        let tail_json = serde_json::to_value(tail()).expect("serialize");
        let mut entry_keys: Vec<&str> = tail_json["lines"][0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        entry_keys.sort_unstable();
        assert_eq!(entry_keys, ["continues", "level", "text"]);
    }

    /// The two enumerations cross the wire in the spelling the reader defined,
    /// not in a second spelling invented here — and `null` is a real value for
    /// both optional fields rather than an omitted key.
    #[test]
    fn the_enumerations_and_optionals_cross_the_wire_as_the_reader_spells_them() {
        use crate::logging::reader::{FileKind, Level, Source};

        assert_eq!(Source::Desktop.label(), "desktop");
        assert_eq!(FileKind::Live.as_str(), "live");
        assert_eq!(FileKind::Archive.as_str(), "archive");
        assert_eq!(FileKind::Other.as_str(), "other");
        for (level, spelling) in [
            (Level::Trace, "trace"),
            (Level::Debug, "debug"),
            (Level::Info, "info"),
            (Level::Warn, "warn"),
            (Level::Error, "error"),
        ] {
            assert_eq!(level.as_str(), spelling);
        }

        let mut without_filter = tail();
        without_filter.min_level = None;
        without_filter.lines[0].level = None;
        let json = serde_json::to_value(&without_filter).expect("serialize");
        assert!(
            json["min_level"].is_null(),
            "a missing filter is null: {json}"
        );
        assert!(
            json["lines"][0]["level"].is_null(),
            "an unclassified line is null: {json}"
        );

        let mut no_mtime = listing();
        no_mtime.trees[0].files[0].modified_seconds = None;
        let json = serde_json::to_value(&no_mtime).expect("serialize");
        assert!(json["trees"][0]["files"][0]["modified_seconds"].is_null());
    }
}
