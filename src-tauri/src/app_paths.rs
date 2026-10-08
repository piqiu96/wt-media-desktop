//! The four directories this app owns, and whether each can be written.
//!
//! This is a second module rather than an extension of `paths` because the two
//! have opposite failure contracts. `paths` locates *a config file* and promises
//! there is always an answer — a locator that could return "nothing" would be a
//! launch with no CSP and no Cloud address. Here, "where does your data live"
//! has to be a fact: a root that cannot be resolved is an error, and a root that
//! cannot be written is an error, because every caller of this module is about
//! to either write something a user cares about or delete something.
//!
//! ## The four roots
//!
//! - [`Root::Data`] holds what an upgrade must not lose. Today that is
//!   `settings.toml`; §6.8 of the architecture baseline lists the rest.
//! - [`Root::Versions`] lives *inside* the data root, which is the shape
//!   `RuntimePaths` gives it on the Agent side (`versions_dir = data_dir /
//!   "versions"`). One definition of where versions go, not two.
//! - [`Root::Logs`] is a root this module names but does not define: the log
//!   tree already has an owner in `logging::paths`, and a second rule computing
//!   the same path would be two rules nothing makes agree.
//! - [`Root::Cache`] is disposable by definition, and that is why it is **not**
//!   inside the data directory. The cleanup command's whole job is to delete
//!   files, and the milestone forbids "升级或清理删除业务数据". A cache root
//!   sharing a parent with `settings.toml` makes that prohibition depend on a
//!   whitelist staying right forever; a cache root in a different tree makes
//!   deleting a user's data require a wrong *path* rather than a wrong *list*.
//!   macOS agrees for its own reason: `~/Library/Caches` is excluded from Time
//!   Machine and may be purged by the system, which is what "regenerable" means.
//!
//! ## The layouts mirror the Agent's
//!
//! Installed: `~/Library/Application Support/WTMedia/Desktop`,
//! `~/Library/Logs/WTMedia/Desktop`, `~/Library/Caches/WTMedia/Desktop` — the
//! same `WTMedia/<Component>` shape `runtime/paths.py` uses with `Agent`.
//! Development: `<crate>/.local/{data,logs,cache}`, with `versions` under `data`.
//! A developer who has read one repository should not be surprised by the other.
//!
//! One asymmetry is inherited deliberately: installed, the data root *is* the
//! component directory, while in a checkout it is one level down (`.local/data`).
//! That is what `RuntimePaths` does, and matching it matters more here than a
//! tidier rule would.
//!
//! ## Which environment decides
//!
//! The **build's**, not `config.environment` — the same rule `logging::paths`
//! states and its tests pin. A release binary is production unconditionally, so
//! no config file can talk it into writing into a checkout; a development run
//! cannot reach the installed directories either.
//!
//! ## Pure, then impure
//!
//! [`directory`] is pure — a root, a home, an environment and a manifest
//! directory in; a path out — so the layout is testable without a filesystem or
//! a real `$HOME`. [`resolve`] is the caller-facing question ("all four, for
//! this process") and holds the one genuine failure: production without a
//! `HOME`. [`prepare`] is the only part that touches the filesystem, and it
//! answers a second question: *can this root actually be written to*.
//!
//! **Readers use [`directory`], writers use [`prepare`].** Creating a directory
//! is a side effect, and "show me how much cache is used" must not be the reason
//! a cache directory starts existing. The split is here so that distinction is
//! visible at every call site rather than remembered.
//!
//! Windows is **not** covered, following `logging::paths`: `~/Library/...` is a
//! macOS shape, no Windows layout has been measured, and a guess would be worse
//! than a registered gap.

use crate::config::Environment;
use crate::system_paths::{Platform, SystemPathError, SystemPaths};
use std::path::{Path, PathBuf};

/// The application directory under the user's `Library`.
pub const APPLICATION_DIR: &str = "WTMedia";

/// The component directory. Its sibling under the same parent is `Agent`, which
/// is why a component level exists at all: one shared directory where a cleanup
/// in one component could reach the other's files is the failure this prevents.
pub const COMPONENT_DIR: &str = "Desktop";

/// The development layout's first component, spelled as `logging::paths` does.
pub const DEVELOPMENT_DIR: &str = ".local";

/// The development data component. Installed, the data root is the component
/// directory itself; in a checkout it is one level down.
const DEVELOPMENT_DATA_DIR: &str = "data";

/// The prefix of the write-probe file. Dot-prefixed so it does not look like a
/// file a user owns, and carrying the pid so two processes cannot remove each
/// other's probe.
///
/// The same marker `logging::paths` uses, deliberately: someone reading any of
/// these directories should see one kind of dot-file rather than one per module.
/// The literal is in both modules; a rename in one of them changes only the name
/// of a leftover file, never a rule.
const PROBE_PREFIX: &str = ".wt-media-write-probe-";

/// One of the directories this app owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Root {
    /// What an upgrade must not lose.
    Data,
    /// Version and upgrade payloads. Inside [`Root::Data`].
    Versions,
    /// This component's log tree.
    Logs,
    /// Regenerable files: the only root whose contents may be deleted wholesale.
    Cache,
}

impl Root {
    /// The name a root goes by: its last component installed, and its component
    /// under `.local/` in a checkout.
    ///
    /// `Data`'s two names differ (`Desktop` installed, `data` here) because the
    /// layouts differ; the other three are spelled once for both.
    pub const fn name(self) -> &'static str {
        match self {
            Root::Data => "data",
            Root::Versions => "versions",
            Root::Logs => "logs",
            Root::Cache => "cache",
        }
    }
}

/// Where one root is, for a given system and build. Pure: creates nothing,
/// reads nothing, and does not consult the environment.
///
/// `SystemPaths` is injected rather than re-read per caller. A development
/// layout ignores it entirely; production reads only the platform's approved
/// system root, never `HOME`, `LOCALAPPDATA`, the executable directory, or cwd.
pub fn directory(
    root: Root,
    system: &SystemPaths,
    environment: Environment,
    manifest_dir: &Path,
) -> PathBuf {
    if environment == Environment::Development {
        return match root {
            Root::Logs => crate::logging::paths::directory(system, environment, manifest_dir),
            Root::Versions => {
                directory(Root::Data, system, environment, manifest_dir).join(Root::Versions.name())
            }
            Root::Data => manifest_dir
                .join(DEVELOPMENT_DIR)
                .join(DEVELOPMENT_DATA_DIR),
            Root::Cache => manifest_dir.join(DEVELOPMENT_DIR).join(Root::Cache.name()),
        };
    }

    let installed = match system.platform {
        Platform::Windows => system
            .local_app_data
            .as_deref()
            .unwrap_or_else(|| Path::new(""))
            .join(APPLICATION_DIR)
            .join(COMPONENT_DIR),
        Platform::Darwin | Platform::Unix => {
            let home = system.home.as_deref().unwrap_or_else(|| Path::new(""));
            match root {
                Root::Logs => {
                    return crate::logging::paths::directory(system, environment, manifest_dir)
                }
                Root::Versions => {
                    return directory(Root::Data, system, environment, manifest_dir)
                        .join(Root::Versions.name())
                }
                Root::Data => {
                    return home
                        .join("Library")
                        .join("Application Support")
                        .join(APPLICATION_DIR)
                        .join(COMPONENT_DIR)
                }
                Root::Cache => {
                    return home
                        .join("Library")
                        .join("Caches")
                        .join(APPLICATION_DIR)
                        .join(COMPONENT_DIR)
                }
            }
        }
    };

    match root {
        Root::Logs => crate::logging::paths::directory(system, environment, manifest_dir),
        Root::Versions => {
            directory(Root::Data, system, environment, manifest_dir).join(Root::Versions.name())
        }
        Root::Data => installed.join("data"),
        Root::Cache => installed.join("cache"),
    }
}

/// Why a set of roots could not be resolved.
#[derive(Debug)]
pub enum ResolveError {
    /// Production with no `HOME`. The installed layout is built *from* the home
    /// directory and there is nothing to substitute for it. A placeholder would
    /// produce a relative path — `Library/Logs/...` under whatever directory the
    /// process happens to be in — which is a silently wrong answer, and this
    /// module's contract is that the answer is either right or refused.
    NoHome,
    /// Windows production cannot invent the per-user tree without Local AppData.
    NoLocalAppData,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NoHome => {
                write!(
                    f,
                    "HOME is not set, so the installed layout has no directory"
                )
            }
            ResolveError::NoLocalAppData => {
                write!(
                    f,
                    "LOCALAPPDATA is not set, so the Windows user layout has no directory"
                )
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// All four roots for one process, resolved together.
///
/// Together rather than one at a time so a caller cannot resolve `data` under
/// one home and `cache` under another and have nothing notice. Holds no
/// credentials: these are directory paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    pub data: PathBuf,
    pub versions: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
}

/// Resolve all four roots. Does not touch the filesystem.
///
/// `home` is `None` when the process has no `HOME`, which only matters for the
/// installed layout: the development layout is built from the manifest directory
/// and never reads it, so a checkout still resolves with no `HOME` at all.
pub fn resolve(
    system: &SystemPaths,
    environment: Environment,
    manifest_dir: &Path,
) -> Result<AppPaths, ResolveError> {
    if environment == Environment::Production {
        system.production_base().map_err(|error| match error {
            SystemPathError::NoHome => ResolveError::NoHome,
            SystemPathError::NoLocalAppData => ResolveError::NoLocalAppData,
        })?;
    }

    Ok(AppPaths {
        data: directory(Root::Data, system, environment, manifest_dir),
        versions: directory(Root::Versions, system, environment, manifest_dir),
        logs: directory(Root::Logs, system, environment, manifest_dir),
        cache: directory(Root::Cache, system, environment, manifest_dir),
    })
}

/// Why a root could not be created and proved writable.
#[derive(Debug)]
pub struct DirectoryError {
    pub root: Root,
    pub path: PathBuf,
    pub reason: std::io::Error,
}

impl std::fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} directory {} is unusable: {}",
            self.root.name(),
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for DirectoryError {}

/// The named root, created if needed and proved writable. The one impure part.
///
/// Returns the directory it proved, so a caller never has to call [`directory`]
/// again and risk the two disagreeing.
///
/// Preparing one root never touches another: each is created lazily, on the
/// launch or the command that needs it. So a reader that must not create is
/// still free to resolve, and a launch that only writes settings does not leave
/// an empty cache behind. (`Root::Versions` is the exception that proves the
/// rule: it lives inside the data root, so creating it creates its parent too —
/// a fact about the filesystem rather than a second rule here.)
pub fn prepare(
    root: Root,
    system: &SystemPaths,
    environment: Environment,
    manifest_dir: &Path,
) -> Result<PathBuf, DirectoryError> {
    let path = directory(root, system, environment, manifest_dir);
    if let Err(reason) = std::fs::create_dir_all(&path) {
        return Err(DirectoryError { root, path, reason });
    }
    if let Err(reason) = probe_write(&path) {
        return Err(DirectoryError { root, path, reason });
    }
    Ok(path)
}

/// Create, then remove, one file — the only honest way to ask "writable?".
///
/// `create_dir_all` says nothing here: a directory that already exists returns
/// `Ok` whether or not it can be written to, so a read-only data root would be
/// reported as usable and the first real write — the one that matters, because
/// it is a user's settings — would be the one that discovered otherwise.
///
/// The probe is removed on success. If the process dies between the two calls a
/// small dot-file is left behind; nothing treats it as anything else, and it is
/// never mistaken for a log file by name.
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

    /// What the installed layout has to be, spelled out rather than derived from
    /// the implementation: data and versions under Application Support, logs
    /// under Logs, cache under Caches. Asserted as literal paths so a change to
    /// any component is a red test rather than a silently moved directory.
    #[test]
    fn the_installed_layout_is_the_architecture_baselines() {
        let root = PathBuf::from("/home/operator/Library");
        assert_eq!(
            directory(Root::Data, &system(), Environment::Production, &manifest()),
            root.join("Application Support")
                .join("WTMedia")
                .join("Desktop")
        );
        assert_eq!(
            directory(
                Root::Versions,
                &system(),
                Environment::Production,
                &manifest()
            ),
            root.join("Application Support")
                .join("WTMedia")
                .join("Desktop")
                .join("versions")
        );
        assert_eq!(
            directory(Root::Logs, &system(), Environment::Production, &manifest()),
            root.join("Logs").join("WTMedia").join("Desktop")
        );
        assert_eq!(
            directory(Root::Cache, &system(), Environment::Production, &manifest()),
            root.join("Caches").join("WTMedia").join("Desktop")
        );
    }

    /// A development run writes under its own crate and names every root by its
    /// last component, so the four are recognisable in a directory listing.
    #[test]
    fn the_development_layout_is_under_the_crates_local_directory() {
        let local = manifest().join(DEVELOPMENT_DIR);
        assert_eq!(
            directory(Root::Data, &system(), Environment::Development, &manifest()),
            local.join("data")
        );
        assert_eq!(
            directory(
                Root::Versions,
                &system(),
                Environment::Development,
                &manifest()
            ),
            local.join("data").join("versions")
        );
        assert_eq!(
            directory(Root::Logs, &system(), Environment::Development, &manifest()),
            local.join("logs")
        );
        assert_eq!(
            directory(
                Root::Cache,
                &system(),
                Environment::Development,
                &manifest()
            ),
            local.join("cache")
        );
    }

    /// The log root has exactly one owner.
    ///
    /// This module does not restate the log path, it delegates — so what has to
    /// be tested is the *agreement*, not the spelling. A delegation that quietly
    /// returned something else (a hand-built `Library/Logs/...` with the wrong
    /// component, say) is the failure this catches, and it is the reason this
    /// test compares two functions instead of asserting a literal.
    #[test]
    fn the_log_root_is_the_logging_modules_own_answer() {
        for environment in [Environment::Development, Environment::Production] {
            assert_eq!(
                directory(Root::Logs, &system(), environment, &manifest()),
                crate::logging::paths::directory(&system(), environment, &manifest()),
                "the log root must be logging::paths' answer, not a second rule"
            );
        }
    }

    /// Cache is a sibling of the data root, never a child of it.
    ///
    /// The cleanup command deletes the cache; the milestone forbids deleting
    /// business data. This is the assertion that makes those two facts
    /// structurally compatible rather than a matter of a whitelist being right:
    /// a cache wipe cannot reach settings or versions by walking downward,
    /// because there is nothing of the user's downward to reach.
    ///
    /// Versions is asserted the other way round in the same breath, because the
    /// pair is the point — the two must not both be "somewhere under there".
    #[test]
    fn cache_is_never_inside_the_data_root() {
        for environment in [Environment::Development, Environment::Production] {
            let data = directory(Root::Data, &system(), environment, &manifest());
            let versions = directory(Root::Versions, &system(), environment, &manifest());
            let cache = directory(Root::Cache, &system(), environment, &manifest());

            assert!(
                versions.starts_with(&data),
                "versions belongs inside the data root: {versions:?}"
            );
            assert!(
                !cache.starts_with(&data),
                "the cache must not be reachable by walking down from data: {cache:?}"
            );
        }
    }

    /// Neither log tree is inside a data root, in either layout.
    ///
    /// The diagnostic export reads **exactly** the two log trees and nothing
    /// else, which is what makes "the bundle holds no user media" a property of
    /// the layout rather than of a filter being right — the same argument
    /// `cache_is_never_inside_the_data_root` makes for the cleanup. This is the
    /// half that belongs to this module: *this* component's log root against
    /// *this* component's data root.
    ///
    /// The other half is the Agent's tree, which is **not** asserted to be
    /// disjoint and must not be: `logging::paths::agent_directory` puts it at
    /// `<agent data_dir>/logs` when that setting is present — inside the Agent's
    /// data root by the Agent's own design. What makes the export safe there is
    /// that it reads that one directory without descending, which
    /// `diagnostic::a_media_file_inside_a_subdirectory_of_the_tree_is_not_reachable`
    /// pins.
    #[test]
    fn the_log_root_is_never_inside_the_data_root() {
        for environment in [Environment::Development, Environment::Production] {
            let data = directory(Root::Data, &system(), environment, &manifest());
            let logs = directory(Root::Logs, &system(), environment, &manifest());

            assert!(
                !logs.starts_with(&data),
                "the log tree must not be reachable by walking down from data: {logs:?}"
            );
            if environment == Environment::Production {
                assert!(
                    !logs.starts_with(&manifest()),
                    "an installed layout must not keep its logs in a checkout: {logs:?}"
                );
            }
        }
    }

    /// Which input decides the layout, asserted as an *absence* on both sides.
    ///
    /// Shape alone cannot tell these apart: `directory` could read the home in
    /// production and the manifest directory in development by accident, and one
    /// test per layout would still pass if it read both and picked by luck. Each
    /// arm here is built from inputs that would show up in the wrong answer.
    #[test]
    fn each_layout_reads_one_input_and_ignores_the_other() {
        let roots = [Root::Data, Root::Versions, Root::Logs, Root::Cache];

        for root in roots {
            let installed = directory(root, &system(), Environment::Production, &manifest());
            assert!(
                installed.starts_with(home()),
                "installed {root:?} must be under the home it was given: {installed:?}"
            );
            assert!(
                !installed.starts_with(manifest()),
                "installed {root:?} must not be built under the manifest directory: {installed:?}"
            );

            let local = directory(root, &system(), Environment::Development, &manifest());
            assert!(
                local.starts_with(manifest()),
                "development {root:?} must be under the manifest directory: {local:?}"
            );
            assert!(
                !local.starts_with(home()),
                "development {root:?} must not be written into the real home: {local:?}"
            );
        }
    }

    /// Four roots means four paths.
    ///
    /// A mutant that returned the data root's path for `Versions` as well would
    /// satisfy every "starts with" assertion in this file; only distinctness
    /// catches it. Pairwise rather than "the set has four elements", so the
    /// failure names the two roots that collided.
    #[test]
    fn the_four_roots_are_distinct() {
        for environment in [Environment::Development, Environment::Production] {
            let roots = [Root::Data, Root::Versions, Root::Logs, Root::Cache];
            for (index, left) in roots.iter().enumerate() {
                for right in &roots[index + 1..] {
                    assert_ne!(
                        directory(*left, &system(), environment, &manifest()),
                        directory(*right, &system(), environment, &manifest()),
                        "{left:?} and {right:?} must not be the same directory"
                    );
                }
            }
        }
    }

    /// `resolve` is a convenience over `directory`, and this is what keeps it
    /// one: each field has to be that function's answer, not a parallel rule.
    #[test]
    fn resolve_gathers_the_four_roots_directory_reports() {
        for environment in [Environment::Development, Environment::Production] {
            let resolved = resolve(&system(), environment, &manifest()).expect("a home");
            assert_eq!(
                resolved,
                AppPaths {
                    data: directory(Root::Data, &system(), environment, &manifest()),
                    versions: directory(Root::Versions, &system(), environment, &manifest()),
                    logs: directory(Root::Logs, &system(), environment, &manifest()),
                    cache: directory(Root::Cache, &system(), environment, &manifest()),
                }
            );
        }
    }

    /// The one real failure: an installed layout with no home to build it under.
    ///
    /// The rejected alternative is asserted too, because "refuse" and "return a
    /// relative path" are the two arms of this decision and only one of them is
    /// acceptable: `Path::new("")` joined with `Library/...` produces a path that
    /// is not rooted anywhere, and a caller that wrote settings into it would
    /// depend on the process's working directory.
    #[test]
    fn an_installed_layout_without_a_home_is_an_error() {
        let error = resolve(&no_home_system(), Environment::Production, &manifest())
            .expect_err("production needs a home");
        assert!(matches!(error, ResolveError::NoHome));
        assert!(
            error.to_string().contains("HOME"),
            "the message must name what is missing: {error}"
        );
    }

    /// A checkout needs no `HOME` at all, and this is the arm that must not
    /// inherit the refusal above.
    #[test]
    fn a_development_layout_resolves_without_a_home() {
        assert_eq!(
            resolve(&system(), Environment::Development, &manifest()).expect("no home needed"),
            resolve(&system(), Environment::Development, &manifest()).expect("the same")
        );
    }

    /// A scratch directory of this test's own, named after the test binary's pid
    /// so two concurrent runs cannot collide, and removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-app-paths-{}-{}-{}",
            label,
            std::process::id(),
            // Enough separation for two tests in the same process, which share a
            // pid.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        path
    }

    #[test]
    fn prepare_creates_the_root_and_reports_the_same_path() {
        let root = scratch("prepare");
        for kind in [Root::Data, Root::Versions, Root::Logs, Root::Cache] {
            let expected = directory(kind, &system(), Environment::Development, &root);
            let got = prepare(kind, &system(), Environment::Development, &root)
                .unwrap_or_else(|error| panic!("a writable scratch root: {error}"));
            assert_eq!(got, expected);
            assert!(got.is_dir(), "prepare must have created it: {got:?}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// Preparing one root must not drag the others into existence.
    ///
    /// Lazy creation is the reason a reader may resolve without writing: a
    /// launch that only needs settings must not leave an empty cache directory
    /// behind, and `Root::Versions` — which lives inside `Root::Data` — must not
    /// appear just because its parent did. The assertion is on the *absence* of
    /// the three untouched roots, which is also why it runs on the data root:
    /// data is the parent of versions, so it is the case where a "create the
    /// whole family" implementation would look most plausible.
    #[test]
    fn preparing_one_root_does_not_create_the_others() {
        let root = scratch("lazy");
        prepare(Root::Data, &system(), Environment::Development, &root).expect("data");

        for kind in [Root::Versions, Root::Logs, Root::Cache] {
            let path = directory(kind, &system(), Environment::Development, &root);
            assert!(
                !path.exists(),
                "{kind:?} must not have been created: {path:?}"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The second launch of the day must not fail because the first one created
    /// the directory.
    #[test]
    fn prepare_is_repeatable() {
        let root = scratch("repeat");
        prepare(Root::Data, &system(), Environment::Development, &root).expect("first");
        prepare(Root::Data, &system(), Environment::Development, &root).expect("second");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The probe leaves nothing behind for a cleanup scan to count.
    #[test]
    fn prepare_leaves_no_probe_file() {
        let root = scratch("clean");
        let directory = prepare(Root::Data, &system(), Environment::Development, &root)
            .expect("a writable scratch root");

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
    /// the root look unusable.
    ///
    /// The probe is two syscalls, and dying between them leaves the file behind.
    /// `prepare` therefore has to tolerate finding its own file. With
    /// `create_new` instead of `create`, a launch that reused the pid of a
    /// crashed process would report a perfectly good data root as unusable — and
    /// only on the first launch after the crash, which is the launch most likely
    /// to be investigated.
    #[test]
    fn a_stale_probe_file_does_not_make_the_root_unusable() {
        let root = scratch("stale");
        let target = prepare(Root::Data, &system(), Environment::Development, &root)
            .expect("create it first");
        let stale = target.join(format!("{}{}", PROBE_PREFIX, std::process::id()));
        std::fs::write(&stale, b"left behind by a process that died mid-probe").expect("plant it");

        let got = prepare(Root::Data, &system(), Environment::Development, &root);

        let still_there = stale.exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(
            got.is_ok(),
            "a stale probe file must not make the root unusable: {got:?}"
        );
        assert!(!still_there, "the probe removes its own file, stale or not");
    }

    /// A target that is not a directory at all is an `Err`, not a panic.
    ///
    /// Made reachable by a file occupying the path rather than by a permissions
    /// trick, following the precedent in `paths.rs`: this behaves the same for a
    /// root user and in CI, whereas a mode bit does not.
    ///
    /// The *kind* is asserted, not merely "some error": `create_dir_all` on a
    /// path occupied by a file reports `AlreadyExists`, whereas the write probe
    /// reaching through the same path would report `NotADirectory`. A mutant that
    /// swallowed the first error and let the second stand is caught here rather
    /// than looking equivalent.
    #[test]
    fn a_root_occupied_by_a_file_is_an_error_rather_than_a_panic() {
        let root = scratch("occupied");
        let occupied = directory(Root::Cache, &system(), Environment::Development, &root);
        std::fs::create_dir_all(occupied.parent().expect("parent")).expect("scratch parent");
        std::fs::write(&occupied, b"not a directory").expect("occupy the path");

        let error = prepare(Root::Cache, &system(), Environment::Development, &root)
            .expect_err("must not succeed");

        assert_eq!(
            error.root,
            Root::Cache,
            "the error must name the root it could not use"
        );
        assert_eq!(error.path, occupied, "and the path it could not use");
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
    fn a_root_that_exists_but_cannot_be_written_is_an_error() {
        let root = scratch("readonly");
        let target = prepare(Root::Data, &system(), Environment::Development, &root)
            .expect("create it first");
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
            std::fs::remove_file(target.join("premise-probe")).ok();
        }

        let outcome = if premise_held {
            prepare(Root::Data, &system(), Environment::Development, &root).map(|_| ())
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
            Ok(()) => panic!("an unwritable data root must not be reported as usable"),
            Err(error) => {
                assert_eq!(error.root, Root::Data);
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

    fn darwin_system(home: &Path) -> SystemPaths {
        SystemPaths::from_parts(
            Platform::Darwin,
            Some(home.to_path_buf()),
            None,
            PathBuf::from("/tmp"),
        )
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
    fn windows_installed_layout_uses_local_app_data() {
        let system = windows_system(Path::new("/local"));
        assert_eq!(
            directory(Root::Data, &system, Environment::Production, &manifest()),
            PathBuf::from("/local/WTMedia/Desktop/data")
        );
        assert_eq!(
            directory(
                Root::Versions,
                &system,
                Environment::Production,
                &manifest()
            ),
            PathBuf::from("/local/WTMedia/Desktop/data/versions")
        );
        assert_eq!(
            directory(Root::Logs, &system, Environment::Production, &manifest()),
            PathBuf::from("/local/WTMedia/Desktop/logs")
        );
        assert_eq!(
            directory(Root::Cache, &system, Environment::Production, &manifest()),
            PathBuf::from("/local/WTMedia/Desktop/cache")
        );
    }

    #[test]
    fn windows_resolve_refuses_without_local_app_data() {
        let system = SystemPaths::from_parts(Platform::Windows, None, None, PathBuf::from("/tmp"));
        assert!(matches!(
            resolve(&system, Environment::Production, &manifest()),
            Err(ResolveError::NoLocalAppData)
        ));
    }

    #[test]
    fn macos_resolve_keeps_the_home_layout() {
        let system = darwin_system(&home());
        let got = resolve(&system, Environment::Production, &manifest()).expect("mac paths");
        assert_eq!(
            got.data,
            PathBuf::from("/home/operator/Library/Application Support/WTMedia/Desktop")
        );
        assert_eq!(
            got.logs,
            PathBuf::from("/home/operator/Library/Logs/WTMedia/Desktop")
        );
        assert_eq!(
            got.cache,
            PathBuf::from("/home/operator/Library/Caches/WTMedia/Desktop")
        );
    }
}
