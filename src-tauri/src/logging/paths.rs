//! Which directory Desktop's log files live in, and whether it can be written.
//!
//! Two layouts, decided by the **build's** environment, matching the one rule
//! `config` and `paths` already follow: a release binary is production and a
//! file may not talk it out of that. Production writes under the user's own
//! `Library/Logs`, which is the macOS convention for exactly this — it is
//! covered by Time Machine, it is not synced to iCloud (unlike `Documents`), and
//! Console.app can open it. Development writes into the crate's `.local/logs`,
//! the same relative shape the Agent uses for its own dev logs, so neither tree
//! surprises a developer who has read the other.
//!
//! The two layouts are *not* interchangeable and the tests say so rather than
//! leaving it implied: production reads `home` and never the manifest
//! directory, development reads the manifest directory and never `home`. A
//! release build that logged into whatever directory it happened to be built
//! from would scatter files across the machine, and a development run that
//! logged into the real `Library/Logs` would mix a developer's scratch output
//! into the logs of the installed app.
//!
//! [`directory`] is pure — a home, an environment and a manifest directory in;
//! a path out — so the shape is testable without a filesystem or a real `$HOME`.
//! [`prepare`] is the one impure part, and it answers the second question a
//! caller has: *can this directory actually be written to*. `create_dir_all`
//! succeeding does not answer it (a directory that exists may be read-only), so
//! the check is a real write. A caller that gets `Err` degrades to stderr alone
//! and starts anyway — a log directory that cannot be written must never be the
//! reason a launch fails (the user's ruling 五).
//!
//! Windows is **not** covered: `home/Library/Logs/...` is a macOS shape, and no
//! Windows layout has been measured. Registered as untested rather than guessed.

use crate::config::Environment;
use std::path::{Path, PathBuf};

/// The application directory, under the user's `Library/Logs`.
pub const APPLICATION_DIR: &str = "WTMedia";

/// The component directory. Named because a second component will want its own
/// subdirectory and "which of these files is Desktop's" should never be a
/// question asked of a shared directory listing.
pub const COMPONENT_DIR: &str = "Desktop";

/// The development layout, as path components relative to the crate.
pub const DEVELOPMENT_DIR: [&str; 2] = [".local", "logs"];

/// The prefix of the write-probe file. Dot-prefixed so it does not look like a
/// log file to a human reading the directory, and carrying the pid so two
/// instances cannot remove each other's probe.
const PROBE_PREFIX: &str = ".wt-media-write-probe-";

/// Where this build keeps its log files.
///
/// `home` is injected rather than read from the environment: a test that called
/// `std::env::home_dir` would write into whoever ran it, and a function that
/// reads the environment cannot be asked "what would you do with *this* home".
pub fn directory(home: &Path, environment: Environment, manifest_dir: &Path) -> PathBuf {
    match environment {
        Environment::Production => home
            .join("Library")
            .join("Logs")
            .join(APPLICATION_DIR)
            .join(COMPONENT_DIR),
        Environment::Development => {
            let mut path = manifest_dir.to_path_buf();
            for part in DEVELOPMENT_DIR {
                path.push(part);
            }
            path
        }
    }
}

/// Why a log directory could not be used. Holds no credentials: a log directory
/// path is a directory path.
#[derive(Debug)]
pub struct LogDirectoryError {
    pub path: PathBuf,
    pub reason: std::io::Error,
}

impl std::fmt::Display for LogDirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "log directory {} is unusable: {}",
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for LogDirectoryError {}

/// The directory for this build, created if needed, and proved writable.
///
/// Returns the directory it proved, so a caller never has to call [`directory`]
/// a second time and risk the two disagreeing. Never panics: every failure is
/// the `Err` arm, because the caller's response to all of them is the same —
/// log to stderr and carry on.
pub fn prepare(
    home: &Path,
    environment: Environment,
    manifest_dir: &Path,
) -> Result<PathBuf, LogDirectoryError> {
    let directory = directory(home, environment, manifest_dir);
    if let Err(reason) = std::fs::create_dir_all(&directory) {
        return Err(LogDirectoryError { path: directory, reason });
    }
    if let Err(reason) = probe_write(&directory) {
        return Err(LogDirectoryError { path: directory, reason });
    }
    Ok(directory)
}

/// Create, then remove, one file — the only honest way to ask "writable?".
///
/// `create_dir_all` says nothing here: a directory that already exists returns
/// `Ok` whether or not it can be written to, so a read-only log directory would
/// be reported as usable and the first real record would be the one that
/// discovered otherwise.
///
/// The probe is removed on success. If the process dies between the two calls a
/// small dot-file is left behind; the budget scan treats it as one more file it
/// may delete, and it is never mistaken for a log file by name.
fn probe_write(directory: &Path) -> std::io::Result<()> {
    let probe = directory.join(format!("{}{}", PROBE_PREFIX, std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)?;
    std::fs::remove_file(&probe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/operator")
    }

    fn manifest() -> PathBuf {
        PathBuf::from("/crate")
    }

    /// A scratch directory of this test's own. Named after the test binary's pid
    /// so two concurrent runs cannot collide, and removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-logging-{}-{}-{}",
            label,
            std::process::id(),
            // Enough separation for two tests in the same process, which share
            // a pid.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        path
    }

    #[test]
    fn production_writes_under_the_injected_home() {
        assert_eq!(
            directory(&home(), Environment::Production, &manifest()),
            PathBuf::from("/home/operator/Library/Logs/WTMedia/Desktop")
        );
    }

    #[test]
    fn development_writes_under_the_injected_manifest_directory() {
        assert_eq!(
            directory(&home(), Environment::Development, &manifest()),
            PathBuf::from("/crate/.local/logs")
        );
    }

    /// Which input decides the layout, asserted as an *absence* on both sides.
    ///
    /// Shape alone cannot tell these apart: `directory` could read the home in
    /// production and the manifest directory in development by accident, and one
    /// test per layout would still pass if it read both and picked by luck. Each
    /// arm here is built from inputs that would show up in the wrong answer, so a
    /// swapped or combined implementation fails rather than looking reasonable.
    #[test]
    fn each_layout_reads_one_input_and_ignores_the_other() {
        let production = directory(&home(), Environment::Production, &manifest());
        assert!(
            production.starts_with(home()),
            "production must be under the home it was given: {production:?}"
        );
        assert!(
            !production.starts_with(manifest()),
            "production must not be built under the manifest directory: {production:?}"
        );

        let development = directory(&home(), Environment::Development, &manifest());
        assert!(
            development.starts_with(manifest()),
            "development must be under the manifest directory it was given: {development:?}"
        );
        assert!(
            !development.starts_with(home()),
            "development must not be written into the real home: {development:?}"
        );
    }

    /// The production directory is not the development one, and the component
    /// directory is present.
    ///
    /// `WTMedia/Desktop` rather than `WTMedia`: a directory shared with a second
    /// component is one where a budget scan in one component deletes the other's
    /// files. Asserted as a suffix so a future move of the parent still has to
    /// keep the component level.
    #[test]
    fn the_production_directory_ends_at_the_component_level() {
        let path = directory(&home(), Environment::Production, &manifest());

        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(COMPONENT_DIR)
        );
        assert_eq!(
            path.parent().and_then(|parent| parent.file_name()).and_then(|name| name.to_str()),
            Some(APPLICATION_DIR)
        );
    }

    #[test]
    fn prepare_creates_the_directory_and_reports_the_same_path() {
        let root = scratch("prepare");
        let expected = directory(&root, Environment::Development, &root);

        let got = prepare(&root, Environment::Development, &root).expect("writable scratch dir");

        assert_eq!(got, expected);
        assert!(got.is_dir(), "prepare must have created it: {got:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The second launch of the day must not fail because the first one created
    /// the directory.
    #[test]
    fn prepare_is_repeatable() {
        let root = scratch("repeat");

        prepare(&root, Environment::Development, &root).expect("first");
        prepare(&root, Environment::Development, &root).expect("second");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The probe leaves nothing behind for the budget scan to count.
    #[test]
    fn prepare_leaves_no_probe_file() {
        let root = scratch("clean");
        let directory = prepare(&root, Environment::Development, &root).expect("writable");

        let leftovers: Vec<String> = std::fs::read_dir(&directory)
            .expect("read back")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(PROBE_PREFIX))
            .collect();

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(leftovers, Vec::<String>::new());
    }

    /// A probe file left behind by a process that died mid-probe must not make
    /// the directory look unusable.
    ///
    /// The probe is two syscalls, and dying between them leaves the file behind
    /// — the docstring on `PROBE_PREFIX` says so. `prepare` therefore has to
    /// tolerate finding its own file. With `create_new` instead of `create`, a
    /// launch that reused the pid of a process that had crashed would report a
    /// perfectly good log directory as unusable, and only on the first launch
    /// after the crash — which is the launch most likely to be investigated.
    #[test]
    fn a_stale_probe_file_does_not_make_the_directory_unusable() {
        let root = scratch("stale");
        let target = prepare(&root, Environment::Development, &root).expect("create it first");
        let stale = target.join(format!("{}{}", PROBE_PREFIX, std::process::id()));
        std::fs::write(&stale, b"left behind by a process that died mid-probe").expect("plant it");

        let got = prepare(&root, Environment::Development, &root);

        let still_there = stale.exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(
            got.is_ok(),
            "a stale probe file must not make the directory unusable: {got:?}"
        );
        assert!(!still_there, "the probe removes its own file, stale or not");
    }

    /// A target that is not a directory at all is an `Err`, not a panic.
    ///
    /// Made reachable by a file occupying the path rather than by a permissions
    /// trick, following the precedent in `paths.rs`: this behaves the same for a
    /// root user and in CI, whereas a mode bit does not.
    #[test]
    fn a_target_that_is_a_file_is_an_error_rather_than_a_panic() {
        let root = scratch("occupied");
        let occupied = directory(&root, Environment::Development, &root);
        std::fs::create_dir_all(occupied.parent().expect("parent")).expect("scratch parent");
        std::fs::write(&occupied, b"not a directory").expect("occupy the path");

        let error = prepare(&root, Environment::Development, &root).expect_err("must not succeed");

        assert_eq!(error.path, occupied, "the error must name the path it could not use");
        // The *kind* is asserted, not merely "some error": the failure has to be
        // the one that could not create the directory. Measured on macOS:
        // `create_dir_all` on a path occupied by a file reports `AlreadyExists`,
        // whereas the write probe reaching through the same path would report
        // `NotADirectory` — so a mutant that swallowed the first error and let
        // the second stand is caught here rather than looking equivalent.
        assert_eq!(
            error.reason.kind(),
            std::io::ErrorKind::AlreadyExists,
            "the error must name the real cause, not a downstream one"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// `create_dir_all` returning `Ok` on an existing directory is exactly why
    /// this arm exists: the directory is there and still unusable.
    ///
    /// The mode bit is the only way to build this case, and it is ignored by a
    /// privileged process — so the arm first establishes its own premise by
    /// probing directly, and reports a skip if the premise does not hold. On the
    /// machine this CHG was verified on the premise held and the assertion ran.
    #[test]
    fn a_directory_that_exists_but_cannot_be_written_is_an_error() {
        let root = scratch("readonly");
        let target = prepare(&root, Environment::Development, &root).expect("create it first");
        let original = std::fs::metadata(&target).expect("stat").permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&target, readonly).expect("make it read-only");

        let premise = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target.join("premise-probe"));
        // `create_new` failing because the file is already there would look the
        // same as failing because the directory is read-only, so the premise is
        // the *reason* and not merely "it failed".
        let premise_held = matches!(
            &premise,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied
        );
        if premise.is_ok() {
            // Cleaning up a file this arm could still create is part of the skip.
            std::fs::remove_file(target.join("premise-probe")).ok();
        }

        let outcome = if premise_held {
            prepare(&root, Environment::Development, &root).map(|_| ())
        } else {
            Ok(())
        };

        std::fs::set_permissions(&target, original).ok();
        std::fs::remove_dir_all(&root).ok();

        match outcome {
            Ok(()) if !premise_held => {
                eprintln!(
                    "skipped: a 0o555 directory is writable for this process, so the read-only \
                     branch cannot be exercised here"
                );
            }
            Ok(()) => panic!("an unwritable directory must not be reported as usable"),
            Err(error) => {
                assert_eq!(error.path, target);
                // Names the cause as well as the path: `create_dir_all` returns
                // `Ok` for a directory that exists, so the only thing that can
                // fail here is the write probe, and it must fail for the reason
                // the premise established.
                assert_eq!(
                    error.reason.kind(),
                    std::io::ErrorKind::PermissionDenied,
                    "the failure must be the read-only directory, not something else"
                );
            }
        }
    }
}
