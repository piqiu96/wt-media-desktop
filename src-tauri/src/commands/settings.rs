//! The 本机设置 page's two commands over the operator's own settings file.
//!
//! Thin, like the other command modules: the rules are `settings`'s (what the
//! file holds, how it is replaced, what a corruption means) and this module wires
//! them to the page. The two things it adds are the two a page needs and the
//! module deliberately does not do:
//!
//! 1. **The chosen directory is checked before it is stored**
//!    ([`settings::check_save_dir`]) — a value the person can be told about now,
//!    rather than a task that fails to write an hour later.
//! 2. **The data root is created before the first write.** `settings::save` does
//!    not create it (its header says why), and a first launch that has only ever
//!    stored a choice has nothing else that would. The **read** path resolves
//!    without creating: a page that is only being looked at must not leave a
//!    directory behind, which is the same line `commands::storage` draws.
//!
//! ## Why this module resolves its own directory
//!
//! `commands::storage::resolve` answers 「两棵日志树在哪」, and this module's only
//! directory is the data root — one root, not two, and the one difference that
//! matters is that writing prepares it. So the resolution is here and is a
//! function of the same three inputs, and
//! `the_data_root_is_the_one_the_storage_commands_use` holds the two answers
//! together rather than trusting that they agree.
//!
//! ## A file that cannot be read is reported, not repaired
//!
//! Both commands are `Err` for an unreadable, unparsable or foreign-schema file,
//! naming the path and never the contents. The **write** refuses too, because
//! `settings::save` reads before it writes: a caller that ignored the error would
//! otherwise replace a settings file it could not read with the defaults, which
//! is the 静默清空 the milestone forbids. The page is the escape hatch — it shows
//! the file's path, and `commands::reveal` can open the folder it is in, so the
//! one step that can throw settings away is a person's.

use crate::app_paths::{self, Root};
use crate::bootstrap;
use crate::config::Environment;
use crate::dto::SettingsView;
use crate::settings::{self, UserSettings};
use std::path::{Path, PathBuf};

/// The three inputs the layout is decided by, read once each.
///
/// Read here rather than passed in because a `#[tauri::command]` may not consult
/// the environment itself without becoming untestable — the rules below take them
/// as arguments, and this is the one place that looks.
fn layout() -> (Option<PathBuf>, Environment, &'static Path) {
    (
        std::env::var_os("HOME").map(PathBuf::from),
        bootstrap::build_environment(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
}

/// The data root as it is, without touching the filesystem.
fn read_root(
    home: Option<&Path>,
    environment: Environment,
    manifest: &Path,
) -> Result<PathBuf, String> {
    app_paths::resolve(home, environment, manifest)
        .map(|paths| paths.data)
        .map_err(|error| format!("无法确定设置目录：{error}"))
}

/// The data root, created if needed and proved writable.
///
/// Returns the directory it proved rather than the one it was asked for, so the
/// caller cannot write into a path that was never checked.
fn write_root(
    home: Option<&Path>,
    environment: Environment,
    manifest: &Path,
) -> Result<PathBuf, String> {
    // An installed layout with no `HOME` is refused by `directory` itself; the
    // empty path stands in for 「there is none」 in the development layout, which
    // never reads it.
    app_paths::prepare(
        Root::Data,
        home.unwrap_or(Path::new("")),
        environment,
        manifest,
    )
    .map_err(|error| format!("无法准备设置目录：{error}"))
}

/// The settings file this page reads and writes.
fn file_of(root: &Path) -> PathBuf {
    settings::path(root)
}

/// What is stored now, or why it could not be read.
fn read(root: &Path) -> Result<SettingsView, String> {
    let file = file_of(root);
    let stored = settings::load(&file).map_err(|error| format!("读取设置失败：{error}"))?;
    Ok(view(&file, &stored))
}

/// Replace the choice, then answer with what is on disk.
///
/// Read back rather than echoed: the answer to 「现在是什么」is the file, and a
/// report built from the value that was handed in cannot tell a successful write
/// from one that a serialization detail quietly changed.
fn write(root: &Path, save_dir: Option<&str>) -> Result<SettingsView, String> {
    let chosen = match save_dir {
        None => None,
        Some(raw) => {
            let path = PathBuf::from(raw);
            settings::check_save_dir(&path).map_err(|problem| {
                format!("保存位置 {} 不能用：{}", path.display(), problem.message())
            })?;
            Some(path)
        }
    };

    let file = file_of(root);
    // Read-modify-write, not build-and-write. The file holds more than the
    // current choice — it holds the **history** of directories this user has
    // used — and a struct built from this function's argument alone would drop
    // that history on every save: change the directory twice and the one last
    // week's files are still in would be gone by the second save.
    //
    // The load is not the write's permission check (`settings::save` reads for
    // itself, and refuses a file it cannot parse). It is here because the value
    // being modified has to be read before it can be modified — and it runs
    // *after* `check_save_dir`, so a directory nobody could use is refused
    // without the file being opened at all.
    let mut stored = settings::load(&file).map_err(|error| format!("读取设置失败：{error}"))?;
    stored.remember(chosen);
    settings::save(&file, &stored).map_err(|error| format!("保存设置失败：{error}"))?;
    read(root)
}

/// The wire shape of one stored state.
///
/// `display()` rather than a UTF-16 or percent-encoded form: a path that got here
/// was parsed out of a TOML string, so it is text, and the page shows it to a
/// person who has to recognise their own directory.
fn view(file: &Path, stored: &UserSettings) -> SettingsView {
    SettingsView {
        save_dir: stored
            .save_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        file: file.display().to_string(),
    }
}

/// The operator's stored choice of save directory, or `None` when there is none.
///
/// The inner half takes the root so it can be exercised without a layout; the
/// outer one is what `commands::downloads` calls through `commands::agent`'s
/// start, and it resolves the root **without creating it** — reading is not
/// writing, the same line `local_settings_get` draws.
///
/// A second reader of the one file rather than a cached copy: the operator may
/// edit it by hand while the app is running, and the two readers disagreeing is
/// exactly the state that would make Desktop push a folder it no longer offers.
pub(crate) fn stored_save_dir(root: &Path) -> Result<Option<String>, String> {
    let file = file_of(root);
    let stored = settings::load(&file).map_err(|error| format!("读取设置失败：{error}"))?;
    Ok(view(&file, &stored).save_dir)
}

/// [`stored_save_dir`] against the real data root.
pub(crate) fn chosen_save_dir() -> Result<Option<String>, String> {
    let (home, environment, manifest) = layout();
    let root = read_root(home.as_deref(), environment, manifest)?;
    stored_save_dir(&root)
}

/// [`known_save_dirs`] against a given root, so it can be exercised without one.
///
/// The same split [`stored_save_dir`] makes, and for the same reason: the rules
/// are worth testing and the layout is not.
pub(crate) fn known_dirs_of(root: &Path) -> Result<Vec<PathBuf>, String> {
    let file = file_of(root);
    let stored = settings::load(&file).map_err(|error| format!("读取设置失败：{error}"))?;
    Ok(stored.search_dirs())
}

/// Every directory this machine has written downloads into, newest first.
///
/// The other question the one file answers. [`chosen_save_dir`] is 「新文件写到
/// 哪儿」— one directory, and a `None` when nothing is chosen. This is 「这个文件
/// 可能在哪些地方」, which is a different question with a different answer: it
/// outlives the choice (clearing it does not forget where the files are) and it
/// is non-empty whenever anything was ever downloaded.
///
/// Resolved through `read_root`, so asking does not create the data root — the
/// same line `local_settings_get` draws, and the reason 「打开文件」 can be
/// pressed on a machine where the page was never opened.
pub(crate) fn known_save_dirs() -> Result<Vec<PathBuf>, String> {
    let (home, environment, manifest) = layout();
    let root = read_root(home.as_deref(), environment, manifest)?;
    known_dirs_of(&root)
}

/// Replace the stored choice, against the real data root.
///
/// The composition the two writers share: `local_settings_set`, which takes a
/// value from the page, and the folder picker, which takes one from a dialog.
/// One function rather than two so the picker's value goes through the **same**
/// `check_save_dir` — a second path into the settings file is a second chance for
/// a directory the tasks cannot use to be stored.
pub(crate) fn set_save_dir(save_dir: Option<&str>) -> Result<SettingsView, String> {
    let (home, environment, manifest) = layout();
    let root = write_root(home.as_deref(), environment, manifest)?;
    write(&root, save_dir)
}

/// Read the operator's settings.
#[tauri::command]
pub fn local_settings_get() -> Result<SettingsView, String> {
    let (home, environment, manifest) = layout();
    let root = read_root(home.as_deref(), environment, manifest)?;
    read(&root)
}

/// Replace the operator's chosen save location.
///
/// `None` clears the choice rather than selecting a directory: 「没有选择」is a
/// state the file can hold, and a page needs a way back to it.
#[tauri::command]
pub fn local_settings_set(save_dir: Option<String>) -> Result<SettingsView, String> {
    set_save_dir(save_dir.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, PRODUCTION_TOML};
    use std::collections::BTreeMap;

    /// A scratch directory of this test's own, removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-settings-cmd-{}-{}-{}",
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

    /// A directory the test may point the setting at, and a file it may not.
    fn pickable(root: &Path) -> (PathBuf, PathBuf) {
        let wanted = root.join("Movies");
        std::fs::create_dir_all(&wanted).expect("a directory to pick");
        let a_file = root.join("a-file");
        std::fs::write(&a_file, b"x").expect("a file to mistake for one");
        (wanted, a_file)
    }

    /// A first launch: nothing chosen, the defaults, and **no file created by
    /// looking**.
    ///
    /// The last part is the one a mutation can break silently: a `load` that
    /// wrote the defaults out would leave a settings file on every machine that
    /// ever opened the page, and the next schema change would have a file to
    /// migrate that nobody ever filled in.
    #[test]
    fn a_first_launch_reports_nothing_chosen_and_writes_nothing() {
        let root = scratch("fresh");
        let got = read(&root).expect("the defaults");
        let created = file_of(&root).exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(got.save_dir, None);
        assert_eq!(got.file, file_of(&root).display().to_string());
        assert!(!created, "reading must not create the settings file");
    }

    /// A choice round-trips, and the file says which schema wrote it.
    #[test]
    fn a_saved_choice_comes_back() {
        let root = scratch("roundtrip");
        let (wanted, _) = pickable(&root);

        let written = write(&root, Some(&wanted.display().to_string())).expect("save");
        let text = std::fs::read_to_string(file_of(&root)).expect("the file");
        let read_back = read(&root).expect("read");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(written.save_dir, Some(wanted.display().to_string()));
        assert_eq!(read_back, written, "the answer and the file disagree");
        assert!(
            text.contains(&format!("schema_version = {}", settings::SCHEMA_VERSION)),
            "{text}"
        );
    }

    /// Clearing the choice is a value the file can hold, not a deletion of the
    /// file: the schema version stays, and `save_dir` goes.
    ///
    /// Asserted by **parsing** rather than by searching the text. 「文件里没有
    /// save_dir 这几个字」 was the older form of this test, and it stopped being the
    /// same claim the moment `known_save_dirs` existed: that key contains the
    /// substring, so a text search would fail on a file that is exactly right —
    /// and, worse, would have gone on passing if the key had been dropped too.
    #[test]
    fn clearing_the_choice_leaves_a_readable_file() {
        let root = scratch("cleared");
        let (wanted, _) = pickable(&root);
        write(&root, Some(&wanted.display().to_string())).expect("save a choice");

        let cleared = write(&root, None).expect("clear");
        let stored = settings::load(&file_of(&root)).expect("the file is still readable");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(cleared.save_dir, None);
        assert_eq!(
            stored.save_dir, None,
            "an unchosen directory is an absent key, not an empty one"
        );
        assert_eq!(
            stored.known_save_dirs,
            vec![wanted],
            "but the directory the files are already in stays known — clearing is not forgetting"
        );
    }

    /// The second choice does not erase the first.
    ///
    /// This is the whole reason the file keeps a history: the material
    /// downloaded while the first directory was current is still sitting in it,
    /// and 「打开文件」 has to be able to look there.
    #[test]
    fn a_second_choice_keeps_the_first_one_known() {
        let root = scratch("history");
        let (first, _) = pickable(&root);
        let second = root.join("Pictures");
        std::fs::create_dir_all(&second).expect("a second directory to pick");

        write(&root, Some(&first.display().to_string())).expect("the first");
        let written = write(&root, Some(&second.display().to_string())).expect("the second");
        let stored = settings::load(&file_of(&root)).expect("read back");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(written.save_dir, Some(second.display().to_string()));
        assert_eq!(stored.save_dir, Some(second.clone()));
        assert_eq!(
            stored.known_save_dirs,
            vec![second, first],
            "newest first, and the older one is still there to be searched"
        );
    }

    /// A v1 file is upgraded by the next save rather than refused by the write.
    ///
    /// The file a user already has is the case this exists for: refusing it would
    /// make 「换个下载目录」 impossible on the first launch after an update, and
    /// silently overwriting it would lose the directory their files are in.
    #[test]
    fn a_v1_file_is_upgraded_by_the_next_save_with_its_directory_kept() {
        let root = scratch("v1");
        let file = file_of(&root);
        let (wanted, _) = pickable(&root);
        std::fs::write(
            &file,
            format!("schema_version = 1\nsave_dir = \"{}\"\n", wanted.display()),
        )
        .expect("plant a v1 file");

        let moved = root.join("Pictures");
        std::fs::create_dir_all(&moved).expect("a new directory to pick");

        let written = write(&root, Some(&moved.display().to_string())).expect("the write");
        let text = std::fs::read_to_string(&file).expect("the file");
        let stored = settings::load(&file).expect("read back");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(written.save_dir, Some(moved.display().to_string()));
        assert!(
            text.contains(&format!("schema_version = {}", settings::SCHEMA_VERSION)),
            "the next save is what upgrades the file: {text}"
        );
        assert_eq!(
            stored.known_save_dirs,
            vec![moved, wanted],
            "and the directory the v1 file knew is not lost on the way"
        );
    }

    /// A value the task could not use is refused, **and nothing is written**.
    ///
    /// Both spellings, and both halves: the error is about this machine being
    /// able to use the place, and the file is still absent afterwards. A refusal
    /// that had already written the half-usable choice would be worse than no
    /// check at all.
    #[test]
    fn a_save_dir_that_cannot_be_used_is_refused_before_anything_is_written() {
        let root = scratch("refused");
        let (_, a_file) = pickable(&root);

        let relative = write(&root, Some("Movies/WTMedia"));
        let absent = write(&root, Some(&root.join("nowhere").display().to_string()));
        let is_a_file = write(&root, Some(&a_file.display().to_string()));
        let created = file_of(&root).exists();
        std::fs::remove_dir_all(&root).ok();

        for (label, result) in [
            ("relative", relative),
            ("absent", absent),
            ("a file", is_a_file),
        ] {
            let error = result.expect_err(label);
            assert!(
                error.contains("不能用"),
                "{label}: the message must say it was refused: {error}"
            );
        }
        assert!(!created, "a refused value must not reach the file");
    }

    /// A file this build cannot read is reported, and **left byte-identical** —
    /// by the write as well as by the read.
    ///
    /// The write half is the one that matters: `settings::save` reads first, so a
    /// page that ignored the read error and saved anyway cannot quietly replace a
    /// file it could not understand with the defaults.
    #[test]
    fn a_file_that_cannot_be_read_is_reported_and_left_alone() {
        let root = scratch("corrupt");
        let file = file_of(&root);
        // One past what this build implements, spelled as an expression rather
        // than as a literal. It was `2` while 2 was foreign, and bumping the
        // schema turned this into a valid file that the write would happily
        // replace — the test would have kept passing its other assertions while
        // asserting nothing.
        let original = format!(
            "save_dir = \"/Users/operator/Movies\"\nschema_version = {}\n",
            settings::SCHEMA_VERSION + 1
        );
        std::fs::write(&file, &original).expect("plant a foreign schema");

        let read_error = read(&root).expect_err("the read must fail");
        let write_error = write(&root, Some("/tmp")).expect_err("the write must fail");
        let after = std::fs::read_to_string(&file).expect("still there");
        std::fs::remove_dir_all(&root).ok();

        assert!(read_error.contains("schema_version"), "{read_error}");
        assert!(write_error.contains("schema_version"), "{write_error}");
        assert_eq!(after, original, "the file must be left exactly as it was");
    }

    /// Neither error repeats what the file said.
    ///
    /// `toml`'s own `Display` renders the offending line, and this file's values
    /// are paths — which is content. The span-free message is what is kept, and
    /// this is the case that would notice if a `Display` were quoted instead.
    #[test]
    fn an_error_never_echoes_the_files_contents() {
        let root = scratch("echo");
        let file = file_of(&root);
        // A path that must not come back in an error message, in a file the parse
        // will reject anyway (the schema is one this build does not implement).
        let secret = "/Users/operator/Private/hunter2";
        std::fs::write(
            &file,
            format!("save_dir = \"{secret}\"\nschema_version = 7\n"),
        )
        .expect("plant");

        let read_error = read(&root).expect_err("must fail");
        let write_error = write(&root, Some("/tmp")).expect_err("must fail");
        std::fs::remove_dir_all(&root).ok();

        for error in [read_error, write_error] {
            assert!(!error.contains("hunter2"), "the contents leaked: {error}");
        }
    }

    /// The parse failure a person can produce by hand is reported as a parse
    /// failure, not as a missing file.
    #[test]
    fn an_unparsable_file_is_reported_rather_than_treated_as_empty() {
        let root = scratch("unparsable");
        std::fs::write(file_of(&root), b"save_dir = [").expect("plant");

        let error = read(&root).expect_err("must fail");
        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("读取设置失败"), "{error}");
        assert!(
            !error.contains("未设置"),
            "a broken file is not an unchosen one: {error}"
        );
    }

    /// The two readers of the one file are one answer.
    ///
    /// `stored_save_dir` is the narrowed one `commands::downloads` pushes to the
    /// Agent; `read` is what the page shows. Two readers that disagreed would let
    /// Desktop push a folder the operator is no longer being shown — and nothing
    /// else in the tree compares them.
    #[test]
    fn the_narrowed_reader_agrees_with_the_pages() {
        let root = scratch("narrowed");
        let (wanted, _) = pickable(&root);
        write(&root, Some(&wanted.display().to_string())).expect("save a choice");

        let narrowed = stored_save_dir(&root).expect("the narrowed reader");
        let page = read(&root).expect("the page's reader");
        write(&root, None).expect("clear");
        let cleared = stored_save_dir(&root).expect("the narrowed reader");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(narrowed, page.save_dir, "two answers to 「选了哪个目录」");
        assert_eq!(cleared, None, "a cleared choice is no choice");
    }

    /// The search space leads with the directory new files go to.
    ///
    /// The two readers of the one file, held together: 「往哪儿写」 is the first
    /// answer to 「去哪儿找」. If they could disagree, 「打开文件」 would look in a
    /// folder the operator is no longer using while skipping the one the page
    /// shows — and nothing else in the tree compares them.
    #[test]
    fn the_search_space_leads_with_the_directory_in_use() {
        let root = scratch("searchspace");
        let (first, _) = pickable(&root);
        let second = root.join("Pictures");
        std::fs::create_dir_all(&second).expect("a second directory to pick");

        write(&root, Some(&first.display().to_string())).expect("the first");
        write(&root, Some(&second.display().to_string())).expect("the second");
        let space = known_dirs_of(&root).expect("the search space");
        let in_use = stored_save_dir(&root).expect("the directory in use");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(space.first(), Some(&second), "newest first");
        assert_eq!(space, vec![second, first], "and the older one is not lost");
        assert_eq!(
            space.first().map(|path| path.display().to_string()),
            in_use,
            "the two readers disagree about which directory is in use"
        );
    }

    /// Nothing chosen and nothing ever chosen is an empty search space.
    ///
    /// Not an error: there is nowhere to look, which is a fact about a machine
    /// that has never downloaded anything — and 「还没有选择下载保存位置」 is the
    /// sentence the caller turns it into.
    #[test]
    fn a_machine_that_never_chose_has_an_empty_search_space() {
        let root = scratch("nosearch");
        let space = known_dirs_of(&root);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            space.expect("no file is not an error"),
            Vec::<PathBuf>::new()
        );
    }

    /// The two roots — this module's and the storage commands' — are one answer.
    ///
    /// `commands::storage` resolves the same data root for the storage page, and
    /// two resolutions that disagreed would mean the page showed a cached size
    /// for one directory while the settings file lived in another. Asserted
    /// rather than assumed, because nothing else would notice.
    #[test]
    fn the_data_root_is_the_one_the_storage_commands_use() {
        let config = load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Production)
            .expect("the shipped config");

        let theirs = crate::commands::storage::resolve(&config)
            .expect("the storage resolver")
            .paths
            .data;

        let (home, environment, manifest) = layout();
        let mine = read_root(home.as_deref(), environment, manifest).expect("this module's root");

        assert_eq!(mine, theirs, "two answers to 「设置文件在哪」");
    }

    /// Writing creates the data root; reading does not.
    ///
    /// The asymmetry is the point of having two functions, and it is the
    /// difference between a first launch that can store a choice and a page that
    /// leaves an empty `Application Support` tree behind because somebody looked
    /// at it.
    #[test]
    fn writing_prepares_the_data_root_and_reading_does_not() {
        let scratch_root = scratch("roots");
        // A scratch manifest directory, so the development layout's data root is
        // this test's own and not the checkout's.
        let manifest = scratch_root.join("crate");
        std::fs::create_dir_all(&manifest).expect("a stand-in manifest dir");

        let resolved =
            read_root(Some(&scratch_root), Environment::Development, &manifest).expect("resolve");
        let created_by_read = resolved.exists();
        let prepared =
            write_root(Some(&scratch_root), Environment::Development, &manifest).expect("prepare");
        let created_by_write = prepared.is_dir();

        std::fs::remove_dir_all(&scratch_root).ok();

        assert_eq!(resolved, prepared, "the two must name the same directory");
        assert!(!created_by_read, "resolving must not create the root");
        assert!(created_by_write, "preparing must create the root");
    }
}
