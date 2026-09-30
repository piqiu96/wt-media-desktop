//! Opening one of this app's own folders in the machine's file manager.
//!
//! The page asks for a **place this app resolves**, never for a path. `place` is
//! a label from a closed vocabulary ([`Place`]), and the directory is the one the
//! launch already uses for that label — the same move `commands::storage` makes
//! when `local_log_tail` takes a file *name* and looks it up in the listing
//! rather than joining it onto a directory. A command that took a path would be a
//! general 「打开任意位置」primitive reachable from the WebView, and the page has no
//! need of one: the two log trees and the data root are the folders a person can
//! be sent to, and each already has a name here.
//!
//! The two log labels are **literally** `logging::reader::Source`'s words
//! (`"desktop"`, `"agent"`), so the viewer passes the same string to
//! `local_log_tail` and to this command. `"data"` is the one addition, and it is
//! what the 本机设置 page uses to show where `settings.toml` lives — the escape
//! hatch for a settings file this build cannot read, which only a person can
//! throw away.
//!
//! ## A folder that is not there yet
//!
//! Reported, not created. `app_paths::prepare` and `logging::paths::prepare` own
//! creating directories, and this command is not a writer; a viewer opened before
//! the first record was ever written would otherwise leave an empty log tree
//! behind as a side effect of a click. The message says which directory and what
//! creates it.
//!
//! ## The one line that cannot be tested
//!
//! [`reveal`] spawns the platform's opener through the `open` crate and returns.
//! Whether the folder then appears in front of the person is not something a test
//! can assert, and a test that spawned it would open a window on whatever machine
//! ran the suite. So the tested half is everything up to the spawn — the label
//! vocabulary, the resolution, and the missing-directory refusal — and the spawn
//! is a registered boundary in the T-08 evidence, checked by hand on this machine.

use crate::commands::storage::{resolve, Resolved};
use crate::config::DesktopConfig;
use crate::logging::reader::Source;
use std::path::{Path, PathBuf};
use tauri::State;

/// A folder this app can be asked to show.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Place {
    /// One component's log tree.
    Log(Source),
    /// The data root — where `settings.toml`, the versions and the material live.
    Data,
}

impl Place {
    /// The page's spelling, or an error naming what is understood.
    ///
    /// `None` rather than a default, for the reason `Source::from_label` gives:
    /// falling back to the Desktop tree would show a person a folder they did not
    /// ask for and let them believe it was the one they did.
    pub fn from_label(label: &str) -> Option<Place> {
        match label.trim() {
            "data" => Some(Place::Data),
            other => Source::from_label(other).map(Place::Log),
        }
    }

    /// What the page calls this place.
    pub const fn label(self) -> &'static str {
        match self {
            Place::Log(source) => source.label(),
            Place::Data => "data",
        }
    }
}

/// Every label the page may send, in the order the message lists them.
///
/// The refusal below is built from this list rather than from a literal, so the
/// vocabulary and the sentence about it cannot drift: a fourth place would make
/// the message name it without anyone remembering to edit the string.
pub const PLACES: [Place; 3] = [
    Place::Log(Source::Desktop),
    Place::Log(Source::Agent),
    Place::Data,
];

/// The directory a place names, from the launch's own resolution.
///
/// Takes the [`Resolved`] the read and cleanup commands use rather than resolving
/// again: the folder this command opens is then, by construction, the folder the
/// viewer listed and the cleanup counted.
fn directory_of(resolved: &Resolved, place: Place) -> PathBuf {
    match place {
        Place::Log(source) => resolved.tree(source),
        Place::Data => resolved.paths.data.clone(),
    }
}

/// The page's label, or an error naming the ones that are understood.
fn place_of(label: &str) -> Result<Place, String> {
    Place::from_label(label).ok_or_else(|| {
        let known: Vec<&str> = PLACES.iter().map(|place| place.label()).collect();
        format!("未知的位置 {label:?}：只认识 {}", known.join("/"))
    })
}

/// Hand one directory to the platform's file manager.
///
/// The wheel: `open` handles the three platforms' differences (the `open`
/// binary, `explorer`, `xdg-open`) rather than a `match` of our own that no test
/// on this machine could check beyond the one arm it runs.
///
/// Split out of [`reveal`] because it is the whole of what a page asking for a
/// **downloaded** file's folder needs (`commands::downloads::local_reveal_saved_file`),
/// and that command has no 「还不存在」refusal to make: the directory it opens is
/// the one a file was just found in. Two copies of the `open::that` line would be
/// two answers to 「这台机器怎么打开文件夹」.
pub(crate) fn open_directory(directory: &Path) -> Result<(), String> {
    open::that(directory).map_err(|error| format!("无法打开 {}：{error}", directory.display()))
}

/// Hand one of this app's own folders to the file manager, refusing one that is
/// not there.
///
/// Returns the directory it opened, so the page can say which one — the same
/// 「报告做成了什么」rule the cleanup and export commands follow. The `is_dir`
/// check is this module's, not [`open_directory`]'s: a log tree or the data root
/// that does not exist yet is a state the page has a sentence for, while a
/// directory a download was just found in cannot be missing.
fn reveal(directory: &Path) -> Result<String, String> {
    if !directory.is_dir() {
        return Err(format!(
            "{} 还不存在：启动一次应用、或执行一次任务之后它才会有内容",
            directory.display()
        ));
    }
    open_directory(directory)?;
    Ok(directory.display().to_string())
}

/// Show one of this app's folders in the file manager.
#[tauri::command]
pub fn local_open_place(config: State<'_, DesktopConfig>, place: String) -> Result<String, String> {
    let resolved = resolve(&config)?;
    reveal(&directory_of(&resolved, place_of(&place)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_paths::{self, AppPaths};
    use crate::config::Environment;
    use std::path::PathBuf;

    fn resolved() -> Resolved {
        Resolved {
            paths: AppPaths {
                data: PathBuf::from("/scratch/data"),
                versions: PathBuf::from("/scratch/data/versions"),
                logs: PathBuf::from("/scratch/logs"),
                cache: PathBuf::from("/scratch/cache"),
            },
            agent_logs: PathBuf::from("/scratch/agent-logs"),
        }
    }

    /// Every label round-trips, and no two places share one.
    ///
    /// Distinctness is the property: two places spelled the same would mean one
    /// of them could never be asked for, and the failure would look like a button
    /// that opens the wrong folder.
    ///
    /// `PLACES` is the list the refusal message is built from, and it is written
    /// by hand — a fourth variant would have to be added here. What this test
    /// holds is that the list and the vocabulary agree, in both directions, for
    /// every entry the list has.
    #[test]
    fn every_place_has_its_own_label() {
        let labels: Vec<&str> = PLACES.iter().map(|place| place.label()).collect();

        let distinct: std::collections::BTreeSet<&str> = labels.iter().copied().collect();
        assert_eq!(distinct.len(), PLACES.len(), "{labels:?}");

        for place in PLACES {
            assert_eq!(
                Place::from_label(place.label()),
                Some(place),
                "{} did not come back",
                place.label()
            );
        }
    }

    /// The two log labels are the reader's own words, not a copy of them.
    ///
    /// The viewer hands the same string to `local_log_tail` and to this command;
    /// if the two vocabularies drifted, a click would open a folder other than the
    /// one being read, and both commands would look like they had worked.
    #[test]
    fn the_log_labels_are_the_ones_the_read_commands_take() {
        for source in [Source::Desktop, Source::Agent] {
            let label = source.label();
            assert_eq!(
                Place::from_label(label),
                Some(Place::Log(source)),
                "{label}"
            );
            // The other side of the same fact, asserted where it lives: the
            // reader parses this word into the same source.
            assert_eq!(Source::from_label(label), Some(source), "{label}");
        }
    }

    /// A path is not a place, in any spelling.
    ///
    /// The whole point of the vocabulary is that the page cannot name a
    /// directory, so the shapes that would name one are enumerated rather than
    /// assumed: absolute, climbing, and a Windows-style drive root.
    #[test]
    fn a_path_is_not_a_place() {
        for label in [
            "/etc",
            "/Users/operator/Library/Logs/WTMedia/Desktop",
            "../..",
            "logs/desktop",
            "~",
            "C:\\Windows",
            "file:///etc",
        ] {
            assert_eq!(Place::from_label(label), None, "{label:?}");
            let error = place_of(label).expect_err(label);
            assert!(error.contains("只认识"), "{error}");
            // The refusal lists the vocabulary it has, every word of it: a
            // message that named two of three places would send a person looking
            // for a fourth spelling.
            for place in PLACES {
                assert!(error.contains(place.label()), "{error}");
            }
        }
    }

    /// Blank and shouted spellings are refused; surrounding space is not a
    /// spelling at all.
    ///
    /// Case is not ignored on either side of the product — the token comes from a
    /// component, not from a person — so `Desktop` is not a second way to say
    /// `desktop`. Trimming *is* inherited from `Source::from_label`, and the two
    /// spellings that only differ by space are asserted as accepted so that the
    /// two vocabularies are provably one rule.
    #[test]
    fn an_unknown_or_shouted_label_is_refused() {
        for label in ["", "   ", "Desktop", "AGENT", "DATA"] {
            assert_eq!(Place::from_label(label), None, "{label:?}");
        }

        assert_eq!(
            Place::from_label(" desktop "),
            Some(Place::Log(Source::Desktop))
        );
        assert_eq!(Place::from_label("\tdata\n"), Some(Place::Data));
    }

    /// Each place resolves to the directory the other commands use.
    ///
    /// The two log trees are [`Resolved::trees`]'s, in that order, and the third
    /// is the data root — the same one `commands::settings` writes `settings.toml`
    /// into. Asserted against `Resolved` rather than against literals so that a
    /// change to what the resolver returns moves this with it.
    #[test]
    fn each_place_is_the_directory_the_other_commands_use() {
        let resolved = resolved();

        assert_eq!(
            directory_of(&resolved, Place::Log(Source::Desktop)),
            resolved.paths.logs
        );
        assert_eq!(
            directory_of(&resolved, Place::Log(Source::Agent)),
            resolved.agent_logs,
            "the Agent's tree is not the Desktop's"
        );
        assert_eq!(directory_of(&resolved, Place::Data), resolved.paths.data);
        // And the data root is the one the settings file is written under.
        assert_eq!(
            crate::settings::path(&directory_of(&resolved, Place::Data)),
            resolved.paths.data.join("settings.toml")
        );
    }

    /// The resolver and this module agree about where the roots are.
    ///
    /// The stub above is this test's own invention; this one goes through
    /// `app_paths` and `logging::paths`, so the field names it fills are the ones
    /// the real resolver fills. A swap of `logs` and `cache` in `Resolved` would
    /// pass the test above and fail here.
    #[test]
    fn the_places_are_the_roots_app_paths_resolves() {
        let home = Path::new("/Users/operator");
        let manifest = Path::new("/build/src-tauri");
        let paths = app_paths::resolve(Some(home), Environment::Production, manifest)
            .expect("an installed layout resolves");
        let agent_logs = crate::logging::paths::agent_directory(Some(home), None)
            .expect("the Agent's tree resolves without a data_dir");
        let resolved = Resolved {
            paths: paths.clone(),
            agent_logs: agent_logs.clone(),
        };

        assert_eq!(directory_of(&resolved, Place::Data), paths.data);
        assert_eq!(
            directory_of(&resolved, Place::Log(Source::Desktop)),
            paths.logs
        );
        assert_eq!(
            directory_of(&resolved, Place::Log(Source::Agent)),
            agent_logs
        );
    }

    /// A folder that is not there is reported, and nothing is opened.
    ///
    /// This is the last thing that runs before the spawn, so it is also the only
    /// refusal a test can reach: a missing directory must be an `Err` rather than
    /// an `open` call on a path that does not exist.
    #[test]
    fn a_directory_that_is_not_there_is_reported() {
        let root =
            std::env::temp_dir().join(format!("wt-media-reveal-absent-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();

        let error = reveal(&root).expect_err("must not be opened");

        assert!(error.contains("还不存在"), "{error}");
        assert!(error.contains(&root.display().to_string()), "{error}");
        assert!(!root.exists(), "the refusal must not create it");
    }

    /// And the one that is there gets as far as the spawn — which is as far as a
    /// test may go.
    ///
    /// **Not run**: this asserts the branch that opens a window, and it is
    /// `#[ignore]`d for that reason. It exists so the refusal above is not the
    /// only shape covered, and so the boundary is a named case rather than a
    /// sentence in a comment. Run it by hand with
    /// `cargo test -- --ignored reveal_opens`.
    #[test]
    #[ignore = "opens a Finder window on the machine that runs it"]
    fn reveal_opens_a_directory_that_is_there() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let directory = crate::logging::paths::agent_directory(home.as_deref(), None)
            .expect("the Agent's tree resolves");

        let opened = reveal(&directory).expect("the tree exists on this machine");

        assert_eq!(opened, directory.display().to_string());
    }
}
