//! What the 本机设置 page is told after a cleanup ran.
//!
//! The shape follows from the hardest question a cleanup report has to answer:
//! *did it work*. A number alone cannot — "已释放 0 字节" is the same string for
//! "there was nothing to delete", "everything went", and "everything failed" —
//! so the report carries the counts, what was deliberately left with its reason,
//! and what could not be removed at all. The page can then say which of the three
//! happened instead of showing a plausible zero.
//!
//! `freed_bytes` is the sum of the sizes of the files this call removed, never a
//! difference of free-space readings: the volume moves under every other process
//! on the machine, so such a difference can be negative. `cleanup`'s header has
//! the long version; the short one is that a person is being told what a button
//! achieved.
//!
//! Field names are load-bearing — the Vue layer reads them by name — and the key
//! sets are pinned by the tests at the bottom of this module.

use serde::Serialize;

/// One cleanup: what went, what was kept, and what could not be done.
#[derive(Clone, Debug, Serialize)]
pub struct CleanupReport {
    /// What was cleaned: `"cache"`, `"desktop"` or `"agent"`.
    ///
    /// The cache is one thing and each log tree is another, so a report about one
    /// of them is a [`CleanupReport`] rather than a list of entries — the page
    /// presses one button and gets one answer.
    pub label: String,
    /// The directory that was cleaned. Shown, because "cleaned the wrong
    /// directory" is a mistake a count would hide.
    pub directory: String,
    /// The bytes of the files that were actually removed.
    pub freed_bytes: u64,
    pub files_removed: usize,
    /// Subdirectories that were removed. Always 0 for a log tree: the tree
    /// itself is never removed.
    pub directories_removed: usize,
    /// Entries left in place on purpose, with the reason. Not a failure.
    pub kept: Vec<KeptFileFact>,
    /// Entries that were supposed to go and did not. Not empty means the cleanup
    /// is incomplete, and the counts above describe only the part that happened.
    pub failures: Vec<CleanupFailureFact>,
}

/// One entry a cleanup deliberately left alone.
#[derive(Clone, Debug, Serialize)]
pub struct KeptFileFact {
    pub name: String,
    /// `"live"`, `"unrecognised"`, `"symlink"` or `"special"` —
    /// [`crate::cleanup::KeptReason::as_str`].
    pub reason: String,
}

/// One entry that could not be removed.
#[derive(Clone, Debug, Serialize)]
pub struct CleanupFailureFact {
    pub path: String,
    /// The operating system's own words. Passed through rather than summarised:
    /// "Permission denied" and "Read-only file system" call for different things
    /// from a person, and this is the only place either appears.
    pub reason: String,
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

    fn report() -> CleanupReport {
        CleanupReport {
            label: "cache".to_string(),
            directory: "/home/operator/Library/Caches/WTMedia/Desktop".to_string(),
            freed_bytes: 1024,
            files_removed: 2,
            directories_removed: 1,
            kept: vec![KeptFileFact {
                name: "elsewhere.bin".to_string(),
                reason: "symlink".to_string(),
            }],
            failures: vec![CleanupFailureFact {
                path: "/home/operator/Library/Caches/WTMedia/Desktop/locked.bin".to_string(),
                reason: "Permission denied (os error 13)".to_string(),
            }],
        }
    }

    /// The key sets, pinned. A field renamed or added tomorrow fails here, which
    /// is the intended cost: the Vue layer reads these by name.
    #[test]
    fn the_wire_key_sets_are_pinned() {
        assert_eq!(
            keys_of(&report()),
            [
                "directories_removed",
                "directory",
                "failures",
                "files_removed",
                "freed_bytes",
                "kept",
                "label",
            ]
        );
        let json = serde_json::to_value(report()).expect("serialize");
        let mut kept_keys: Vec<&str> = json["kept"][0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        kept_keys.sort_unstable();
        assert_eq!(kept_keys, ["name", "reason"]);
        let mut failure_keys: Vec<&str> = json["failures"][0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        failure_keys.sort_unstable();
        assert_eq!(failure_keys, ["path", "reason"]);
    }

    /// The empty report is a real answer, not a missing one: every key is present
    /// with a zero or an empty list, so a page cannot tell "nothing to do" apart
    /// from "the response was not what I expected" by accident.
    #[test]
    fn an_empty_report_still_carries_every_key() {
        let empty = CleanupReport {
            label: "agent".to_string(),
            directory: "/home/operator/Library/Logs/WTMedia/Agent".to_string(),
            freed_bytes: 0,
            files_removed: 0,
            directories_removed: 0,
            kept: Vec::new(),
            failures: Vec::new(),
        };
        let json = serde_json::to_value(&empty).expect("serialize");

        assert_eq!(json["freed_bytes"], 0);
        assert_eq!(json["files_removed"], 0);
        assert!(
            json["kept"].as_array().is_some_and(Vec::is_empty),
            "an empty kept is [] rather than null: {json}"
        );
        assert!(json["failures"].as_array().is_some_and(Vec::is_empty));
    }
}
