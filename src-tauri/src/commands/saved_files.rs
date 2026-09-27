//! The four commands the 下载中心 and the 本机设置 page use to ask where this
//! machine's downloaded files are, and to move or delete some of them.
//!
//! ## Every one of them takes names, never a path
//!
//! The same rule `commands::downloads` and `commands::cleanup` follow, and here it
//! is load-bearing in a second way: a move and a delete **write and destroy**. A
//! command that took a path would be 「move/delete anything this process can reach」,
//! reachable from the page; a name is matched against the directories the app
//! itself resolved, which is what makes the reachable set a set the operator chose.
//! `saved_files::file_name_of` is where that is enforced, and it is enforced once,
//! before any directory is touched.
//!
//! ## The search space is the settings file's answer, not this module's
//!
//! `settings::save_places()` reads the one file and answers both questions at once
//! — 「新文件写到哪儿」 (`chosen`) and 「这个文件可能在哪些地方」 (`search`) — because
//! they are two answers about the same file and a second read could answer about a
//! different moment (the operator may edit `settings.toml` by hand while the app is
//! running). The split below is the same one `commands::settings` draws: the rules
//! take a `SavePlaces` a test can build, and the commands are the two lines that
//! resolve the real one.
//!
//! ## Why these are `async`
//!
//! Not for concurrency. A non-async `#[tauri::command]` runs on the main thread,
//! and this family walks directories that can be on a volume that has stopped
//! answering and copies files that can be hundreds of megabytes: a `rename` across
//! two volumes is a full copy, and the window would be frozen for its duration. An
//! `async` command is spawned onto the runtime instead, which is the same reason
//! `local_pick_save_directory` is `async`.
//!
//! ## Free space is measured, and a failed measurement is an `Err`
//!
//! [`storage::available_bytes_for`] is `statvfs` on the target's volume. A plan
//! built from `0` when that failed would tell the operator 「需要 230 MB、剩余
//! 0 字节」 — or, worse, the reverse: a plan that read the failure as "plenty of
//! room" and offered a move that cannot finish. Neither is a sentence this module
//! is entitled to invent, so the failure is the command's failure.

use crate::dto::{
    KeptFileFact, MigrationFailure, MigrationPlan, MigrationReport, SavedFileFact, SavedFileState,
    UnreadableDirFact,
};
use crate::saved_files::{self, Plan, Presence, Report};
use crate::storage;
use std::path::{Path, PathBuf};

use super::settings::{self, SavePlaces};

/// The directory new downloads go to, or why there is none.
///
/// A move needs one and so does the plan shown before it — both write into the
/// target — so both refuse here. `delete` does not: deleting does not write into
/// a directory, so a machine whose choice was cleared can still delete.
///
/// The sentence is the one `saved_files::find_in_known` uses for the same
/// situation, so a person who has never chosen reads one wording of 「还没有选择
/// 下载保存位置」 wherever they meet it.
fn target_of(places: &SavePlaces) -> Result<PathBuf, String> {
    places
        .chosen
        .clone()
        .ok_or_else(|| "还没有选择下载保存位置".to_string())
}

/// Free space on the target's volume, or why it could not be measured.
fn free_space(target: &Path) -> Result<u64, String> {
    storage::available_bytes_for(target)
        .map_err(|error| format!("无法读取保存位置的剩余空间：{error}"))
}

/// One located file as the plan's row.
fn located_fact(located: &saved_files::Located) -> SavedFileFact {
    SavedFileFact {
        name: located.name.clone(),
        directory: located.directory.display().to_string(),
        bytes: located.bytes,
    }
}

/// A name that is in no known directory, in the plan's row shape.
///
/// A record rather than a bare name because the dialog shows a size beside every
/// row, and an empty directory is how 「已经不在」 reads in this shape — the page's
/// `normalizeSavedFile` sees `directory: ""` and renders 「已不存在」 (or 「未找到」,
/// when the plan also carries an `unreadable`).
fn missing_fact(name: &str) -> SavedFileFact {
    SavedFileFact {
        name: name.to_string(),
        directory: String::new(),
        bytes: None,
    }
}

/// One presence as the page's row.
///
/// No `current` when there is no file, and none when nothing is chosen: the flag
/// answers 「它是不是就在现在这个目录里」, and with no directory there is nothing
/// for it to be in.
fn state_of(presence: &Presence) -> SavedFileState {
    SavedFileState {
        name: presence.name.clone(),
        directory: presence
            .directory
            .as_ref()
            .map(|path| path.display().to_string()),
        current: presence.current,
        bytes: presence.bytes,
    }
}

/// One report as the page sees it.
///
/// The two lists the core keeps apart — `kept` with a closed reason, `failures`
/// with a sentence — cross as they are, and `missing` stays a list of names here:
/// `normalizeMigrationReport` maps it through `String`, unlike the plan's rows.
fn migration_report(core: Report) -> MigrationReport {
    MigrationReport {
        directory: core.directory.display().to_string(),
        moved: core.moved,
        deleted: core.deleted,
        missing: core.missing,
        kept: core
            .kept
            .into_iter()
            .map(|kept| KeptFileFact {
                name: kept.name,
                reason: kept.reason.as_str().to_string(),
            })
            .collect(),
        failures: core
            .failures
            .into_iter()
            .map(|failure| MigrationFailure {
                name: failure.name,
                reason: failure.reason,
            })
            .collect(),
    }
}

/// What the machine knows about each name.
fn states_of(places: &SavePlaces, names: &[String]) -> Result<Vec<SavedFileState>, String> {
    saved_files::presences(&places.search, places.chosen.as_deref(), names)
        .map(|found| found.iter().map(state_of).collect())
}

/// What moving these names into the target would do.
///
/// `free_bytes` is passed in rather than measured here so the arithmetic is a
/// test's: a plan whose numbers depend on what the test machine's disk happens to
/// be doing is a plan no test can hold still, and the measurement itself is one
/// call to `storage`.
fn plan_of(
    places: &SavePlaces,
    names: &[String],
    free_bytes: u64,
) -> Result<MigrationPlan, String> {
    let target = target_of(places)?;
    let planned = saved_files::plan(&places.search, &target, names, free_bytes)?;
    Ok(from_plan(&planned))
}

fn from_plan(planned: &Plan) -> MigrationPlan {
    MigrationPlan {
        to: planned.to.display().to_string(),
        free_bytes: planned.free_bytes,
        needed_bytes: planned.needed_bytes,
        movable: planned.movable.iter().map(located_fact).collect(),
        already_there: planned.already_there.iter().map(located_fact).collect(),
        missing: planned
            .missing
            .iter()
            .map(|name| missing_fact(name))
            .collect(),
        unreadable: planned
            .unreadable
            .iter()
            .map(|unreadable| UnreadableDirFact {
                directory: unreadable.directory.display().to_string(),
                // The raw phrase, not `sentence()`: the page prints the directory
                // itself and puts this in parentheses after it.
                reason: unreadable.reason.clone(),
            })
            .collect(),
    }
}

/// Move these names into the target.
fn moved_by(places: &SavePlaces, names: &[String]) -> Result<MigrationReport, String> {
    let target = target_of(places)?;
    saved_files::move_files(&places.search, &target, names).map(migration_report)
}

/// Delete these names, wherever they are.
///
/// No target, because nothing is written: the report still carries a directory —
/// the shape is shared with the move — and it is empty on a machine that has no
/// choice, which is the honest answer to 「这些是从哪儿删的」 when there is no
/// 「新文件写到哪儿」. The page names it only for a move.
fn deleted_by(places: &SavePlaces, names: &[String]) -> Result<MigrationReport, String> {
    let target = places.chosen.clone().unwrap_or_default();
    saved_files::delete_files(&places.search, &target, names).map(migration_report)
}

/// Where each of these names is now, in this machine's known save directories.
#[tauri::command]
pub async fn local_saved_file_states(names: Vec<String>) -> Result<Vec<SavedFileState>, String> {
    states_of(&settings::save_places()?, &names)
}

/// What moving these names into the directory now chosen would do.
///
/// Read-only: nothing is moved, and the answer is a plan the person may decline.
#[tauri::command]
pub async fn local_save_dir_migration_plan(names: Vec<String>) -> Result<MigrationPlan, String> {
    let places = settings::save_places()?;
    let target = target_of(&places)?;
    let free_bytes = free_space(&target)?;
    plan_of(&places, &names, free_bytes)
}

/// Move these names into the directory now chosen.
#[tauri::command]
pub async fn local_move_saved_files(names: Vec<String>) -> Result<MigrationReport, String> {
    moved_by(&settings::save_places()?, &names)
}

/// Delete these names from every known save directory.
#[tauri::command]
pub async fn local_delete_saved_files(names: Vec<String>) -> Result<MigrationReport, String> {
    deleted_by(&settings::save_places()?, &names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory of this test's own, removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-saved-files-cmd-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).expect("scratch dir");
        path
    }

    /// A directory of this test's own with one file of that name in it.
    fn holding(root: &Path, label: &str, name: &str, body: &[u8]) -> PathBuf {
        let directory = root.join(label);
        std::fs::create_dir_all(&directory).expect("a directory");
        std::fs::write(directory.join(name), body).expect("a file");
        directory
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    /// The choice made and an older directory behind it, in the shape
    /// `settings::save_places()` produces: the target first, then the history.
    fn places(target: &Path, older: &[PathBuf]) -> SavePlaces {
        let mut search = vec![target.to_path_buf()];
        search.extend(older.iter().cloned());
        SavePlaces {
            chosen: Some(target.to_path_buf()),
            search,
        }
    }

    // ---- the states -------------------------------------------------------

    /// The three answers the 下载中心 needs, in one call: a file in the directory
    /// in use, one in an older one, and one that is nowhere.
    ///
    /// The denominator is the point — three names, three different answers, so a
    /// command that answered one thing about everything cannot pass — and the
    /// second name is the defect 「改完保存路径后就找不到文件了」 as an assertion.
    #[test]
    fn the_three_presences_are_told_apart() {
        let root = scratch("states");
        let target = holding(&root, "now", "here.mp4", b"01234");
        let older = holding(&root, "before", "moved.mp4", b"0123456789");
        let places = places(&target, &[older.clone()]);

        let answered =
            states_of(&places, &names(&["here.mp4", "moved.mp4", "gone.mp4"])).expect("a read");

        std::fs::remove_dir_all(&root).ok();

        let by_name: Vec<(&str, Option<String>, bool, Option<u64>)> = answered
            .iter()
            .map(|state| {
                (
                    state.name.as_str(),
                    state.directory.clone(),
                    state.current,
                    state.bytes,
                )
            })
            .collect();
        assert_eq!(answered.len(), 3, "one answer per name: {by_name:?}");
        assert_eq!(
            by_name,
            vec![
                (
                    "here.mp4",
                    Some(target.display().to_string()),
                    true,
                    Some(5)
                ),
                (
                    "moved.mp4",
                    Some(older.display().to_string()),
                    false,
                    Some(10)
                ),
                ("gone.mp4", None, false, None),
            ]
        );
    }

    /// A machine that has never chosen an answer has no `current` in it.
    ///
    /// The `Option` target's own case: a target of `None` is not a default
    /// directory, and a file in the only directory the machine ever wrote to must
    /// not come back as 「就在当前位置」 on a machine where there is no current
    /// place.
    #[test]
    fn a_machine_with_no_choice_has_no_current_file() {
        let root = scratch("states-no-choice");
        let only = holding(&root, "before", "a.mp4", b"x");
        let places = SavePlaces {
            chosen: None,
            search: vec![only.clone()],
        };

        let answered = states_of(&places, &names(&["a.mp4"])).expect("a read");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            answered[0].directory.as_deref(),
            Some(only.to_str().unwrap())
        );
        assert!(!answered[0].current);
    }

    // ---- the plan ---------------------------------------------------------

    /// The plan's numbers and rows: one to carry, one already there, one gone —
    /// and the sum is the carrying one's size.
    #[test]
    fn the_plan_splits_the_names_and_sums_what_a_move_would_carry() {
        let root = scratch("plan");
        let target = holding(&root, "now", "here.mp4", b"01234");
        let older = holding(&root, "before", "moved.mp4", b"0123456789");
        let places = places(&target, &[older.clone()]);

        let plan = plan_of(
            &places,
            &names(&["moved.mp4", "here.mp4", "gone.mp4"]),
            4096,
        )
        .expect("a plan");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(plan.to, target.display().to_string());
        assert_eq!(plan.free_bytes, 4096);
        assert_eq!(plan.needed_bytes, 10, "only what a move would carry");
        assert_eq!(
            plan.movable
                .iter()
                .map(|file| (file.name.as_str(), file.directory.as_str(), file.bytes))
                .collect::<Vec<_>>(),
            vec![("moved.mp4", older.to_str().unwrap(), Some(10))]
        );
        assert_eq!(
            plan.already_there
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            vec!["here.mp4"]
        );
        assert_eq!(
            plan.missing
                .iter()
                .map(|file| (file.name.as_str(), file.directory.as_str(), file.bytes))
                .collect::<Vec<_>>(),
            vec![("gone.mp4", "", None)],
            "a missing row is a record with no directory"
        );
        assert!(plan.unreadable.is_empty());
    }

    /// A directory nobody could list reaches the page as an `unreadable` row with
    /// the **raw** phrase, not as a sentence that repeats the directory.
    ///
    /// `local-settings-view.js` renders `directory（reason）`, so a `reason` built
    /// by `Unreadable::sentence` would print the directory twice — and the page's
    /// 「未找到」 instead of 「已不存在」 hangs off this list being non-empty.
    #[test]
    fn an_unreadable_directory_arrives_as_a_directory_and_a_phrase() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("plan-unreadable");
        let target = root.join("now");
        std::fs::create_dir_all(&target).expect("a directory");
        let locked = root.join("locked");
        std::fs::create_dir_all(&locked).expect("a directory");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("lock it");
        let places = places(&target, &[locked.clone()]);

        let plan = plan_of(&places, &names(&["gone.mp4"]), 0).expect("a plan");

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("unlock");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(plan.unreadable.len(), 1, "{:?}", plan.unreadable);
        assert_eq!(plan.unreadable[0].directory, locked.display().to_string());
        assert!(
            !plan.unreadable[0]
                .reason
                .contains(&locked.display().to_string()),
            "the directory is the page's to print, not the phrase's: {}",
            plan.unreadable[0].reason
        );
        assert!(!plan.unreadable[0].reason.is_empty());
        assert_eq!(
            plan.missing.len(),
            1,
            "and the name is still reported, so the page can say 「未找到」"
        );
    }

    /// The plan needs a target, and says the same sentence the lookup uses.
    #[test]
    fn a_plan_without_a_choice_is_refused_in_the_known_words() {
        let places = SavePlaces {
            chosen: None,
            search: Vec::new(),
        };

        let refused = plan_of(&places, &names(&["a.mp4"]), 0);

        assert_eq!(refused.expect_err("no target"), "还没有选择下载保存位置");
    }

    /// A path-shaped name is refused before the plan is built.
    #[test]
    fn a_path_shaped_name_refuses_the_plan() {
        let root = scratch("plan-name");
        let target = holding(&root, "now", "a.mp4", b"x");
        let places = places(&target, &[]);

        let refused = plan_of(&places, &names(&["../a.mp4"]), 0);

        std::fs::remove_dir_all(&root).ok();

        assert!(refused.expect_err("refused").contains("不是一个文件名"));
    }

    // ---- the reports ------------------------------------------------------

    /// A move reaches the page with the name it moved, the one it left alone, the
    /// one it could not find, and the reason tokens it left alone for.
    ///
    /// One call, four outcomes: the shape `MigrationReport` exists for, and the
    /// only place the core's closed `KeptReason` becomes the page's vocabulary.
    #[test]
    fn a_move_report_carries_every_outcome() {
        let root = scratch("move-report");
        let target = holding(&root, "now", "here.mp4", b"x");
        std::fs::create_dir_all(target.join("taken.mp4")).expect("a directory in the way");
        let older = holding(&root, "before", "moved.mp4", b"0123456789");
        std::fs::write(older.join("taken.mp4"), b"x").expect("a file with a taken name");
        let places = places(&target, &[older.clone()]);

        let report = moved_by(
            &places,
            &names(&["moved.mp4", "here.mp4", "taken.mp4", "gone.mp4"]),
        )
        .expect("a move");

        let moved_there = older.join("moved.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(report.directory, target.display().to_string());
        assert_eq!(report.moved, names(&["moved.mp4"]));
        assert!(!moved_there, "and the file really moved");
        assert_eq!(report.missing, names(&["gone.mp4"]));
        let kept: Vec<(&str, &str)> = report
            .kept
            .iter()
            .map(|kept| (kept.name.as_str(), kept.reason.as_str()))
            .collect();
        assert_eq!(
            kept,
            vec![
                ("here.mp4", "same_directory"),
                ("taken.mp4", "target_exists")
            ],
            "{:?}",
            report.kept
        );
        assert!(report.deleted.is_empty());
    }

    /// A delete reaches the page with a `directory` even on a machine that has no
    /// choice — and the deletion itself does not need one.
    ///
    /// The asymmetry the two commands have on purpose: a delete writes nothing
    /// into the target, so a cleared choice must not make the 删除 button refuse
    /// to do what it says.
    #[test]
    fn a_delete_works_without_a_choice_and_reports_no_destination() {
        let root = scratch("delete-report");
        let older = holding(&root, "before", "doomed.mp4", b"x");
        let places = SavePlaces {
            chosen: None,
            search: vec![older.clone()],
        };

        let report = deleted_by(&places, &names(&["doomed.mp4"])).expect("a delete");

        let gone = !older.join("doomed.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(report.deleted, names(&["doomed.mp4"]));
        assert!(gone);
        assert_eq!(
            report.directory, "",
            "a deletion has no destination, and none is invented"
        );
        assert!(report.moved.is_empty());
    }

    /// One bad name refuses the whole call, and nothing moves.
    ///
    /// The positive control is the good name in the same list: it has to still be
    /// where it was. A partial move is a machine in a state nobody asked for, and
    /// a caller that sent a path for a name is broken rather than unlucky.
    #[test]
    fn one_bad_name_refuses_the_whole_move() {
        let root = scratch("move-bad-name");
        let target = holding(&root, "now", "kept.mp4", b"x");
        let older = holding(&root, "before", "good.mp4", b"x");
        let places = places(&target, &[older.clone()]);

        let refused = moved_by(&places, &names(&["good.mp4", "/etc/passwd"]));

        let untouched = older.join("good.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(refused.expect_err("refused").contains("不是一个文件名"));
        assert!(untouched, "the good name must not have been moved anyway");
    }

    // ---- free space -------------------------------------------------------

    /// Free space is a measurement, and a path that cannot be measured is an
    /// error rather than a `0`.
    ///
    /// Both halves in one test, because the second is only meaningful against the
    /// first: the same function answers a real number for a directory that exists,
    /// so 「什么量不出来都报错」 cannot pass instead.
    ///
    /// The failing fixture is a path **under a file**, deliberately, and not a
    /// directory that does not exist. `storage::available_bytes_for` walks up to
    /// the nearest existing ancestor on purpose — a directory this app has not
    /// created yet is still on a volume — so `root.join("no-such-directory")`
    /// answers the volume's number and a test written that way would assert the
    /// opposite of what it says (and did, on this test's first run). `ENOTDIR` is
    /// the case the walk stops on: it is not a missing component, and no ancestor
    /// would answer it differently.
    #[test]
    fn free_space_is_measured_and_a_failure_is_not_a_zero() {
        let root = scratch("free-space");
        let a_file = root.join("a-file");
        std::fs::write(&a_file, b"x").expect("a file");

        let measured = free_space(&root);
        let unmeasurable = free_space(&a_file.join("child"));

        std::fs::remove_dir_all(&root).ok();

        let measured = measured.expect("a real directory has a volume");
        assert!(measured > 0, "a volume with no free space: {measured}");
        let error = unmeasurable.expect_err("an unmeasurable path is not 0 bytes free");
        assert!(error.contains("无法读取保存位置的剩余空间"), "{error}");
    }

    /// `free_space` asks the storage module's **ancestor-walking** reader — not its
    /// strict sibling, and not a measurement of its own.
    ///
    /// The two readers differ on exactly one input: a path whose last component
    /// does not exist. `available_bytes` refuses it; `available_bytes_for` walks up
    /// to the nearest directory that does and measures that volume. The walking one
    /// is the reader this command wants (the directory the operator picked can be
    /// gone by the time a plan is asked for, and 「这个保存位置还有多少空间」 is still
    /// a question about its volume), so both halves are asserted here: the fixture
    /// has to be able to tell them apart, not merely be commented as if it did.
    ///
    /// **The number itself is deliberately not asserted, and the equality form of
    /// this test was removed.** It compared two readings of a live volume taken
    /// microseconds apart; under the full parallel suite, where other tests of this
    /// same binary are writing scratch files, the two disagreed and it failed — and
    /// a test that passes only while the rest of the machine is quiet is not
    /// evidence. What the number *means* is pinned where it can be pinned without a
    /// clock: `plan_of` is handed 4096 by its caller and the plan carries 4096.
    #[test]
    fn free_space_asks_the_reader_that_walks_up_to_the_volume() {
        let root = scratch("free-space-reader");
        let not_yet = root.join("not-created-yet");

        let mine = free_space(&not_yet);
        let strict = storage::available_bytes(&not_yet);

        std::fs::remove_dir_all(&root).ok();

        assert!(
            strict.is_err(),
            "the strict reader must refuse this path, or the two cannot be told apart here"
        );
        let mine = mine.expect("a directory that does not exist is still on a volume");
        assert!(mine > 0, "a live volume has room: {mine}");
    }

    /// **Every command this module declares is registered, and no others are.**
    ///
    /// A `#[tauri::command]` that nobody put in `generate_handler!` compiles,
    /// passes its own tests, and fails at runtime as 「command not found」. The
    /// same reader `commands::downloads` uses, with this module's prefix: the
    /// four commands here are the ones the page invokes by name, and the Web side
    /// (`desktopBridge.js`'s `TRANSFER_COMMANDS`, `local-settings/service.js`'s
    /// `LOCAL_SETTINGS_COMMANDS`) is the other direction.
    ///
    /// Read as source text rather than through any macro, because that is what
    /// 「registered」 means here. The reader takes only lines that are exactly the
    /// attribute and only entries that are not comments, which is the difference
    /// between this and a `grep` that counts a doc comment mentioning the
    /// attribute — a mistake made once already in this CHG.
    #[test]
    fn every_command_in_this_module_is_registered() {
        fn declared(source: &str) -> Vec<String> {
            let lines: Vec<&str> = source.lines().collect();
            let mut names = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if line.trim() != "#[tauri::command]" {
                    continue;
                }
                let signature = lines[index..]
                    .iter()
                    .find(|line| line.contains("fn "))
                    .unwrap_or_else(|| {
                        panic!("no signature after the attribute at line {}", index + 1)
                    });
                let after = signature.split("fn ").nth(1).expect("a name");
                names.push(
                    after
                        .split('(')
                        .next()
                        .expect("an open paren")
                        .trim()
                        .to_string(),
                );
            }
            names
        }

        fn registered(main: &str) -> Vec<String> {
            let start = main.find("generate_handler![").expect("the handler list");
            let rest = &main[start..];
            let end = rest.find("])").expect("the end of the handler list");
            let mut names = Vec::new();
            for line in rest[..end].lines() {
                let entry = line.trim().trim_end_matches(',');
                if entry.starts_with("//") {
                    continue;
                }
                if let Some(rest) = entry.strip_prefix("commands::saved_files::") {
                    names.push(rest.to_string());
                }
            }
            names
        }

        let mut declared = declared(include_str!("saved_files.rs"));
        let mut registered = registered(include_str!("../main.rs"));
        declared.sort();
        registered.sort();

        assert_eq!(
            declared, registered,
            "the module's commands and the app's handler list are not the same set"
        );
        assert_eq!(
            declared.len(),
            4,
            "the denominator, measured here: {declared:?}"
        );
    }
}
