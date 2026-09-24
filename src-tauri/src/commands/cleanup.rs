//! The two cleanup commands: 清缓存 and 清理旧日志.
//!
//! Both are thin. The rules are in `cleanup` (which deletes) and in
//! `commands::storage::resolve` (which says where the directories are), and this
//! module only wires them to the page — the same split `commands::storage` uses,
//! for the same reason: a command takes `State`, which no test can build.
//!
//! ## What a caller may decide
//!
//! Neither command takes a path. The cache command takes nothing at all, and the
//! log command takes a **source label** (`"desktop"` or `"agent"`, the same
//! spelling `local_log_tail` takes), which selects between two directories the
//! app resolved for itself. So the page chooses *which of this app's things* to
//! clean, never *which directory on the disk*.
//!
//! The log command cleans **one** tree. T-05's read commands cover both at once
//! (`Q-08`), but reading the Agent's tree and deleting from it are different
//! acts: the Agent's archives are the Agent's, and a person who wants them gone
//! can say so for that tree. A single button that did both would be this module
//! deciding, on the user's behalf, that another component's history is its to
//! delete.
//!
//! ## An unreadable tree fails the command; a file that will not go is in the report
//!
//! The same split `cleanup` draws, carried out to the wire: a tree that cannot be
//! listed is an `Err` (nothing is known, and "0 files removed" would be the 0 MB
//! lie), while a single failed removal is a [`CleanupFailureFact`] beside the
//! counts of what did happen.

use crate::app_paths::AppPaths;
use crate::cleanup::{self, CleanupOutcome};
use crate::commands::storage::resolve;
use crate::config::DesktopConfig;
use crate::dto::{CleanupFailureFact, CleanupReport, KeptFileFact};
use crate::logging::reader::Source;
use std::path::Path;
use tauri::State;

/// The page's spelling of a source, or an error naming what is understood.
///
/// No default: a command that fell back to Desktop would delete from a tree the
/// caller did not name, which is worse than telling them the value was wrong —
/// the same rule `local_log_tail` follows.
fn source_of(label: &str) -> Result<Source, String> {
    Source::from_label(label)
        .ok_or_else(|| format!("未知的日志来源 {label:?}：只认识 desktop 与 agent"))
}

/// One outcome as the page sees it.
fn report(label: &str, directory: &Path, outcome: CleanupOutcome) -> CleanupReport {
    CleanupReport {
        label: label.to_string(),
        directory: directory.display().to_string(),
        freed_bytes: outcome.freed_bytes,
        files_removed: outcome.files_removed,
        directories_removed: outcome.directories_removed,
        kept: outcome
            .kept
            .into_iter()
            .map(|kept| KeptFileFact {
                name: kept.name,
                reason: kept.reason.as_str().to_string(),
            })
            .collect(),
        failures: outcome
            .failures
            .into_iter()
            .map(|failure| CleanupFailureFact {
                path: failure.path.display().to_string(),
                reason: failure.reason.to_string(),
            })
            .collect(),
    }
}

/// Empty this component's cache root, reporting what went.
fn cache_report(paths: &AppPaths) -> Result<CleanupReport, String> {
    let outcome = cleanup::empty_regenerable_root(&paths.cache)
        .map_err(|error| format!("清理缓存失败: {error}"))?;
    Ok(report("cache", &paths.cache, outcome))
}

/// Remove one log tree's rotated archives, reporting what went.
fn log_report(directory: &Path, source: Source) -> Result<CleanupReport, String> {
    let outcome = cleanup::remove_rotated_archives(directory, source)
        .map_err(|error| format!("清理日志 {} 失败: {error}", directory.display()))?;
    Ok(report(source.label(), directory, outcome))
}

/// Delete the cache's contents. The cache root itself is kept.
#[tauri::command]
pub fn local_cache_cleanup(config: State<'_, DesktopConfig>) -> Result<CleanupReport, String> {
    let resolved = resolve(&config)?;
    cache_report(&resolved.paths)
}

/// Delete one log tree's rotated archives. The live file is never touched.
#[tauri::command]
pub fn local_log_cleanup(
    config: State<'_, DesktopConfig>,
    source: String,
) -> Result<CleanupReport, String> {
    let resolved = resolve(&config)?;
    let source = source_of(&source)?;
    log_report(&resolved.tree(source), source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_paths::{self, Root};
    use crate::config::Environment;
    use crate::logging::paths as log_paths;
    use std::path::PathBuf;

    /// A scratch directory of this test's own, removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-cleanup-cmd-{}-{}-{}",
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

    fn plant(path: &Path, bytes: usize) -> u64 {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, vec![b'x'; bytes]).expect("plant");
        bytes as u64
    }

    /// Four roots of this test's own, in the shape `AppPaths` has.
    fn paths_under(base: &Path) -> AppPaths {
        AppPaths {
            data: base.join("data"),
            versions: base.join("data/versions"),
            logs: base.join("logs"),
            cache: base.join("cache"),
        }
    }

    /// The business and runtime categories the milestone names, each planted in
    /// the tree it lives in: five under the **data** root, and the sixth — the
    /// file being written — in the log tree.
    ///
    /// The names are the point: `docs/product/prd` and the architecture baseline
    /// call these 素材、成片、SQLite、检查点、待回传结果, and a cleanup that reached
    /// them would be deleting exactly what the milestone forbids.
    fn plant_the_five_in(data: &Path) -> Vec<(&'static str, PathBuf)> {
        [
            ("素材", data.join("downloads/素材-001.mp4")),
            ("成片", data.join("outputs/成片-001.mp4")),
            ("SQLite", data.join("state.db")),
            ("检查点", data.join("checkpoints/task-7.json")),
            ("待回传结果", data.join("pending/result-9.json")),
        ]
        .into_iter()
        .map(|(label, path)| {
            plant(&path, 100);
            (label, path)
        })
        .collect()
    }

    /// The cache command frees exactly what was in the cache, keeps the root, and
    /// leaves the data root alone.
    ///
    /// The premise is one run, not two: the five categories are planted in the
    /// same base directory as the cache, and the assertion is that the cache's
    /// files went while theirs did not. So a cleanup that did nothing fails this
    /// test, and so does one that reached a directory it was not pointed at.
    #[test]
    fn the_cache_command_empties_the_cache_and_not_the_data_root() {
        let base = scratch("cache-report");
        let paths = paths_under(&base);
        let cache_file = plant(&paths.cache.join("blobs/chunk-1.bin"), 4096);
        let five = plant_the_five_in(&paths.data);

        let report = cache_report(&paths).expect("a readable cache");

        let mut survivors = Vec::new();
        for (label, path) in &five {
            survivors.push((*label, path.exists()));
        }
        let cache_root_survives = paths.cache.is_dir();
        let leftovers = std::fs::read_dir(&paths.cache)
            .expect("the cache root stays")
            .count();
        std::fs::remove_dir_all(&base).ok();

        assert_eq!(report.label, "cache");
        assert_eq!(report.directory, paths.cache.display().to_string());
        assert_eq!(report.freed_bytes, cache_file);
        assert_eq!(report.files_removed, 1);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(cache_root_survives, "the cache root itself is kept");
        assert_eq!(leftovers, 0, "and nothing is left inside it");
        for (label, survived) in survivors {
            assert!(
                survived,
                "{label} lives in the data root and this command cannot reach it"
            );
        }
    }

    /// The log command removes that tree's archives, never its live file, and
    /// never a name it does not recognise.
    ///
    /// The three leftovers are the whole point of the report: a person who
    /// pressed 清理旧日志 and sees one file still listed is owed the reason.
    #[test]
    fn the_log_command_removes_archives_and_reports_what_it_kept() {
        let base = scratch("log-report");
        let tree = base.join("logs");
        let live = tree.join("desktop.log");
        std::fs::create_dir_all(&tree).expect("the tree");
        std::fs::write(&live, b"2026-09-24T21:00:00 [INFO] desktop.startup: hi\n").expect("plant");
        let first = plant(&tree.join("desktop.log.2026-09-24-19"), 120);
        let second = plant(&tree.join("desktop.log.2026-09-24-20"), 80);
        plant(&tree.join("desktop-20260924-1.log"), 64);

        let report = log_report(&tree, Source::Desktop).expect("a readable tree");

        let live_survives = live.exists();
        let stray_survives = tree.join("desktop-20260924-1.log").exists();
        std::fs::remove_dir_all(&base).ok();

        assert_eq!(report.label, "desktop");
        assert_eq!(report.directory, tree.display().to_string());
        assert_eq!(report.freed_bytes, first + second);
        assert_eq!(report.files_removed, 2);
        assert_eq!(
            report.directories_removed, 0,
            "a log tree is never the cleanup's to remove"
        );
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(live_survives, "正在写入文件永不删");
        assert!(stray_survives, "a name this app did not write is not ours");
        let mut kept: Vec<(String, String)> = report
            .kept
            .into_iter()
            .map(|kept| (kept.name, kept.reason))
            .collect();
        kept.sort();
        assert_eq!(
            kept,
            vec![
                (
                    "desktop-20260924-1.log".to_string(),
                    "unrecognised".to_string()
                ),
                ("desktop.log".to_string(), "live".to_string()),
            ]
        );
    }

    /// A removal that failed keeps its path and the system's own words.
    ///
    /// The report is the only place a person learns that pressing the button did
    /// not do everything, so a failure that stays inside `cleanup` and never
    /// crosses the wire is the same as no failure at all — with a cheerful
    /// "已释放 0 字节" on top.
    #[test]
    fn a_failed_removal_reaches_the_page() {
        use std::os::unix::fs::PermissionsExt;

        let base = scratch("failure-report");
        let paths = paths_under(&base);
        let locked = paths.cache.join("locked");
        plant(&locked.join("stuck.bin"), 40);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).expect("chmod");

        let report = cache_report(&paths).expect("a readable cache");

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).ok();
        let survived = locked.join("stuck.bin").exists();
        std::fs::remove_dir_all(&base).ok();

        if !survived {
            eprintln!(
                "premise failed: this process deleted a file in a 0o500 directory \
                 (running as root?), so the failing-removal case was not exercised"
            );
            return;
        }
        assert_eq!(
            report.freed_bytes, 0,
            "nothing was freed, so nothing is claimed"
        );
        assert_eq!(report.files_removed, 0);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        assert!(
            report.failures[0].path.ends_with("stuck.bin"),
            "the failing file is named: {:?}",
            report.failures
        );
        assert!(
            !report.failures[0].reason.is_empty(),
            "and the operating system's own reason comes with it"
        );
    }

    /// A source the reader does not know is refused rather than defaulted.
    #[test]
    fn an_unknown_source_is_an_error_rather_than_a_default() {
        assert_eq!(source_of("desktop").expect("known"), Source::Desktop);
        assert_eq!(source_of("agent").expect("known"), Source::Agent);
        for unknown in ["", "Desktop", "cache", "..", "logs"] {
            let refused = source_of(unknown);
            assert!(
                refused.is_err(),
                "{unknown:?} must not select a tree: {refused:?}"
            );
        }
        // The control: the two labels that *are* understood are accepted by the
        // same function, so "everything is refused" cannot pass this test.
        assert!(source_of("agent").is_ok());
    }

    /// The five categories are outside every directory these commands can name,
    /// in both layouts — the property that makes the protection a path rather
    /// than a list.
    ///
    /// Denominator: 5 categories × 3 reachable roots × 2 layouts. The positive
    /// control is the same predicate on a path that *is* reachable: it must say
    /// "inside", so a predicate that answered `false` for everything could not
    /// pass.
    #[test]
    fn the_five_categories_are_outside_every_directory_a_cleanup_can_name() {
        let home = Path::new("/home/operator");
        let manifest = Path::new("/checkout/src-tauri");
        let agent_data = "/home/operator/agent-data";

        let relatives = [
            "downloads/素材-001.mp4",
            "outputs/成片-001.mp4",
            "state.db",
            "checkpoints/task-7.json",
            "pending/result-9.json",
        ];

        let mut checked = 0_usize;
        for environment in [Environment::Production, Environment::Development] {
            let paths = app_paths::resolve(Some(home), environment, manifest).expect("the layout");
            let agent_logs =
                log_paths::agent_directory(Some(home), Some(agent_data)).expect("the Agent's tree");
            let reachable = [&paths.cache, &paths.logs, &agent_logs];

            for relative in relatives {
                let in_data = paths.data.join(relative);
                for root in reachable {
                    assert!(
                        !in_data.starts_with(root),
                        "{relative} in the data root must not be reachable by a cleanup \
                         pointed at {}: {}",
                        root.display(),
                        in_data.display()
                    );
                    checked += 1;
                }
            }

            // The positive control: a path that *is* under a reachable root is
            // reported as such by the same comparison.
            let reachable_file = paths.cache.join("blobs/chunk-1.bin");
            assert!(
                reachable_file.starts_with(&paths.cache),
                "the control must be inside the cache root"
            );
            // And the layout itself puts the cache outside the data root, which
            // is what `app_paths::cache_is_never_inside_the_data_root` pins from
            // that side.
            assert!(!paths.cache.starts_with(&paths.data));
            assert_eq!(Root::Cache.name(), "cache");
        }

        assert_eq!(checked, 5 * 3 * 2, "every category was compared");
    }
}
