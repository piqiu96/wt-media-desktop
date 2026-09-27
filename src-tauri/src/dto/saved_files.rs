//! What the 下载中心 and the 本机设置 page are told about this machine's
//! downloaded files.
//!
//! Two questions, and the shapes differ because the questions do. 「这个文件还在
//! 不在」 is answered one name at a time ([`SavedFileState`]) and has a *third*
//! answer besides yes and no — 「没人查过」 — which is why the page's own
//! `savedFileStates` turns an empty table into `unknown` rather than `absent`.
//! 「搬过去会怎样」 is answered for a whole selection at once ([`MigrationPlan`]),
//! because the space it needs is the sum over all of them and a per-file answer
//! could not carry that.
//!
//! ## No path ever crosses this boundary
//!
//! Every `directory` here is a **directory** — one the machine is known to write
//! downloads into — and never a file's own path. [`MigrationFailure`] carries a
//! `name` for exactly that reason, where `dto::cleanup`'s failure carries a
//! `path`: a cleanup names files the operator can see in a file manager, and this
//! family is the one CHG-061 §4 keeps paths out of. `local-settings-view.js` reads
//! `failure.name`; a reused `CleanupFailureFact` would have sent a path and the
//! page would have shown an empty string.
//!
//! ## A size that was not measured is `null`
//!
//! [`SavedFileFact::bytes`] is an `Option`, and the page keeps it nullable
//! (`asNullableCount`). `needed_bytes` is a sum of them, and a missing size
//! silently worth `0` is how 「需要 12 MB、剩余 8 MB」 passes a space check on a
//! volume that cannot hold the file.
//!
//! Field names are load-bearing — the Vue layer reads them by name — and the key
//! sets are pinned by the tests at the bottom of this module.

use crate::dto::KeptFileFact;
use serde::Serialize;

/// One saved file, as 「它现在在哪儿」 answers it.
///
/// `directory` is `None` for a name that is in no known directory — the page
/// reads that as `absent`, so an entry that is *missing* and an entry that is
/// *not there but the machine could not look* are told apart by `unreadable` in
/// the plan, not here. A name the page asked about is always present in the
/// answer, even when nothing was found: an omitted entry would read as 「没人查
/// 过」, which is the one thing this command exists to distinguish.
#[derive(Clone, Debug, Serialize)]
pub struct SavedFileState {
    pub name: String,
    /// The directory a regular file of that name is in, or `None`.
    pub directory: Option<String>,
    /// Whether that directory is the one new downloads go to. `false` when there
    /// is no file and when nothing is chosen.
    pub current: bool,
    /// Its size, or `None` when it could not be measured.
    pub bytes: Option<u64>,
}

/// One saved file as the migration plan counts it.
#[derive(Clone, Debug, Serialize)]
pub struct SavedFileFact {
    pub name: String,
    /// The directory it is in. **Empty** for [`MigrationPlan::missing`], which
    /// carries a record rather than a bare name because the dialog shows a size
    /// beside every row: a name with no directory is what 「已经不在」 looks like
    /// in this shape, and the page's `normalizeSavedFile` renders it as such.
    pub directory: String,
    pub bytes: Option<u64>,
}

/// A known save directory that could not be listed, and why.
#[derive(Clone, Debug, Serialize)]
pub struct UnreadableDirFact {
    pub directory: String,
    /// The operating system's own words, or 「现在不是一个目录」 — the **raw**
    /// phrase, not `saved_files::Unreadable::sentence`.
    ///
    /// `local-settings-view.js` renders `directory（reason）`, so a sentence that
    /// already contains the directory would print it twice, and the page's own
    /// wording is the one that has to be editable without a Rust change.
    pub reason: String,
}

/// What moving the saved files would do, before anything is touched.
///
/// The whole selection in one answer, because the two numbers a person acts on —
/// 「需要多少」 and 「还剩多少」 — are sums that no single file's row could carry.
#[derive(Clone, Debug, Serialize)]
pub struct MigrationPlan {
    /// Where the files would go: the directory now chosen.
    pub to: String,
    pub free_bytes: u64,
    /// The sum of the sizes the move would carry. Under-counts only what could not
    /// be measured, whose own row carries `bytes: null`.
    pub needed_bytes: u64,
    /// Regular files in an older directory, with a free name in the target.
    pub movable: Vec<SavedFileFact>,
    /// Already in the target: nothing to do.
    pub already_there: Vec<SavedFileFact>,
    /// Names no known directory holds as a downloadable file.
    pub missing: Vec<SavedFileFact>,
    /// Directories that could not be listed. Not a failure of the plan: the
    /// person is told which names were not looked for, and reads 「未找到」 rather
    /// than 「已不存在」 beside them — the distinction `missingStatusLabel` draws.
    pub unreadable: Vec<UnreadableDirFact>,
}

/// What one move or one delete did.
///
/// One shape for both, because `local-settings-view.js` has one
/// `describeMigration` for both: a move fills `moved` and a delete fills
/// `deleted`, and the page names `directory` only for a move — a deletion has no
/// destination, and a directory printed beside one would read as 「these were
/// deleted from there」.
#[derive(Clone, Debug, Serialize)]
pub struct MigrationReport {
    pub directory: String,
    pub moved: Vec<String>,
    pub deleted: Vec<String>,
    /// Names in none of the readable directories.
    pub missing: Vec<String>,
    /// Entries left where they are on purpose, with the reason: `"same_directory"`,
    /// `"target_exists"`, `"symlink"` or `"not_a_regular_file"` —
    /// [`crate::saved_files::KeptReason::as_str`].
    ///
    /// `KeptFileFact` is shared with `cleanup` rather than copied: it is already
    /// `{name, reason}` and this family's `kept` is the same fact, so a second
    /// struct with the same two fields would be a second thing for the page's
    /// `KEPT_REASON_LABELS` to keep in step.
    pub kept: Vec<KeptFileFact>,
    pub failures: Vec<MigrationFailure>,
}

/// One name the move or the delete could not carry out.
#[derive(Clone, Debug, Serialize)]
pub struct MigrationFailure {
    /// The **name**, never a path. See this module's header.
    pub name: String,
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

    fn fact(name: &str) -> SavedFileFact {
        SavedFileFact {
            name: name.to_string(),
            directory: "/home/operator/Movies".to_string(),
            bytes: Some(10),
        }
    }

    fn plan() -> MigrationPlan {
        MigrationPlan {
            to: "/home/operator/Movies".to_string(),
            free_bytes: 1_024,
            needed_bytes: 10,
            movable: vec![fact("a.mp4")],
            already_there: vec![fact("b.mp4")],
            missing: vec![SavedFileFact {
                name: "gone.mp4".to_string(),
                directory: String::new(),
                bytes: None,
            }],
            unreadable: vec![UnreadableDirFact {
                directory: "/Volumes/Offline".to_string(),
                reason: "No such file or directory (os error 2)".to_string(),
            }],
        }
    }

    fn report() -> MigrationReport {
        MigrationReport {
            directory: "/home/operator/Movies".to_string(),
            moved: vec!["a.mp4".to_string()],
            deleted: Vec::new(),
            missing: vec!["gone.mp4".to_string()],
            kept: vec![KeptFileFact {
                name: "b.mp4".to_string(),
                reason: "target_exists".to_string(),
            }],
            failures: vec![MigrationFailure {
                name: "c.mp4".to_string(),
                reason: "Permission denied (os error 13)".to_string(),
            }],
        }
    }

    /// The key sets, pinned. A field renamed or added tomorrow fails here, which
    /// is the intended cost: the Vue layer reads these by name.
    #[test]
    fn the_wire_key_sets_are_pinned() {
        assert_eq!(
            keys_of(&plan()),
            [
                "already_there",
                "free_bytes",
                "missing",
                "movable",
                "needed_bytes",
                "to",
                "unreadable",
            ]
        );
        assert_eq!(
            keys_of(&report()),
            [
                "deleted",
                "directory",
                "failures",
                "kept",
                "missing",
                "moved"
            ]
        );
        assert_eq!(
            keys_of(&SavedFileState {
                name: "a.mp4".to_string(),
                directory: None,
                current: false,
                bytes: None,
            }),
            ["bytes", "current", "directory", "name"]
        );
        assert_eq!(keys_of(&fact("a.mp4")), ["bytes", "directory", "name"]);
        assert_eq!(
            keys_of(&UnreadableDirFact {
                directory: "/Volumes/Offline".to_string(),
                reason: "…".to_string(),
            }),
            ["directory", "reason"]
        );
        assert_eq!(
            keys_of(&MigrationFailure {
                name: "c.mp4".to_string(),
                reason: "…".to_string(),
            }),
            ["name", "reason"]
        );
    }

    /// A size that was not measured is `null` on the wire, and a directory that
    /// does not exist is `null` too — neither becomes an empty string or a `0`,
    /// because the page tells 「没测到」 apart from 「0 字节」 by exactly that.
    #[test]
    fn an_unmeasured_size_stays_null_rather_than_becoming_zero() {
        let json = serde_json::to_value(SavedFileState {
            name: "a.mp4".to_string(),
            directory: None,
            current: false,
            bytes: None,
        })
        .expect("serialize");

        assert!(json["bytes"].is_null(), "{json}");
        assert!(json["directory"].is_null(), "{json}");
    }

    /// An empty plan and an empty report still carry every key.
    ///
    /// The same claim `dto::cleanup` pins: a page must not tell 「没什么可搬的」
    /// apart from 「响应不是我以为的形状」 by accident.
    #[test]
    fn an_empty_answer_still_carries_every_key() {
        let empty = MigrationPlan {
            to: String::new(),
            free_bytes: 0,
            needed_bytes: 0,
            movable: Vec::new(),
            already_there: Vec::new(),
            missing: Vec::new(),
            unreadable: Vec::new(),
        };
        let json = serde_json::to_value(&empty).expect("serialize");

        for key in ["movable", "already_there", "missing", "unreadable"] {
            assert!(
                json[key].as_array().is_some_and(Vec::is_empty),
                "{key} is [] rather than null: {json}"
            );
        }

        let empty = MigrationReport {
            directory: String::new(),
            moved: Vec::new(),
            deleted: Vec::new(),
            missing: Vec::new(),
            kept: Vec::new(),
            failures: Vec::new(),
        };
        let json = serde_json::to_value(&empty).expect("serialize");
        assert!(json["kept"].as_array().is_some_and(Vec::is_empty), "{json}");
        assert!(json["failures"].as_array().is_some_and(Vec::is_empty));
    }

    /// The `missing` rows are records with an empty directory and a null size —
    /// the shape `normalizeSavedFile` is applied to, in the committed mock's
    /// spelling.
    ///
    /// Worth its own test because the shape is not obvious from the Rust side: the
    /// core's `Plan::missing` is a list of names, and this conversion is the only
    /// place the two meet.
    #[test]
    fn a_missing_row_is_a_record_with_no_directory() {
        let json = serde_json::to_value(&plan()).expect("serialize");

        assert_eq!(json["missing"][0]["name"], "gone.mp4");
        assert_eq!(json["missing"][0]["directory"], "");
        assert!(json["missing"][0]["bytes"].is_null(), "{json}");
    }
}
