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
//!
//! ## The Agent's tree, which Desktop only mirrors
//!
//! The log viewer shows both components' files, and the Agent's are not in
//! Desktop's directory — they are wherever the Agent decided to put them
//! (`runtime/paths.py`, which is the Agent's to own). Desktop therefore mirrors
//! **the inputs the Agent uses**, not its own: [`agent_directory`] takes a home
//! and the configured data directory and takes *no* `Environment`, because a
//! Desktop development build says nothing about where the Agent writes while the
//! Agent's own `environment` and frozen-ness say everything.
//!
//! Two of the Agent's three branches are mirrored:
//!
//! | the Agent's rule | mirrored as |
//! | --- | --- |
//! | a non-empty `data_dir` override wins, logs beside it | [`agent_directory`] expands a leading `~/` and appends `logs` |
//! | frozen *or* production ⇒ `~/Library/Logs/WTMedia/Agent` | the arm an unset `data_dir` takes |
//!
//! The third — not frozen, not production ⇒ the Agent's own checkout
//! (`<repo>/.local/logs`) — is **not** mirrored and is not guessed at. Desktop
//! cannot see either input: `frozen` is a property of a process it did not start
//! from here, and the Agent's `environment` comes from the Agent's own config
//! file, which is the Agent's to read (the shipped one says `production`; the
//! built-in default is `development`, so the branch is reachable by editing that
//! file rather than by anything Desktop does). Pointing at the installed tree
//! when the Agent logs elsewhere costs an empty list and a visible path — the
//! page shows the directory it read — whereas guessing the checkout would point
//! a reader at somebody else's `.local/logs`.
//!
//! Measured on the machine this was written on: `~/Library/Logs/WTMedia/Agent`
//! holds the Agent's three live files and the Agent repo's `.local/logs` is
//! empty, so the runs that produced them took the installed branch — which is
//! the arm an unset `data_dir` predicts.

use crate::config::Environment;
use crate::system_paths::{Platform, SystemPaths};
use std::path::{Path, PathBuf};

/// The application directory, under the user's `Library/Logs`.
pub const APPLICATION_DIR: &str = "WTMedia";

/// The component directory. Named because a second component will want its own
/// subdirectory and "which of these files is Desktop's" should never be a
/// question asked of a shared directory listing.
pub const COMPONENT_DIR: &str = "Desktop";

/// The Agent's component directory, one level over in the same application
/// directory. Not a courtesy: it is the other half of the name the Agent chose
/// in `runtime/paths.py` (`INSTALLED_LOGS_DIR`).
pub const AGENT_COMPONENT_DIR: &str = "Agent";

/// The subdirectory an Agent data directory keeps its logs in, taken from the
/// Agent's own override branch (`logs_dir = base / "logs"`).
pub const AGENT_DATA_LOGS_SUBDIR: &str = "logs";

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
pub fn directory(system: &SystemPaths, environment: Environment, manifest_dir: &Path) -> PathBuf {
    if environment == Environment::Development {
        return manifest_dir
            .join(DEVELOPMENT_DIR[0])
            .join(DEVELOPMENT_DIR[1]);
    }

    let base = match system.platform {
        Platform::Windows => system
            .local_app_data
            .as_deref()
            .unwrap_or_else(|| Path::new("")),
        Platform::Darwin | Platform::Unix => {
            system.home.as_deref().unwrap_or_else(|| Path::new(""))
        }
    };
    match system.platform {
        Platform::Windows => base.join(APPLICATION_DIR).join(COMPONENT_DIR).join("logs"),
        Platform::Darwin | Platform::Unix => base
            .join("Library")
            .join("Logs")
            .join(APPLICATION_DIR)
            .join(COMPONENT_DIR),
    }
}

#[derive(Debug)]
pub struct LogDirectoryError {
    pub path: PathBuf,
    pub reason: std::io::Error,
}

/// Why an Agent log directory could not be determined from here.
///
/// Not an [`std::io::Error`]: nothing was read. This is a refusal to answer,
/// which is a different thing from a failure to read, and a page that showed the
/// two the same way would let "configured in a way I cannot follow" look like
/// "the directory is broken".
#[derive(Debug)]
pub enum AgentDirectoryError {
    /// The Agent's default tree is under the user's home and this process has no
    /// home to build it from — the same input `app_paths::resolve` refuses to
    /// guess at for the installed layout, refused for the same reason.
    NoHome,
    /// Windows production cannot derive the Agent tree without Local AppData.
    NoLocalAppData,
    /// A configured value that cannot be resolved from this process.
    Unresolvable {
        /// The value, quoted back so whoever set it sees what was refused. It
        /// comes from Desktop's own config, not from a log file, and it is a
        /// directory rather than a secret.
        configured: String,
        reason: &'static str,
    },
}

impl std::fmt::Display for AgentDirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentDirectoryError::NoHome => write!(
                f,
                "the Agent's log directory is under the user's home ({}), and this process has no \
                 home directory set",
                AGENT_COMPONENT_DIR
            ),
            AgentDirectoryError::NoLocalAppData => write!(
                f,
                "LOCALAPPDATA is not set, so the Windows Agent tree cannot be resolved"
            ),
            AgentDirectoryError::Unresolvable { configured, reason } => write!(
                f,
                "the Agent's log directory cannot be determined from the configured data \
                 directory {configured:?}: {reason}"
            ),
        }
    }
}

impl std::error::Error for AgentDirectoryError {}

/// Where the Agent put its log files, as far as Desktop can determine it.
///
/// Mirrors `RuntimePaths.resolve`'s override and installed branches — see this
/// module's header for the branch that is deliberately not mirrored.
///
/// The `data_dir` input is Desktop's own config value (`[agent] data_dir`),
/// which is passed to the child as `WT_MEDIA_AGENT_DATA_DIR` when it is set:
/// reading the same setting the Agent was told means the two cannot disagree
/// about which directory to look in.
///
/// A relative or `~user`-shaped value is refused rather than resolved. The
/// Agent would resolve such a path against *its* working directory, which
/// Desktop does not know, so any answer given here would be about a different
/// directory than the one being written to — and a listing of the wrong
/// directory presented as the Agent's logs is worse than no listing.
///
/// `home` is the same optional input `app_paths::resolve` takes, and for the same
/// reason: the installed tree cannot be built without it while a configured
/// absolute `data_dir` needs no home at all. A home that is not absolute is
/// treated as absent — an empty or relative `HOME` would produce a relative
/// answer that resolves against whatever directory this process happens to be in.
pub fn agent_directory(
    system: &SystemPaths,
    configured: Option<&str>,
) -> Result<PathBuf, AgentDirectoryError> {
    let configured = configured
        .map(str::trim)
        .filter(|configured| !configured.is_empty());
    let base = match configured {
        Some(configured) if configured.starts_with('~') => {
            let rest = configured.strip_prefix('~').unwrap_or("");
            let absolute_home = rest
                .strip_prefix('/')
                .or_else(|| rest.strip_prefix('\\'))
                .unwrap_or("");
            let has_separator = rest.starts_with('/') || rest.starts_with('\\');
            if rest.is_empty() || has_separator {
                let home = system
                    .home
                    .as_deref()
                    .filter(|path| path.is_absolute())
                    .ok_or(AgentDirectoryError::NoHome)?;
                if rest.is_empty() {
                    home.to_path_buf()
                } else {
                    home.join(absolute_home)
                }
            } else {
                return Err(AgentDirectoryError::Unresolvable {
                    configured: configured.to_string(),
                    reason: "another user's home is not a path this process can resolve",
                });
            }
        }
        Some(configured) => {
            let configured = Path::new(configured.trim());
            if configured.is_absolute() {
                configured.to_path_buf()
            } else {
                return Err(AgentDirectoryError::Unresolvable {
                    configured: configured.display().to_string(),
                    reason: "it is not absolute, so the Agent's own working directory would decide where its logs are",
                });
            }
        }
        None => {
            let root = match system.platform {
                Platform::Windows => system
                    .local_app_data
                    .as_deref()
                    .ok_or(AgentDirectoryError::NoLocalAppData)?
                    .join(APPLICATION_DIR)
                    .join(AGENT_COMPONENT_DIR),
                Platform::Darwin | Platform::Unix => system
                    .home
                    .as_deref()
                    .filter(|path| path.is_absolute())
                    .ok_or(AgentDirectoryError::NoHome)?
                    .join("Library")
                    .join("Application Support")
                    .join(APPLICATION_DIR)
                    .join(AGENT_COMPONENT_DIR),
            };
            root
        }
    };
    Ok(base.join(AGENT_DATA_LOGS_SUBDIR))
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
    system: &SystemPaths,
    environment: Environment,
    manifest_dir: &Path,
) -> Result<PathBuf, LogDirectoryError> {
    let directory = directory(system, environment, manifest_dir);
    if let Err(reason) = std::fs::create_dir_all(&directory) {
        return Err(LogDirectoryError {
            path: directory,
            reason,
        });
    }
    if let Err(reason) = probe_write(&directory) {
        return Err(LogDirectoryError {
            path: directory,
            reason,
        });
    }
    Ok(directory)
}

/// Prepare an already-selected fallback path without pretending it is the normal tree.
pub fn prepare_at(directory: &Path) -> Result<PathBuf, LogDirectoryError> {
    if let Err(reason) = std::fs::create_dir_all(directory) {
        return Err(LogDirectoryError {
            path: directory.to_path_buf(),
            reason,
        });
    }
    if let Err(reason) = probe_write(directory) {
        return Err(LogDirectoryError {
            path: directory.to_path_buf(),
            reason,
        });
    }
    Ok(directory.to_path_buf())
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

    fn system() -> SystemPaths {
        SystemPaths::from_parts(Platform::Darwin, Some(home()), None, PathBuf::from("/tmp"))
    }

    fn no_home_system() -> SystemPaths {
        SystemPaths::from_parts(Platform::Darwin, None, None, PathBuf::from("/tmp"))
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

    /// The arm an unset `data_dir` takes: the Agent's installed tree, spelled out
    /// rather than asserted as a suffix so the component directory is pinned too.
    #[test]
    fn an_unset_agent_data_dir_lands_in_the_agents_installed_tree() {
        let installed =
            PathBuf::from("/home/operator/Library/Application Support/WTMedia/Agent/logs");
        assert_eq!(
            agent_directory(&system(), None).expect("determinable"),
            installed
        );
        // The shipped default is the empty string, not an absent key, and the two
        // mean the same thing to the Agent (`(data_dir or "").strip()`).
        assert_eq!(
            agent_directory(&system(), Some("")).expect("determinable"),
            installed
        );
        assert_eq!(
            agent_directory(&system(), Some("   ")).expect("determinable"),
            installed,
            "the Agent strips the setting before using it, and so does this"
        );
    }

    /// The Agent's tree is not Desktop's, and the two names differ for real.
    ///
    /// A mirror that returned this module's own directory would show the Agent's
    /// page Desktop's files — which a shared parent would make look plausible.
    #[test]
    fn the_agent_tree_is_not_desktops_tree() {
        let desktop_logs = directory(&system(), Environment::Production, &manifest());
        let agent = agent_directory(&system(), None).expect("determinable");

        assert_ne!(desktop_logs, agent);
        assert!(
            desktop_logs.ends_with("Library/Logs/WTMedia/Desktop"),
            "Desktop logs keep the logging layout: {desktop_logs:?}"
        );
        assert!(
            agent.ends_with("Library/Application Support/WTMedia/Agent/logs"),
            "Agent logs stay inside its data root: {agent:?}"
        );
    }

    /// A configured data directory wins, and the logs sit beside it.
    ///
    /// Both `~` spellings Python's `expanduser` resolves: a leading `~/` and a
    /// bare `~`. The first is what the Agent's own config comment recommends, the
    /// second is what the same function would do with it.
    #[test]
    fn a_configured_agent_data_dir_puts_the_logs_beside_it() {
        assert_eq!(
            agent_directory(&system(), Some("/Volumes/Scratch/agent-data")).expect("determinable"),
            PathBuf::from("/Volumes/Scratch/agent-data/logs")
        );
        assert_eq!(
            agent_directory(&system(), Some("~/agent-data")).expect("determinable"),
            PathBuf::from("/home/operator/agent-data/logs")
        );
        assert_eq!(
            agent_directory(&system(), Some(" ~/agent-data ")).expect("determinable"),
            PathBuf::from("/home/operator/agent-data/logs")
        );
        assert_eq!(
            agent_directory(&system(), Some("~")).expect("determinable"),
            PathBuf::from("/home/operator/logs"),
            "a bare `~` is the home, and its logs subdirectory is the Agent's rule"
        );
    }

    /// Values the Agent would resolve against *its* working directory, or through
    /// another user's home, are refused rather than resolved here — and the
    /// refusal quotes the value back.
    #[test]
    fn a_data_dir_that_cannot_be_resolved_from_here_is_refused() {
        for configured in [
            "agent-data",
            "./agent-data",
            "../agent-data",
            "~someone/agent-data",
        ] {
            let refused = agent_directory(&system(), Some(configured))
                .expect_err("must not resolve a path this process cannot know");
            match &refused {
                AgentDirectoryError::Unresolvable {
                    configured: quoted, ..
                } => {
                    assert_eq!(quoted, configured.trim(), "{configured}")
                }
                other => panic!("expected an unresolvable value, got {other:?}"),
            }
            assert!(
                refused.to_string().contains(configured.trim()),
                "the message must name what was refused: {refused}"
            );
        }
    }

    /// Without a home the installed tree cannot be built, and that is a refusal
    /// rather than a path that starts without a root.
    ///
    /// The two arms that read the home are the ones that refuse; a configured
    /// absolute path does not need it and still answers, which is what makes this
    /// a per-input rule instead of a blanket "no HOME, no answer".
    #[test]
    fn no_home_refuses_the_arms_that_need_one() {
        assert!(matches!(
            agent_directory(&no_home_system(), None),
            Err(AgentDirectoryError::NoHome)
        ));
        assert!(matches!(
            agent_directory(&no_home_system(), Some("~/agent-data")),
            Err(AgentDirectoryError::NoHome)
        ));
        // A relative HOME is as unusable as none: it would resolve against this
        // process's working directory rather than the user's home.
        assert!(matches!(
            agent_directory(&no_home_system(), None),
            Err(AgentDirectoryError::NoHome)
        ));

        assert_eq!(
            agent_directory(&no_home_system(), Some("/Volumes/Scratch/agent-data"))
                .expect("needs no home"),
            PathBuf::from("/Volumes/Scratch/agent-data/logs"),
            "an absolute data directory is answerable without a home"
        );
    }

    /// The refusal is a refusal and not a fallback: what it must not do is quietly
    /// answer with the installed tree, which would list a directory that is not
    /// the one the Agent is writing to.
    #[test]
    fn the_refusal_does_not_fall_back_to_the_installed_tree() {
        let refused = agent_directory(&system(), Some("agent-data"));
        let installed = agent_directory(&system(), None).expect("determinable");

        assert!(
            matches!(&refused, Err(error) if !error.to_string().contains(&installed.display().to_string())),
            "a refusal must not name the tree it did not read: {refused:?}"
        );
    }

    #[test]
    fn production_writes_under_the_injected_home() {
        assert_eq!(
            directory(&system(), Environment::Production, &manifest()),
            PathBuf::from("/home/operator/Library/Logs/WTMedia/Desktop")
        );
    }

    #[test]
    fn development_writes_under_the_injected_manifest_directory() {
        assert_eq!(
            directory(&system(), Environment::Development, &manifest()),
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
        let production = directory(&system(), Environment::Production, &manifest());
        assert!(
            production.starts_with(home()),
            "production must be under the home it was given: {production:?}"
        );
        assert!(
            !production.starts_with(manifest()),
            "production must not be built under the manifest directory: {production:?}"
        );

        let development = directory(&system(), Environment::Development, &manifest());
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
        let path = directory(&system(), Environment::Production, &manifest());

        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(COMPONENT_DIR)
        );
        assert_eq!(
            path.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some(APPLICATION_DIR)
        );
    }

    #[test]
    fn prepare_creates_the_directory_and_reports_the_same_path() {
        let root = scratch("prepare");
        let expected = directory(&system(), Environment::Development, &root);

        let got =
            prepare(&system(), Environment::Development, &root).expect("writable scratch dir");

        assert_eq!(got, expected);
        assert!(got.is_dir(), "prepare must have created it: {got:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The second launch of the day must not fail because the first one created
    /// the directory.
    #[test]
    fn prepare_is_repeatable() {
        let root = scratch("repeat");

        prepare(&system(), Environment::Development, &root).expect("first");
        prepare(&system(), Environment::Development, &root).expect("second");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The probe leaves nothing behind for the budget scan to count.
    #[test]
    fn prepare_leaves_no_probe_file() {
        let root = scratch("clean");
        let directory = prepare(&system(), Environment::Development, &root).expect("writable");

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
        let target = prepare(&system(), Environment::Development, &root).expect("create it first");
        let stale = target.join(format!("{}{}", PROBE_PREFIX, std::process::id()));
        std::fs::write(&stale, b"left behind by a process that died mid-probe").expect("plant it");

        let got = prepare(&system(), Environment::Development, &root);

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
        let occupied = directory(&system(), Environment::Development, &root);
        std::fs::create_dir_all(occupied.parent().expect("parent")).expect("scratch parent");
        std::fs::write(&occupied, b"not a directory").expect("occupy the path");

        let error =
            prepare(&system(), Environment::Development, &root).expect_err("must not succeed");

        assert_eq!(
            error.path, occupied,
            "the error must name the path it could not use"
        );
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
        let target = prepare(&system(), Environment::Development, &root).expect("create it first");
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
            prepare(&system(), Environment::Development, &root).map(|_| ())
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

    fn darwin_system() -> SystemPaths {
        SystemPaths::from_parts(Platform::Darwin, Some(home()), None, PathBuf::from("/tmp"))
    }

    fn windows_system(local: &Path) -> SystemPaths {
        SystemPaths::from_parts(
            Platform::Windows,
            None,
            Some(local.to_path_buf()),
            PathBuf::from("/tmp"),
        )
    }

    #[test]
    fn windows_production_log_is_under_local_app_data() {
        assert_eq!(
            directory(
                &windows_system(Path::new("/local")),
                Environment::Production,
                &manifest()
            ),
            PathBuf::from("/local/WTMedia/Desktop/logs")
        );
    }

    #[test]
    fn windows_agent_log_defaults_to_the_agent_tree() {
        let got = agent_directory(&windows_system(Path::new("/local")), None)
            .expect("windows agent default");
        assert_eq!(got, PathBuf::from("/local/WTMedia/Agent/logs"));
    }

    #[test]
    fn windows_agent_log_missing_local_app_data_is_refused() {
        let system = SystemPaths::from_parts(Platform::Windows, None, None, PathBuf::from("/tmp"));
        assert!(matches!(
            agent_directory(&system, None),
            Err(AgentDirectoryError::NoLocalAppData)
        ));
    }
}
