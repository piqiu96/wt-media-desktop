//! How much room is left, and how much of this app is using.
//!
//! Two questions the 本机设置 page asks that are not about paths at all: the
//! volume has *this* much free, and the cache and the log trees occupy *these*
//! many bytes. Both are measurements of the machine as it is now, so both are
//! taken when asked and neither is cached — a number that was true a minute ago
//! is exactly the kind of number a storage panel must not show.
//!
//! ## A missing directory is zero bytes; a directory we cannot read is an error
//!
//! This is the milestone's 读取失败必须是错误而非 0 MB, and the two halves have to
//! be told apart rather than lumped into one `unwrap_or(0)`:
//!
//! - **Not there** is a true zero. A first launch has no cache and no archives,
//!   so 0 MB is the honest reading and reporting an error would train a reader to
//!   ignore the error that matters.
//! - **There but unreadable** is not a reading at all. `PermissionDenied` on a
//!   directory, a directory where a file should be, an I/O error mid-walk: every
//!   one of them means the answer is unknown, and printing 0 for it is a lie the
//!   page would then act on ("缓存占 0 MB，可以清空" on a tree it never saw).
//!
//! The line between them is drawn on the `io::ErrorKind`, not on `Path::exists`:
//! `exists()` is false both for "absent" and for "an ancestor is not searchable",
//! and only the first of those is a zero.
//!
//! ## Where free space comes from
//!
//! The answer is platform-specific on purpose. Unix uses `statvfs` through
//! `rustix`'s safe wrapper; Windows uses `GetDiskFreeSpaceExW` through
//! `windows-sys`, which is already in the lockfile as a Tauri dependency. Both
//! report the bytes available to the current caller rather than raw blocks that
//! only an administrator could use.
//!
//! Targets outside the supported desktop platforms still get an honest refusal
//! rather than a guessed number.
//!
//! ## Symlinks are not followed
//!
//! [`directory_bytes`] measures what is *in* the tree. A symlink counts as the
//! link itself (a few dozen bytes), never as its target: following one would let
//! a link inside the cache directory report the size of a film elsewhere on the
//! disk, and — once the cleanup command exists — would let it delete outside the
//! tree it was pointed at.

use std::path::{Path, PathBuf};

/// Why a storage question could not be answered. Holds no credentials: a path is
/// a path.
#[derive(Debug)]
pub enum StorageError {
    /// The tree is there but could not be walked, so its size is unknown.
    Unreadable {
        path: PathBuf,
        reason: std::io::Error,
    },
    /// This build cannot ask the operating system how much room is left.
    Unsupported { reason: String },
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Unreadable { path, reason } => write!(
                f,
                "the size of {} could not be measured (it is not zero): {reason}",
                path.display()
            ),
            StorageError::Unsupported { reason } => {
                write!(f, "free space is not measurable on this platform: {reason}")
            }
        }
    }
}

impl std::error::Error for StorageError {}

/// The bytes of every file under `root`, counted recursively.
///
/// A `root` that does not exist is `Ok(0)`. Anything else that stops the walk is
/// an `Err` — see this module's header for why the two cannot share an answer.
///
/// Subdirectories are entered, symlinked ones are not: the walk visits real
/// directories only, so a loop of links cannot make it run forever.
pub fn directory_bytes(root: &Path) -> Result<u64, StorageError> {
    match std::fs::read_dir(root) {
        // The one error that is an answer rather than a failure. Anything else
        // -- permissions, a file where a directory belongs, an unreadable
        // ancestor -- is the unknown this function refuses to report as 0.
        Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(reason) => Err(StorageError::Unreadable {
            path: root.to_path_buf(),
            reason,
        }),
        Ok(entries) => {
            let mut total = 0_u64;
            for entry in entries {
                let entry = entry.map_err(|reason| StorageError::Unreadable {
                    path: root.to_path_buf(),
                    reason,
                })?;
                let path = entry.path();
                // `DirEntry::metadata` is `symlink_metadata`: it describes the
                // link itself rather than what it points at.
                let metadata = entry
                    .metadata()
                    .map_err(|reason| StorageError::Unreadable {
                        path: path.clone(),
                        reason,
                    })?;
                if metadata.is_dir() {
                    total += directory_bytes(&path)?;
                } else {
                    total += metadata.len();
                }
            }
            Ok(total)
        }
    }
}

/// The bytes available to this user on the volume holding `path`.
///
/// `f_bavail`, not `f_bfree`: the latter counts blocks reserved for root, which
/// no ordinary process can write into, and a storage panel that quotes it
/// promises room the user does not have.
///
/// `path` must exist — the platform free-space API fails with `NotFound` otherwise, and that is
/// reported rather than rounded to zero. The caller passes a root it has already
/// prepared (`app_paths::prepare`), so "the path I measure" and "the path I use"
/// are the same directory.
pub fn available_bytes(path: &Path) -> Result<u64, StorageError> {
    platform::available_bytes(path)
}

/// The bytes available on the volume that *would* hold `path`, which need not
/// exist.
///
/// The rule is the nearest existing ancestor: free space is a property of the
/// volume, and a directory the app has not created yet is still on one. Without
/// this, a first launch would report no free space — refused because nothing has
/// written to the data root yet, which is not a condition the user can act on —
/// and the alternative of creating the directory to make the question answerable
/// is not a read command's business (`app_paths::prepare` is the writer's).
///
/// This is where the two questions in this module part company, and the split is
/// deliberate: [`directory_bytes`] answers 0 for an absent tree because *nothing
/// is in it*, while this walks up because the volume is not in the directory.
/// Both are true statements about different questions.
///
/// A path with no existing ancestor at all (a bare relative name) is an error:
/// the walk stops at the last component and reports what the platform API said there.
pub fn available_bytes_for(path: &Path) -> Result<u64, StorageError> {
    let mut candidate = path;
    loop {
        match platform::available_bytes(candidate) {
            // Only the missing-component case walks up. `PermissionDenied` on an
            // ancestor means the volume is there and this process may not look
            // at it, which is not a question the parent would answer differently.
            Err(StorageError::Unreadable { reason, .. })
                if reason.kind() == std::io::ErrorKind::NotFound =>
            {
                match candidate.parent() {
                    Some(parent) => candidate = parent,
                    None => {
                        return Err(StorageError::Unreadable {
                            path: path.to_path_buf(),
                            reason,
                        })
                    }
                }
            }
            other => return other,
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::{Path, StorageError};

    pub(super) fn available_bytes(path: &Path) -> Result<u64, StorageError> {
        let stats = rustix::fs::statvfs(path).map_err(|reason| StorageError::Unreadable {
            path: path.to_path_buf(),
            reason: reason.into(),
        })?;
        // Both are in units of `f_frsize` — the fragment size, which is what
        // POSIX says `f_bavail` is counted in, and on APFS is 4096. `f_bsize`
        // is the preferred I/O size and is *not* the unit here; using it happens
        // to be right on the filesystems this has been measured on, which is
        // exactly the kind of accident that breaks on the next one.
        Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
    }
}

#[cfg(windows)]
mod platform {
    use super::{Path, StorageError};
    use std::os::windows::ffi::OsStrExt;

    pub(super) fn available_bytes(path: &Path) -> Result<u64, StorageError> {
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut available = 0_u64;
        let mut total = 0_u64;
        let mut total_free = 0_u64;
        let measured = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                &mut total,
                &mut total_free,
            )
        };
        if measured == 0 {
            return Err(StorageError::Unreadable {
                path: path.to_path_buf(),
                reason: std::io::Error::last_os_error(),
            });
        }
        Ok(available)
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::{Path, StorageError};

    pub(super) fn available_bytes(_path: &Path) -> Result<u64, StorageError> {
        Err(StorageError::Unsupported {
            reason: "this desktop target has no supported free-space API".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory of this test's own, named after the pid so two
    /// concurrent runs cannot collide, and removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-storage-{}-{}-{}",
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

    /// A file of exactly `bytes` bytes, so the expected total is a number rather
    /// than a guess.
    fn plant(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("plant parents");
        std::fs::write(path, vec![b'x'; bytes]).expect("plant");
    }

    #[test]
    fn a_tree_is_the_sum_of_its_files() {
        let root = scratch("sum");
        plant(&root.join("a.log"), 1000);
        plant(&root.join("nested/b.log"), 2000);
        plant(&root.join("nested/deeper/c.log"), 24);

        let total = directory_bytes(&root);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(total.expect("measurable"), 3024);
    }

    /// Windows reports through `GetDiskFreeSpaceExW`; this pins the command to
    /// a real reading rather than the old non-Unix refusal.
    #[cfg(windows)]
    #[test]
    fn windows_available_bytes_is_measurable() {
        let measured = available_bytes(&std::env::temp_dir());
        assert!(
            matches!(measured, Ok(bytes) if bytes > 0),
            "expected a positive free-space reading, got {measured:?}"
        );
    }

    /// The milestone's 不是 0 MB, half one: absent really is zero, and the walk
    /// must not create the directory it was asked about.
    #[test]
    fn an_absent_tree_is_zero_and_is_not_created() {
        let root = scratch("absent");
        let missing = root.join("never-written");

        let total = directory_bytes(&missing);

        let created = missing.exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(total.expect("a true zero"), 0);
        assert!(!created, "measuring must not create the tree");
    }

    /// The other half, and the one the milestone names: a tree that is there but
    /// unreadable is an error, not a zero.
    ///
    /// Reached by making the *root itself* a file, which fails the same way a
    /// `PermissionDenied` does from the caller's side but does not depend on
    /// being able to drop permissions — a root user, or a CI runner with a
    /// different umask, gets the same reading. The permission case is covered by
    /// the next test, which checks its own premise first.
    #[test]
    fn a_tree_that_is_a_file_is_an_error_rather_than_zero() {
        let root = scratch("isfile");
        let target = root.join("not-a-directory");
        plant(&target, 10);

        let measured = directory_bytes(&target);

        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(measured, Err(StorageError::Unreadable { .. })),
            "expected an error, got {measured:?}"
        );
    }

    /// The unreadable-directory case itself. Skipped, with a printed reason,
    /// when the premise does not hold — running as root ignores the mode bits,
    /// and a test that silently measured a readable directory would be a green
    /// light over nothing.
    #[test]
    fn a_tree_that_cannot_be_read_is_an_error_rather_than_zero() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("unreadable");
        plant(&root.join("locked/a.log"), 100);
        let locked = root.join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");

        let still_readable = std::fs::read_dir(&locked).is_ok();
        let measured = if still_readable {
            None
        } else {
            Some(directory_bytes(&locked))
        };

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).ok();
        std::fs::remove_dir_all(&root).ok();

        match measured {
            None => eprintln!(
                "premise failed: this process can read a 0o000 directory (running as root?), \
                 so the unreadable case was not exercised"
            ),
            Some(measured) => assert!(
                matches!(measured, Err(StorageError::Unreadable { .. })),
                "expected an error for an unreadable tree, got {measured:?}"
            ),
        }
    }

    /// A symlink is sized as a link, not as whatever it points at.
    ///
    /// The file it points at is far larger than the link, so "the target was
    /// followed" is a difference no rounding can hide.
    #[test]
    fn a_symlink_is_not_followed() {
        let outside = scratch("outside");
        plant(&outside.join("huge.bin"), 100_000);
        let root = scratch("links");
        plant(&root.join("here.log"), 1000);
        std::os::unix::fs::symlink(outside.join("huge.bin"), root.join("elsewhere.bin"))
            .expect("plant the link");

        let total = directory_bytes(&root);
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();

        let total = total.expect("measurable");
        assert!(
            total >= 1000 && total < 2000,
            "the link's target must not be counted: {total}"
        );
    }

    /// A link to a directory is not walked either — the same rule, and the one
    /// that keeps a link loop from being an infinite walk.
    #[test]
    fn a_symlinked_directory_is_not_walked() {
        let outside = scratch("outside-dir");
        plant(&outside.join("huge.bin"), 100_000);
        let root = scratch("linked-dir");
        plant(&root.join("here.log"), 1000);
        std::os::unix::fs::symlink(&outside, root.join("elsewhere")).expect("plant the link");

        let total = directory_bytes(&root);
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();

        assert!(total.expect("measurable") < 2000);
    }

    /// A real (non-symlinked) empty tree is zero, and is not confused with the
    /// error cases above.
    #[test]
    fn an_empty_tree_is_zero() {
        let root = scratch("empty");
        let total = directory_bytes(&root);
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(total.expect("measurable"), 0);
    }

    /// Free space is a positive number for a directory that exists, and it is a
    /// number that fits on the volume it is about.
    #[test]
    fn free_space_is_measured_for_an_existing_path() {
        let root = scratch("space");
        let stats = rustix::fs::statvfs(&root).expect("the scratch directory's volume");
        let available = available_bytes(&root);
        std::fs::remove_dir_all(&root).ok();

        let available = available.expect("measurable");
        assert!(available > 0, "a live volume has room: {available}");
        // The bound is the volume's own size, measured here rather than asserted
        // as "a big number". It is the reading that catches a wrong *unit*, and the
        // unit is where this function can be wrong without looking wrong: on this
        // machine `f_bsize` is 1 MiB and `f_frsize` is 4096, so multiplying by the
        // wrong one overstates the answer 256× — 93 GB reads as 23.8 TB, which a
        // "less than an exbibyte" bound waves through. Free space cannot exceed
        // what the volume holds, and that inequality is false for any mix-up that
        // inflates the reading.
        let total = stats.f_blocks.saturating_mul(stats.f_frsize);
        assert!(
            available <= total,
            "free space cannot exceed the volume's total: {available} > {total}"
        );
    }

    /// A path that is not there cannot be asked about: the platform API has no answer
    /// for it, and 0 would read as "the disk is full".
    #[test]
    fn free_space_for_an_absent_path_is_an_error_not_zero() {
        let root = scratch("space-absent");
        let missing = root.join("never-created");

        let available = available_bytes(&missing);

        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(available, Err(StorageError::Unreadable { .. })),
            "expected an error, got {available:?}"
        );
    }

    /// Two readings of one volume, taken moments apart, agreeing.
    ///
    /// Equality is what pins the claim — the same volume, not a neighbouring one —
    /// but free space is a *live* number: any process writing a file between the
    /// two calls moves it by however much it wrote, and a test that read once
    /// would fail for that and be read as a defect. (Measured during T-05's
    /// mutation table: one run failed whose only difference was a rebuilt binary;
    /// twenty focused reruns passed, pristine and mutated alike.) So a mismatch is
    /// retried rather than tolerated: a neighbouring volume does not match on any
    /// attempt, while a machine that is merely busy does.
    fn readings_agree<'a>(
        mut read: impl FnMut() -> (Result<u64, StorageError>, Result<u64, StorageError>),
        description: &'a str,
    ) -> u64 {
        let mut last = (0_u64, 0_u64);
        for _ in 0..4 {
            let (left, right) = read();
            last = (
                left.expect("a readable volume"),
                right.expect("a readable volume"),
            );
            if last.0 == last.1 {
                return last.0;
            }
        }
        panic!("{description}: {} != {}", last.0, last.1);
    }

    /// The volume of a path that does not exist yet is answered from its nearest
    /// existing ancestor, and it is the *same* volume as the parent's — which is
    /// the whole claim, so it is asserted against the parent's own reading rather
    /// than against "some positive number".
    #[test]
    fn free_space_for_an_absent_path_comes_from_its_nearest_existing_ancestor() {
        let root = scratch("space-ancestor");
        let missing = root.join("not-yet/deeper/still-deeper");

        let agreed = readings_agree(
            || (available_bytes_for(&missing), available_bytes(&root)),
            "the reading must be the volume's, not a neighbouring one's",
        );
        let created = root.join("not-yet").exists();

        std::fs::remove_dir_all(&root).ok();

        assert!(!created, "measuring must not create the tree it measured");
        assert!(agreed > 0, "a live volume has room: {agreed}");
    }

    /// And the walk is only for missing components: a path that exists is
    /// measured where it is, so this and [`available_bytes`] agree.
    #[test]
    fn free_space_for_an_existing_path_is_the_same_either_way() {
        let root = scratch("space-either");

        readings_agree(
            || (available_bytes_for(&root), available_bytes(&root)),
            "an existing path is measured where it is",
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A path with nothing behind it stops at its last component and reports what
    /// the platform API said, rather than walking up to a directory the caller never
    /// mentioned.
    ///
    /// A relative name with no separator is the case: its "parent" is the empty
    /// path, and answering from the working directory would report free space for
    /// a directory nobody asked about.
    #[test]
    fn free_space_stops_at_the_last_component_and_reports_the_error() {
        let available = available_bytes_for(Path::new("no-such-relative-name-here"));

        assert!(
            matches!(available, Err(StorageError::Unreadable { .. })),
            "expected an error rather than the working directory's volume: {available:?}"
        );
    }

    /// Both errors name their path, because the page has to say which tree it
    /// could not read.
    #[test]
    fn an_error_names_the_path_it_could_not_measure() {
        let error = StorageError::Unreadable {
            path: PathBuf::from("/Users/operator/Library/Caches/WTMedia/Desktop"),
            reason: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let shown = error.to_string();
        assert!(
            shown.contains("/Users/operator/Library/Caches/WTMedia/Desktop"),
            "{shown}"
        );
        assert!(
            shown.contains("not zero"),
            "the message must not read as a zero: {shown}"
        );
    }
}
