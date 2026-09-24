//! Deleting what this app may delete, and nothing else.
//!
//! The 本机设置 page has two buttons behind this module: 清缓存 and 清理旧日志. Both
//! delete, so both live here rather than in `commands::storage`, whose read-only
//! property is a property of the module: "the page was looking at something" and
//! "the page removed something" must not be the same request.
//!
//! ## Two reachable places, and neither is decided by the caller
//!
//! [`empty_regenerable_root`] is pointed at the cache root and
//! [`remove_rotated_archives`] at one log tree. Neither takes a *list* of things
//! to delete, and the commands above them take no path at all: the directories
//! come from `app_paths::resolve` / `logging::paths::agent_directory`, the same
//! pure resolvers the launch used.
//!
//! That is the whole answer to the milestone's 清理不删业务文件与运行数据 — 素材、成片、
//! SQLite、检查点 and 待回传结果 live under the **data** root, and this module can only
//! be pointed at the **cache** root or a log tree. The protection is a path, not
//! a name list: a list has to stay right forever, and `app_paths` says the same
//! thing from the other side (`cache_is_never_inside_the_data_root`). The
//! remaining category — 正在写入文件 — *is* inside a reachable tree, and it is
//! protected by name: [`FileKind::Live`] is never deleted, whatever it holds.
//!
//! ## What inside a reachable tree is still left alone
//!
//! - **The live log file.** It is open in a writer's hand; deleting it would not
//!   free the bytes anyway (the inode survives until the handle closes) and would
//!   lose what the app is saying right now.
//! - **A name the reader does not recognise** ([`FileKind::Other`]). The reader's
//!   own rule is that such a file is "not ours"; deleting what we cannot classify
//!   is how a cleanup deletes a person's notes. A pre-T-02 install leaves
//!   `desktop-20260924-1.log` behind, and it stays — it is visible in the viewer,
//!   and removing it is a person's decision, made in the Finder.
//! - **A symlink, and anything that is neither a file nor a directory.** A link's
//!   contents are not in this tree, so following it would be deleting outside the
//!   tree we were pointed at (`storage`'s header says the same for measuring). A
//!   symlink is left where it is, and so is a socket or a device node.
//!
//! ## A tree that cannot be read is an error; a file that cannot be deleted is a line in the report
//!
//! The two halves of a cleanup fail differently, and the difference is not
//! cosmetic. A listing that fails means *nothing is known*: the report would have
//! to say "0 files removed" for a tree it never saw — the same lie as 0 MB, and
//! the reason T-05 fails whole. A single `remove_file` that fails is the opposite
//! situation: the work has already happened, some of it succeeded, and the honest
//! answer is the bytes that really went plus a line naming what did not. So
//! [`CleanupOutcome::failures`] exists, and [`CleanupOutcome::freed_bytes`]
//! counts only removals that returned `Ok`.
//!
//! ## The freed number is what was removed, not what the volume gained
//!
//! `freed_bytes` is the sum of the sizes of the files this call removed. It is
//! deliberately **not** a difference of `storage::available_bytes_for` readings:
//! the volume's free space moves under every other process on the machine, so
//! such a difference is a number that can be negative, and a page showing it
//! would be showing noise as a result. The check that this number is honest is a
//! test: an independent walk (`storage::directory_bytes`, a different code path)
//! must report exactly this much lost from the tree.

use crate::logging::reader::{self, FileKind, Source};
use crate::storage::StorageError;
use std::path::{Path, PathBuf};

/// Why an entry was deliberately left in place.
///
/// A closed vocabulary, because these strings cross the wire: the page shows a
/// reason next to a file it expected to disappear, and a reason it cannot render
/// is a row with a blank where the explanation should be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeptReason {
    /// The file is being written now ([`FileKind::Live`]).
    Live,
    /// A name this app did not write ([`FileKind::Other`]).
    Unrecognised,
    /// A symbol link: its contents are not in this tree, so it is not followed
    /// and not removed.
    Symlink,
    /// Neither a file nor a directory — a socket, a fifo, a device node.
    Special,
}

impl KeptReason {
    /// The spelling the page reads.
    pub const fn as_str(self) -> &'static str {
        match self {
            KeptReason::Live => "live",
            KeptReason::Unrecognised => "unrecognised",
            KeptReason::Symlink => "symlink",
            KeptReason::Special => "special",
        }
    }
}

/// One entry that was left where it is, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Kept {
    /// The entry's name, as it is on disk.
    pub name: String,
    pub reason: KeptReason,
}

/// One entry that was supposed to go and did not.
#[derive(Debug)]
pub struct Failure {
    pub path: PathBuf,
    pub reason: std::io::Error,
}

/// What one cleanup did.
///
/// Not `Clone`: a failure holds the `io::Error` that caused it, and that error
/// carries its own kind rather than being flattened into a string here. The
/// command maps it once, on the way out.
#[derive(Debug, Default)]
pub struct CleanupOutcome {
    /// The sizes of the files that were actually removed. See this module's
    /// header for why this is not a difference of free-space readings.
    pub freed_bytes: u64,
    /// How many files were removed.
    pub files_removed: usize,
    /// How many directories were removed. Directories a cleanup was pointed at
    /// are never among them — see [`empty_regenerable_root`].
    pub directories_removed: usize,
    /// What was deliberately left, with the reason.
    pub kept: Vec<Kept>,
    /// What could not be removed. Not empty means the cleanup is incomplete, and
    /// the counts above describe only the part that happened.
    pub failures: Vec<Failure>,
}

impl CleanupOutcome {
    /// Record one entry that was left where it is.
    fn keep(&mut self, name: String, reason: KeptReason) {
        self.kept.push(Kept { name, reason });
    }
}

/// Empty a directory whose *contents* are all regenerable, keeping the directory.
///
/// The root itself survives on purpose. It is where the app expects to write, it
/// is what the storage panel measures, and removing it would only make the next
/// `app_paths::prepare` create it again — with the extra failure mode of having
/// to tell "I deleted the cache" apart from "you have no cache".
///
/// An absent root is `Ok` with nothing done, and is **not** created: a cleanup
/// that creates a directory is a cleanup that reports a job it did not do. A root
/// that exists but cannot be read is an `Err` (this module's header).
pub fn empty_regenerable_root(root: &Path) -> Result<CleanupOutcome, StorageError> {
    let mut outcome = CleanupOutcome::default();
    // The root's own emptiness is not this function's business — it is kept
    // whether or not anything was in it — so the answer is discarded.
    let _ = empty_into(root, &mut outcome)?;
    Ok(outcome)
}

/// Empty one directory into `outcome`, and say whether it is now empty.
///
/// The question "did anything stay?" is what the parent needs: a directory that
/// still holds a kept entry cannot be removed, and trying anyway would report a
/// failure for a decision that was deliberate. The answer is `true` for an
/// absent directory — there is nothing in it to keep it in place.
fn empty_into(directory: &Path, outcome: &mut CleanupOutcome) -> Result<bool, StorageError> {
    let entries = match std::fs::read_dir(directory) {
        Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(reason) => {
            return Err(StorageError::Unreadable {
                path: directory.to_path_buf(),
                reason,
            })
        }
        Ok(entries) => entries,
    };

    let mut empty = true;
    for entry in entries {
        let entry = entry.map_err(|reason| StorageError::Unreadable {
            path: directory.to_path_buf(),
            reason,
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // `DirEntry::metadata` is `symlink_metadata`: it describes the link
        // itself, which is the only thing here we are allowed to look at.
        let metadata = entry
            .metadata()
            .map_err(|reason| StorageError::Unreadable {
                path: path.clone(),
                reason,
            })?;
        let file_type = metadata.file_type();

        if file_type.is_symlink() {
            outcome.keep(name, KeptReason::Symlink);
            empty = false;
            continue;
        }

        if file_type.is_dir() {
            if empty_into(&path, outcome)? {
                match std::fs::remove_dir(&path) {
                    Ok(()) => outcome.directories_removed += 1,
                    // Removed by someone else between the two calls: the
                    // directory is gone, which is what was asked for.
                    Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => {}
                    Err(reason) => {
                        outcome.failures.push(Failure { path, reason });
                        empty = false;
                    }
                }
            } else {
                // Something inside was deliberately kept, so this directory stays
                // with it. Not a failure: the report lists what was kept.
                empty = false;
            }
            continue;
        }

        if !file_type.is_file() {
            outcome.keep(name, KeptReason::Special);
            empty = false;
            continue;
        }

        match std::fs::remove_file(&path) {
            Ok(()) => {
                outcome.files_removed += 1;
                outcome.freed_bytes += metadata.len();
            }
            Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => {}
            Err(reason) => {
                outcome.failures.push(Failure { path, reason });
                // A file that is still there keeps its directory there too. Without
                // this the parent would try to remove the directory, fail with
                // "Directory not empty", and report a *second* problem for the one
                // already named — with the misleading reason on top, because the
                // real one is at the file.
                empty = false;
            }
        }
    }

    Ok(empty)
}

/// Remove one log tree's rotated archives, leaving everything else.
///
/// The listing is [`reader::list`]'s: the same call the viewer and the storage
/// panel use, so what a person sees in the page and what this deletes cannot
/// disagree. Files are removed by the path the listing produced, never by a name
/// joined onto a directory.
///
/// The tree's directory is never removed. An absent tree is `Ok` with nothing
/// done (`reader::list` answers an absent directory with an empty listing).
pub fn remove_rotated_archives(
    directory: &Path,
    source: Source,
) -> Result<CleanupOutcome, reader::LogReadError> {
    let mut outcome = CleanupOutcome::default();
    for file in reader::list(directory, source)? {
        match file.kind {
            FileKind::Archive => match std::fs::remove_file(&file.path) {
                Ok(()) => {
                    // The size the listing reported, which is the same number the
                    // storage panel sums for this tree — one measurement of the
                    // file, not two that could differ.
                    outcome.files_removed += 1;
                    outcome.freed_bytes += file.bytes;
                }
                Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => {}
                Err(reason) => outcome.failures.push(Failure {
                    path: file.path,
                    reason,
                }),
            },
            FileKind::Live => outcome.keep(file.name, KeptReason::Live),
            FileKind::Other => outcome.keep(file.name, KeptReason::Unrecognised),
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    /// A scratch directory of this test's own, removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-cleanup-{}-{}-{}",
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

    /// A file of exactly `bytes` bytes, with its parents created.
    fn plant(path: &Path, bytes: usize) -> u64 {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, vec![b'x'; bytes]).expect("plant");
        bytes as u64
    }

    fn record(stamp: &str, level: &str) -> String {
        format!("{stamp} [{level}] desktop.startup: hi")
    }

    fn names_in(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .expect("the directory must still be there")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort_unstable();
        names
    }

    /// An absent root is nothing to do, and asking must not bring it into being.
    ///
    /// The second half is the one that matters: `app_paths::prepare` creates
    /// directories, and a cleanup that called it would turn "you have no cache"
    /// into "here is an empty cache", which the storage panel would then show as
    /// a real zero rather than an absent tree.
    #[test]
    fn an_absent_root_has_nothing_to_delete_and_is_not_created() {
        let root = scratch("absent-root");
        std::fs::remove_dir_all(&root).ok();

        let outcome = empty_regenerable_root(&root).expect("an absent root is not a failure");
        let created = root.exists();

        assert_eq!(outcome.freed_bytes, 0);
        assert_eq!(outcome.files_removed, 0);
        assert_eq!(outcome.directories_removed, 0);
        assert!(!created, "a cleanup must not create what it was pointed at");
    }

    /// The root's contents go, the root stays, and the counts describe exactly
    /// that.
    #[test]
    fn a_root_is_emptied_down_to_its_own_contents() {
        let root = scratch("emptied");
        plant(&root.join("a.bin"), 100);
        plant(&root.join("nested/b.bin"), 200);
        plant(&root.join("nested/deeper/c.bin"), 300);

        let outcome = empty_regenerable_root(&root).expect("a readable root");

        let survives = root.is_dir();
        let leftovers = names_in(&root);
        std::fs::remove_dir_all(&root).ok();

        assert!(
            survives,
            "the root itself is kept: it is where the app writes"
        );
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        assert_eq!(outcome.files_removed, 3);
        assert_eq!(outcome.directories_removed, 2, "nested/deeper, then nested");
        assert_eq!(outcome.freed_bytes, 600);
        assert!(outcome.kept.is_empty());
        assert!(outcome.failures.is_empty());
    }

    /// The freed number is checked against a *different* code path's measurement.
    ///
    /// `directory_bytes` walks the tree itself, so a cleanup that counted what it
    /// intended to delete rather than what it deleted would disagree with it —
    /// and this is the number the page shows a person as a result.
    #[test]
    fn a_freed_byte_count_is_what_an_independent_walk_says_the_tree_lost() {
        let root = scratch("freed");
        plant(&root.join("a.bin"), 100);
        plant(&root.join("nested/b.bin"), 200);

        let before = storage::directory_bytes(&root).expect("a readable tree");
        let outcome = empty_regenerable_root(&root).expect("a readable root");
        let after = storage::directory_bytes(&root).expect("a readable tree");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(before, 300);
        assert_eq!(after, 0);
        assert_eq!(
            before - after,
            outcome.freed_bytes,
            "the number reported must be the number the tree lost"
        );
    }

    /// A root that is there and unreadable is an error, and nothing is deleted.
    ///
    /// The same line T-05 draws for reading: `NotFound` is an answer, everything
    /// else is the unknown. Here the unknown must not become a cheerful
    /// "0 bytes freed".
    #[test]
    fn an_unreadable_root_is_an_error_rather_than_a_silent_zero() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("locked-root");
        plant(&root.join("a.bin"), 10);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).expect("chmod");

        let readable = std::fs::read_dir(&root).is_ok();
        let outcome = if readable {
            None
        } else {
            Some(empty_regenerable_root(&root))
        };

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).ok();
        let survived = root.join("a.bin").exists();
        std::fs::remove_dir_all(&root).ok();

        match outcome {
            None => eprintln!(
                "premise failed: this process can read a 0o000 directory (running as root?), \
                 so the unreadable case was not exercised"
            ),
            Some(outcome) => {
                assert!(
                    matches!(outcome, Err(StorageError::Unreadable { .. })),
                    "expected an error for an unreadable root, got {outcome:?}"
                );
                assert!(survived, "an unreadable tree must not lose a file either");
            }
        }
    }

    /// A symlink is left in place, and what it points at is not touched.
    ///
    /// Deleting the link would be harmless in itself; walking through it is not —
    /// it is the one entry in this tree whose *contents* are somewhere else, and
    /// the file beside it proves the cleanup really did run.
    #[test]
    fn a_symlink_is_left_in_place_and_never_followed() {
        let outside = scratch("link-target");
        let target = outside.join("elsewhere.bin");
        plant(&target, 100_000);

        let root = scratch("links");
        plant(&root.join("real.bin"), 10);
        std::os::unix::fs::symlink(&target, root.join("elsewhere.bin")).expect("plant the link");

        let outcome = empty_regenerable_root(&root).expect("a readable root");
        let link_remains = root.join("elsewhere.bin").is_symlink();
        let target_intact = std::fs::metadata(&target).map(|m| m.len());
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();

        assert_eq!(outcome.files_removed, 1, "only the real file goes");
        assert_eq!(outcome.freed_bytes, 10);
        assert_eq!(
            outcome.kept,
            vec![Kept {
                name: "elsewhere.bin".to_string(),
                reason: KeptReason::Symlink,
            }]
        );
        assert!(link_remains, "the link itself is left where it is");
        assert_eq!(target_intact.ok(), Some(100_000), "the target is untouched");
        assert!(
            outcome.failures.is_empty(),
            "a deliberate keep is not a failure: {:?}",
            outcome.failures
        );
    }

    /// A directory that still holds something kept stays with it.
    ///
    /// The other half of the rule above: the parent has to *ask* whether its
    /// child is empty before removing it. Removing anyway would fail — a
    /// directory with a link in it is not empty — and that failure would be
    /// reported against a cleanup that did exactly the right thing.
    #[test]
    fn a_directory_that_still_holds_a_kept_entry_stays_with_it() {
        let outside = scratch("nested-link-target");
        let target = outside.join("elsewhere.bin");
        plant(&target, 100);

        let root = scratch("nested-links");
        plant(&root.join("nested/real.bin"), 7);
        std::fs::create_dir_all(root.join("nested/kept")).expect("the link's own directory");
        std::os::unix::fs::symlink(&target, root.join("nested/kept/link")).expect("plant the link");

        let outcome = empty_regenerable_root(&root).expect("a readable root");
        let nested_survives = root.join("nested/kept/link").is_symlink();
        let remaining = names_in(&root);
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();

        assert_eq!(outcome.files_removed, 1, "the real file went");
        assert_eq!(outcome.kept.len(), 1, "{:?}", outcome.kept);
        assert!(
            outcome.failures.is_empty(),
            "keeping a link is not a failure: {:?}",
            outcome.failures
        );
        assert!(nested_survives, "the link is still there");
        assert_eq!(
            remaining,
            vec!["nested".to_string()],
            "and the directory holding it stayed with it"
        );
    }

    /// Neither a file nor a directory: not ours to delete.
    ///
    /// A unix socket is the cheapest real one to plant, and it is not a
    /// hypothetical — a sidecar's socket would land in a directory like this one.
    #[test]
    fn a_socket_is_left_in_place() {
        let root = scratch("sockets");
        let socket = root.join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind a socket");
        plant(&root.join("real.bin"), 5);

        let outcome = empty_regenerable_root(&root).expect("a readable root");
        let socket_remains = socket.exists();
        drop(listener);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(outcome.files_removed, 1);
        assert_eq!(outcome.freed_bytes, 5);
        assert_eq!(
            outcome.kept,
            vec![Kept {
                name: "agent.sock".to_string(),
                reason: KeptReason::Special,
            }]
        );
        assert!(socket_remains, "a socket is not a file to remove");
    }

    /// A removal that fails is reported, not counted — and reported **once**.
    ///
    /// A read-only *parent* is how a real machine produces this: the directory
    /// can be listed (so the cleanup knows what is in it) but not written to (so
    /// nothing inside can go). The report must not claim bytes it did not free,
    /// and it must not invent a second problem either: a file that will not go
    /// keeps its directory in place, so the directory is *not* attempted, and
    /// the report does not also carry a "Directory not empty" for the same file.
    #[test]
    fn a_file_that_cannot_be_deleted_is_reported_and_not_counted() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("undeletable");
        let locked = root.join("locked");
        plant(&locked.join("a.bin"), 40);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).expect("chmod");

        let outcome = empty_regenerable_root(&root).expect("a readable root");

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).ok();
        let survived = locked.join("a.bin").exists();
        std::fs::remove_dir_all(&root).ok();

        if !survived {
            eprintln!(
                "premise failed: this process deleted a file in a 0o500 directory \
                 (running as root?), so the failing-removal case was not exercised"
            );
            return;
        }
        assert_eq!(outcome.files_removed, 0);
        assert_eq!(
            outcome.freed_bytes, 0,
            "nothing went, so nothing is claimed"
        );
        assert_eq!(
            outcome.failures.len(),
            1,
            "one problem, reported once: {:?}",
            outcome.failures
        );
        assert!(
            outcome.failures[0].path.ends_with("a.bin"),
            "and it is named: {:?}",
            outcome.failures
        );
    }

    /// The live file stays; its rotated hours go.
    ///
    /// This is the one file inside a reachable tree that the *name* protects, and
    /// the milestone says it plainly: 正在写入文件永不删.
    #[test]
    fn the_live_file_is_never_touched_and_the_archives_are() {
        let tree = scratch("log-tree");
        let live = tree.join("desktop.log");
        std::fs::write(&live, record("2026-09-24T21:00:00", "INFO")).expect("plant");
        let older = plant(&tree.join("desktop.log.2026-09-24-19"), 120);
        let newer = plant(&tree.join("desktop.log.2026-09-24-20"), 80);

        let first = remove_rotated_archives(&tree, Source::Desktop).expect("a readable tree");
        let survived = std::fs::read_to_string(&live).ok();
        let leftovers = names_in(&tree);
        std::fs::remove_dir_all(&tree).ok();

        assert_eq!(first.files_removed, 2);
        assert_eq!(first.freed_bytes, older + newer);
        assert_eq!(
            first.directories_removed, 0,
            "the tree is not the cleanup's"
        );
        assert_eq!(
            first.kept,
            vec![Kept {
                name: "desktop.log".to_string(),
                reason: KeptReason::Live,
            }]
        );
        assert_eq!(leftovers, vec!["desktop.log".to_string()]);
        assert!(
            survived.is_some_and(|text| text.contains("INFO")),
            "the live file must keep its contents"
        );
    }

    /// An archive that cannot be deleted is reported, and the live file is still
    /// untouched in the same call.
    ///
    /// A read-only log directory is a real state — an administrator's, or a
    /// mounted volume that went read-only — and it is the one where a viewer and a
    /// cleanup differ most: for the log half there is no "kept because the tree is
    /// broken", every file is either removed or named.
    #[test]
    fn an_archive_that_cannot_be_deleted_is_reported_and_the_live_file_stays() {
        use std::os::unix::fs::PermissionsExt;

        let tree = scratch("undeletable-archive");
        std::fs::write(
            tree.join("desktop.log"),
            record("2026-09-24T21:00:00", "INFO"),
        )
        .expect("plant");
        plant(&tree.join("desktop.log.2026-09-24-19"), 90);
        std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o500)).expect("chmod");

        let outcome = remove_rotated_archives(&tree, Source::Desktop).expect("a readable tree");

        std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o755)).ok();
        let live_survives = tree.join("desktop.log").exists();
        let archive_survived = tree.join("desktop.log.2026-09-24-19").exists();
        std::fs::remove_dir_all(&tree).ok();

        if !archive_survived {
            eprintln!(
                "premise failed: this process deleted a file in a 0o500 directory \
                 (running as root?), so the failing-removal case was not exercised"
            );
            return;
        }
        assert_eq!(outcome.files_removed, 0);
        assert_eq!(
            outcome.freed_bytes, 0,
            "nothing went, so nothing is claimed"
        );
        assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
        assert!(
            outcome.failures[0]
                .path
                .ends_with("desktop.log.2026-09-24-19"),
            "the archive that could not go is named: {:?}",
            outcome.failures
        );
        assert!(
            live_survives,
            "and the live file is not collateral of a failed cleanup"
        );
        assert_eq!(
            outcome.kept,
            vec![Kept {
                name: "desktop.log".to_string(),
                reason: KeptReason::Live,
            }]
        );
    }

    /// A name the reader does not recognise stays, and the reason says so.
    ///
    /// On a real machine this is the pre-T-02 `desktop-20260924-1.log`. The
    /// viewer shows it, and a person can remove it; a cleanup that guessed would
    /// be one deleted `notes.txt` away from being a bug report.
    #[test]
    fn an_unrecognised_name_is_left_where_the_reader_leaves_it() {
        let tree = scratch("unrecognised");
        let stray = plant(&tree.join("desktop-20260924-1.log"), 64);
        plant(&tree.join("desktop.log.2026-09-24-19"), 30);
        let _ = stray;

        let outcome = remove_rotated_archives(&tree, Source::Desktop).expect("a readable tree");
        let stray_remains = tree.join("desktop-20260924-1.log").exists();
        std::fs::remove_dir_all(&tree).ok();

        assert_eq!(outcome.files_removed, 1);
        assert_eq!(outcome.freed_bytes, 30);
        assert_eq!(
            outcome.kept,
            vec![Kept {
                name: "desktop-20260924-1.log".to_string(),
                reason: KeptReason::Unrecognised,
            }]
        );
        assert!(
            stray_remains,
            "a file this app did not write is not this app's to delete"
        );
    }

    /// The tree the cleanup was pointed at is never removed, even when everything
    /// in it went.
    ///
    /// The writer holds a handle on that directory, and `logging::rolling` reopens
    /// files in it: a cleanup that removed the tree would break the next rotation
    /// rather than free space.
    #[test]
    fn the_log_tree_itself_is_never_removed() {
        let tree = scratch("tree-survives");
        plant(&tree.join("desktop.log.2026-09-24-19"), 12);

        let outcome = remove_rotated_archives(&tree, Source::Desktop).expect("a readable tree");
        let survives = tree.is_dir();
        let leftovers = names_in(&tree);
        std::fs::remove_dir_all(&tree).ok();

        assert_eq!(outcome.files_removed, 1);
        assert!(survives, "the tree stays");
        assert!(leftovers.is_empty(), "but nothing is left in it");
    }

    /// An absent log tree is nothing to do, and is not created either.
    #[test]
    fn an_absent_log_tree_is_nothing_to_do() {
        let tree = scratch("absent-tree");
        std::fs::remove_dir_all(&tree).ok();

        let outcome = remove_rotated_archives(&tree, Source::Agent).expect("an absent tree");
        let created = tree.exists();

        assert_eq!(outcome.freed_bytes, 0);
        assert_eq!(outcome.files_removed, 0);
        assert!(outcome.kept.is_empty());
        assert!(!created, "the Agent's tree is not this command's to create");
    }

    /// The vocabulary the page renders, pinned. A rename here is a change to the
    /// page, which is the intended cost.
    #[test]
    fn the_kept_reasons_are_the_pages_vocabulary() {
        assert_eq!(KeptReason::Live.as_str(), "live");
        assert_eq!(KeptReason::Unrecognised.as_str(), "unrecognised");
        assert_eq!(KeptReason::Symlink.as_str(), "symlink");
        assert_eq!(KeptReason::Special.as_str(), "special");
    }
}
