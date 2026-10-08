//! What an upgrade may write, decided by **path**.
//!
//! T-07 of CHG-20260923-059 (acceptance AC-07). The proposition is negative —
//! *an upgrade does not write the operator's settings, the Agent's SQLite,
//! checkpoints or pending results* — and this build has no upgrade action to
//! change: `updater::Updater` and `filesystem::FileSystemBridge` are still
//! declared shells and nothing runs an updater. So what T-07 delivers is the
//! **criterion**: one declaration of the paths an upgrade may write, a refusal
//! for everything else, and the arms that pin both.
//!
//! That is why this module is `#[cfg(test)]`-only. A guard with no caller would
//! be a `dead_code` warning and a claim nothing acts on; a criterion that a test
//! pins is the honest shape of "prove a negative about an action that does not
//! exist yet". When an updater is built, this module is what it must call, and
//! the arms here are what its first version has to keep green.
//!
//! ## The rule is containment, not a name list
//!
//! The milestone forbids 「升级或清理删除业务数据」, and `cleanup` (CHG-C T-05)
//! answers its half the same way: *the protection is a path, not a name list —
//! a list has to stay right forever*. The same two failure modes decide this
//! module's shape, and both are arms below rather than sentences here:
//!
//! - **A name list is wrong by construction.** User data is not called
//!   `settings.toml`; it is *whatever is under the data root and is not a
//!   declared site*. A file the operator renamed, or one a later version will
//!   add, has to be protected without anyone remembering to extend a list.
//! - **A string prefix is not containment.** `…/WTMedia/Desktop-backup` and
//!   `…/WTMedia/Agent-old` begin with the same characters as the real roots and
//!   are not inside them. [`inside`] compares **path components**
//!   (`Path::starts_with`), which is the difference an arm exists to keep.
//!
//! ## The two roots, and why the Agent's is not the Desktop's business
//!
//! `app_paths` gives the Desktop four roots, and the Agent's data directory is
//! deliberately **not** one of them: the two components share a parent under
//! `Library/Application Support` (that is what `COMPONENT_DIR` is for), and the
//! Agent's `data_dir` holds the SQLite, the checkpoints and the pending results.
//! So the Desktop's upgrade surface has to be refused *there* by path — the
//! Agent's root, whatever is in it — rather than by recognising file names that
//! belong to another repository. [`agent_data_root`] mirrors the installed shape
//! the way `logging::paths::agent_directory` already mirrors the Agent's log
//! tree: from the Agent's own declaration, through the constants this side owns.
//!
//! **Scope, stated rather than implied.** In a checkout the two components live
//! in their own repositories and neither writes the other's `.local/`, so this
//! criterion covers the installed layout — the one an upgrade happens in. A path
//! outside both roots (the artifact being staged, a cache tree, a log tree) is
//! not user data and is not this module's business; `allows_a_path_outside_both_roots`
//! says so out loud so the scope cannot be read as wider than it is.

use crate::app_paths::AppPaths;
use crate::logging::paths::{AGENT_COMPONENT_DIR, APPLICATION_DIR};
use crate::settings;
use std::path::{Path, PathBuf};

/// The Agent's data root as the Agent installs it.
///
/// `~/Library/Application Support/WTMedia/Agent`, the data-side sibling of the
/// log tree `logging::paths::agent_directory` resolves — the same
/// `WTMedia/<Component>` shape, one directory over, from the Agent's own
/// `runtime/paths.py` (`INSTALLED_DATA_DIR`). Read-only from this side: the
/// Desktop's only use for it is to know what **not** to write.
pub fn agent_data_root(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join(APPLICATION_DIR)
        .join(AGENT_COMPONENT_DIR)
}

/// Which class of user data a refused path belongs to.
///
/// Named rather than numbered because the refusal is shown to whoever is
/// diagnosing an upgrade, and "which of the two roots" is the first thing that
/// tells them whether the fault is here or in the Agent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    /// A path under the Desktop's data root that is not a declared write site.
    DesktopUserData,
    /// Anything at all under the Agent's data root.
    AgentData,
}

impl Class {
    fn of(self) -> &'static str {
        match self {
            Class::DesktopUserData => "the Desktop's data root",
            Class::AgentData => "the Agent's data root",
        }
    }
}

/// Why a write was refused. Carries the path and the class, never any content —
/// the same rule `settings` and `config` hold for their own errors.
#[derive(Debug)]
pub struct Refusal {
    pub class: Class,
    pub path: PathBuf,
    /// The class's own description, for the sentence a caller prints.
    pub because: String,
}

/// The paths this Desktop's upgrade may write inside its own data root.
///
/// Two, and both come from the module that owns them rather than from a literal
/// here: the settings file from `settings::path` (the one writer of it is
/// `commands::settings`, which resolves it the same way) and the version/upgrade
/// payload area from `app_paths::Root::Versions`, which is *inside* the data root
/// by construction. Everything else under the data root is user data: §6.8 of the
/// architecture baseline lists the rest — the Agent's SQLite, the file index, the
/// checkpoints, the pending results, the runtime files.
pub fn desktop_write_sites(paths: &AppPaths) -> [PathBuf; 2] {
    [settings::path(&paths.data), paths.versions.clone()]
}

/// Decide whether `target` may be written on the upgrade path.
///
/// `paths` is the Desktop's own four roots; `agent_data` is
/// [`agent_data_root`] for the home in play. Both are injected for the same
/// reason `app_paths::directory` injects the home: a function that reads the
/// environment cannot be asked "what would you do with *this* home", and the
/// arms have to be able to ask.
pub fn refuse(target: &Path, paths: &AppPaths, agent_data: &Path) -> Result<(), Refusal> {
    // The Agent's root first: everything under it is user data to this side, and
    // the SQLite, the checkpoints and the pending results live in it.
    if inside(target, agent_data) {
        return Err(Refusal {
            class: Class::AgentData,
            path: target.to_path_buf(),
            because: format!(
                "it is under the Agent's data root {} — the Desktop's upgrade \
                 surface does not write there, whatever the file is called",
                agent_data.display()
            ),
        });
    }

    if inside(target, &paths.data)
        && !desktop_write_sites(paths)
            .iter()
            .any(|site| inside(target, site))
    {
        return Err(Refusal {
            class: Class::DesktopUserData,
            path: target.to_path_buf(),
            because: format!(
                "it is under the data root {} and is not a declared write site \
                 ({})",
                paths.data.display(),
                desktop_write_sites(paths)
                    .iter()
                    .map(|site| site.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }

    Ok(())
}

/// Is `target` inside `root`, compared **by component**?
///
/// `Path::starts_with` is the whole point: `/a/Desktop-backup` does not start
/// with `/a/Desktop` as paths, though it does as text.
pub fn inside(target: &Path, root: &Path) -> bool {
    target.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_paths::{self, Root};
    use crate::config::Environment;

    fn home() -> PathBuf {
        PathBuf::from("/home/operator")
    }

    fn manifest() -> PathBuf {
        PathBuf::from("/crate")
    }

    /// The Desktop's four roots, installed layout, for the operator above.
    fn installed() -> AppPaths {
        app_paths::resolve(&system(), Environment::Production, &manifest())
            .expect("a home is given")
    }

    fn system() -> crate::system_paths::SystemPaths {
        crate::system_paths::SystemPaths::from_parts(
            crate::system_paths::Platform::Darwin,
            Some(home()),
            None,
            manifest(),
        )
    }

    /// The Agent's data root for the same home.
    fn agent() -> PathBuf {
        agent_data_root(&home())
    }

    fn data_root() -> PathBuf {
        installed().data
    }

    fn refused(target: &Path) -> Refusal {
        refuse(target, &installed(), &agent()).expect_err("must be refused")
    }

    fn allowed(target: &Path) {
        if let Err(refusal) = refuse(target, &installed(), &agent()) {
            panic!(
                "{} must be writable on the upgrade path, but it was refused as {}: {}",
                target.display(),
                refusal.class.of(),
                refusal.because
            );
        }
    }

    /// The two declared sites are writable — the positive control.
    ///
    /// Without this, every arm below would also pass for a `refuse` that refused
    /// everything, and the criterion would be "an upgrade writes nothing", which
    /// is not the proposition: an upgrade does write the settings file and its
    /// payload area.
    #[test]
    fn the_declared_sites_are_writable() {
        let paths = installed();
        let [settings_file, versions] = desktop_write_sites(&paths);

        assert_eq!(settings_file, paths.data.join("settings.toml"));
        assert_eq!(versions, paths.data.join("versions"));

        allowed(&settings_file);
        allowed(&versions.join("0.2.2").join("payload.bin"));
    }

    /// A user-data path under the Desktop's data root is refused, by path.
    ///
    /// The file is named `notes.db`: nothing recognises that name, and it is
    /// still the operator's, so a criterion that only knows the names this code
    /// happens to write today lets it through.
    #[test]
    fn an_unrecognised_file_under_the_data_root_is_refused() {
        let refusal = refused(&data_root().join("notes.db"));

        assert_eq!(refusal.class, Class::DesktopUserData);
        assert_eq!(refusal.path, data_root().join("notes.db"));
    }

    /// The same for a directory: what is protected is the subtree, not a file.
    #[test]
    fn a_directory_under_the_data_root_is_refused() {
        let refusal = refused(&data_root().join("checkpoints").join("task-1.json"));

        assert_eq!(refusal.class, Class::DesktopUserData);
    }

    /// A path under the **Agent's** data root is refused whatever it is called.
    ///
    /// This is the arm a name list cannot pass: `settings.toml` under the Agent's
    /// root is the Agent's file — or a file the Agent will write later — and the
    /// only thing that makes the write wrong is *where* it is. The Agent's root is
    /// the one holding the SQLite, the checkpoints and the pending results.
    #[test]
    fn the_agent_root_is_refused_whatever_the_file_is_called() {
        for name in [
            "local-agent.sqlite3",
            "task_checkpoints.db",
            "settings.toml",
            "versions",
            "offline-results.jsonl",
        ] {
            let refusal = refused(&agent().join(name));
            assert_eq!(refusal.class, Class::AgentData, "{name}");
        }
    }

    /// A sibling directory that merely shares a prefix is **not** inside.
    ///
    /// The two roots are `…/WTMedia/Desktop` and `…/WTMedia/Agent`, and both a
    /// hand-made backup — `…/WTMedia/Desktop-backup`, `…/WTMedia/Agent-old` — and
    /// a sibling of a *write site* — `<data>/versions-backup` — share their
    /// characters. A criterion written as `starts_with` on a **string** treats
    /// those as inside; this one compares components, and the arm is what keeps
    /// it so.
    ///
    /// The direction that matters is the site's, and it is the one that is not
    /// symmetric: with a string prefix, `<data>/versions-backup/payload.bin`
    /// reads as inside the payload area ⇒ an upgrade writes *beside* it, where
    /// nothing will ever look for it or clean it up. So the site's sibling must be
    /// refused, and that is asserted here rather than left to the reading of
    /// [`inside`]. The root's siblings are outside both roots and therefore not
    /// this criterion's business: it does not claim them, so it does not get to
    /// refuse them either — the same scope line `allows_a_path_outside_both_roots`
    /// draws, asserted here so a prefix bug cannot hide behind it.
    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_inside() {
        let wtmedia = home()
            .join("Library")
            .join("Application Support")
            .join(APPLICATION_DIR);
        let paths = installed();
        let [_, versions] = desktop_write_sites(&paths);

        let beside_the_site = paths.data.join("versions-backup").join("payload.bin");
        assert!(
            !inside(&beside_the_site, &versions),
            "{} must not read as inside the payload area {}",
            beside_the_site.display(),
            versions.display()
        );
        assert_eq!(refused(&beside_the_site).class, Class::DesktopUserData);

        let backup = wtmedia.join("Desktop-backup").join("settings.toml");
        assert!(
            !inside(&backup, &data_root()),
            "{} must not read as inside {}",
            backup.display(),
            data_root().display()
        );
        allowed(&backup);

        let old_agent = wtmedia.join("Agent-old").join("local-agent.sqlite3");
        assert!(
            !inside(&old_agent, &agent()),
            "{} must not read as inside {}",
            old_agent.display(),
            agent().display()
        );
        allowed(&old_agent);
    }

    /// The two roots are siblings, and the Desktop's sites are outside the
    /// Agent's root.
    ///
    /// An upgrade that wrote "the settings file" into the shared parent would be
    /// writing into whichever component got there first. The arm holds the
    /// relationship that makes the refusals above meaningful: neither declared
    /// site is inside the Agent's root, and the two roots share a parent.
    #[test]
    fn the_two_roots_are_siblings_with_disjoint_sites() {
        let paths = installed();

        assert_eq!(data_root().parent(), agent().parent());
        assert_ne!(data_root(), agent());
        for site in desktop_write_sites(&paths) {
            assert!(!inside(&site, &agent()), "{}", site.display());
        }
        assert!(!inside(&paths.cache, &agent()), "{}", paths.cache.display());
    }

    /// Nothing is refused outside the two roots, and that is the criterion's
    /// scope said out loud.
    ///
    /// The artifact being staged is the clearest case: `stage-release-config.sh`
    /// writes `Contents/Resources/config/agent.toml` inside the bundle it is
    /// about to sign, and refusing that would be refusing the release. This arm
    /// exists so the scope cannot be read as "an upgrade writes nowhere".
    #[test]
    fn allows_a_path_outside_both_roots() {
        allowed(Path::new(
            "/Applications/WT Media.app/Contents/Resources/config/agent.toml",
        ));
        allowed(&installed().cache.join("thumbnail-1.bin"));
    }

    /// A checkout resolves the development layout, and the rule is the same one.
    ///
    /// In a checkout the Agent's root is not under the crate, so the second arm
    /// here is the honest limit of this criterion: the Desktop knows *its own*
    /// layout in both environments, and the Agent's data root only in the
    /// installed one.
    #[test]
    fn the_development_layout_gets_the_same_rule() {
        let system = crate::system_paths::SystemPaths::from_parts(
            crate::system_paths::Platform::Darwin,
            Some(home()),
            None,
            manifest(),
        );
        let paths = app_paths::resolve(&system, Environment::Development, &manifest())
            .expect("a checkout needs no home");
        let [settings_file, versions] = desktop_write_sites(&paths);

        assert!(settings_file.starts_with(manifest().join(".local").join("data")));
        assert!(refuse(&settings_file, &paths, &agent()).is_ok());
        assert!(refuse(&versions.join("0.2.2"), &paths, &agent()).is_ok());

        let refusal = refuse(
            &paths.data.join("checkpoints").join("task-1.json"),
            &paths,
            &agent(),
        )
        .expect_err("user data is user data in a checkout too");
        assert_eq!(refusal.class, Class::DesktopUserData);
    }

    /// The data root and the versions site come from `Root`, not from a literal
    /// repeated here.
    #[test]
    fn the_sites_are_the_roots_owner_declaration() {
        let paths = installed();
        let [_, versions] = desktop_write_sites(&paths);

        assert_eq!(
            versions,
            app_paths::directory(
                Root::Versions,
                &system(),
                Environment::Production,
                &manifest()
            )
        );
        assert!(inside(&versions, &paths.data));
    }
}
