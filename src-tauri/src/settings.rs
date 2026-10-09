//! User settings: what the operator changes, where it lives, and how it is replaced.
//!
//! This is the second of the three configuration classes the launch-engineering
//! program separates, and the one that belongs to the *user* rather than to the
//! build. The deployment file (`resources/desktop.production.toml`, located by
//! `paths`) says how this installation behaves and is not writable by the app;
//! the sensitive and per-run context (tokens, credentials, `WT_MEDIA_*`) is
//! environment and memory only. A value a person edits on the 本机设置 page goes
//! here and nowhere else — the three do not share a file, which is what makes
//! "an upgrade must not lose the user's settings" a property of one small file
//! rather than of a merge.
//!
//! ## Where it lives, and who creates the directory
//!
//! [`path`] places it under the data root [`crate::app_paths::Root::Data`]
//! resolves — `~/Library/Application Support/WTMedia/Desktop/settings.toml`
//! installed, `<crate>/.local/data/settings.toml` in a checkout. [`save`] does
//! **not** create that directory: creating directories is `app_paths::prepare`'s
//! job, and a settings writer that silently made a directory tree would hide a
//! data root that is missing for a reason worth reporting.
//!
//! ## Corruption is reported, never repaired
//!
//! Three shapes of "this file is not one I can use" are all the same answer:
//! unreadable, unparsable, or written by a schema this build does not implement.
//! Each is an `Err`, and in every one of them **the file is left exactly as it
//! was**. The failure the milestone forbids is 配置写入一半损坏用户设置且静默清空 —
//! a silent reset is indistinguishable from losing what the user typed, so a
//! caller that gets an `Err` here must report it and carry on with defaults.
//!
//! That last requirement is not left to the caller's discipline either:
//! [`save`] reads before it writes, so a file that cannot be read cannot be
//! replaced by a caller that ignored the error. See [`save`] for what that
//! costs and why it is worth it.
//!
//! ## The replacement is atomic, and the temp file is why
//!
//! [`save`] writes the new text to a sibling of the target, syncs it, and then
//! renames it over the target. The sibling is not a detail: `rename` is only
//! atomic within one filesystem, so the temp file has to be beside the target
//! rather than in the system temp directory. Nothing ever opens the settings
//! file for writing, so there is no instant at which it is half-written; the
//! worst a power cut can do is lose the *latest* change, not the file.
//!
//! [`write_atomically`] takes the write as a parameter so the half-written case
//! is testable: a writer that fails partway through leaves the previous file
//! byte-identical and no temp file behind, which is what
//! `a_half_written_replacement_leaves_the_previous_file_untouched` asserts.
//!
//! ## Errors name the path, never the contents
//!
//! Following `config::parse_error`, and for the reason it gives: `toml`'s error
//! `Display` renders the offending source line verbatim, so anything that quotes
//! it can put a value — and from there a credential, in a class where this file
//! is not supposed to hold one — into a log. Only the span-free message is kept,
//! and `an_error_never_echoes_the_files_contents` fails if that is undone.
//!
//! ## One rule that is *not* the writer's
//!
//! [`check_save_dir`] says whether a directory a person picked is one a task
//! could write into. [`save`] does not call it, on purpose: a validator inside
//! the writer would make a temporarily unmounted disk mean 「设置存不下来」. The
//! rule sits here so it has one home and one test; the command that can say
//! something to a person is what enforces it.
//!
//! Windows coverage is inherited from `app_paths` and absent for the same reason.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The schema this build writes.
///
/// v1 held one directory. v2 adds the **history** of directories the user has
/// chosen (`known_save_dirs`), because a file that was downloaded when an older
/// directory was current is still there afterwards: without the history, 「打开
/// 文件」 after a change could only look in the one directory that is current now
/// and would report a file that plainly exists as 「可能已被移动或删除」.
///
/// The version check is `!=` on the *found* version, and v1 is **upgraded**
/// rather than refused — see [`parse_named`]. Every other value (0, 3, …) is
/// still an `Err` whose file is left exactly as it was: a version this build does
/// not implement is not one it can migrate from.
pub const SCHEMA_VERSION: u32 = 2;

/// How many previously chosen directories are remembered.
///
/// A bound rather than unlimited growth, because the file is small, hand-editable
/// and read on every start. Eight is more than any real installation accumulates
/// (a person changes their download folder a handful of times, and adds a
/// directory to search a rarer handful still), and the oldest is what goes: the
/// file that was downloaded longest ago is the one least likely to still be on
/// this machine. The consequence — a directory dropped from the history is no
/// longer searched by 「打开文件」 — is why this is a number worth stating rather
/// than an implementation detail, and why
/// [`UserSettings::note_search_dir`] reports the directory it evicted.
pub const MAX_KNOWN_SAVE_DIRS: usize = 8;

/// The file name inside the data root.
pub const FILE_NAME: &str = "settings.toml";

/// Appended to the settings path to name the file a replacement is built in.
///
/// A suffix rather than a random name because a crash between write and rename
/// leaves it behind, and the next launch should be able to recognise it as
/// ours; `a_stale_temp_file_does_not_break_the_next_save` covers the recovery.
pub const TEMP_SUFFIX: &str = ".tmp";

/// What the user has changed about this installation.
///
/// `deny_unknown_fields`, as every struct in `config` uses: a key we do not
/// understand is more likely a typo than a feature, and dropping it silently is
/// the behaviour this module exists to avoid.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UserSettings {
    /// Which schema wrote this file. Always written by [`save`], never by the
    /// caller — see [`save`] for why.
    pub schema_version: u32,
    /// Where downloaded material and finished pieces are saved.
    ///
    /// `None` means the user has not chosen, and is written as an absent key
    /// rather than an empty string: "unset" and "the empty path" are different
    /// answers and only one of them is a place a file can be written to. What
    /// `None` means for the task that reads it is that task's decision; this
    /// module stores the choice, it does not make it.
    ///
    /// A path that is not valid UTF-8 cannot be stored in a TOML string and is
    /// therefore refused by serde rather than mangled — registered in the T-04
    /// evidence rather than worked around.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_dir: Option<PathBuf>,
    /// Every directory this user has chosen **or asked to search**, newest first.
    ///
    /// The current one is always in this list, and [`save`] normalizes the file so
    /// that it leads. A directory that is in here *without* being the current
    /// choice is one [`UserSettings::note_search_dir`] added — 「我以前下到这儿
    /// 的文件还在这个目录里」, which is a different statement from 「新文件写到
    /// 这儿」 and only the first one is being made.
    ///
    /// Written only when non-empty, so a first launch's file is unchanged from v1's
    /// — an absent key and an empty list are the same fact.
    ///
    /// The name is `known_save_dirs` rather than `save_dirs` and that is not
    /// cosmetic: `an_unknown_key_is_refused_rather_than_dropped` plants `save_dirs`
    /// as its misspelling sample, and a field that took the name would make that
    /// test start passing for the wrong reason.
    ///
    /// **Clearing the choice does not clear this.** 「清除」 says 「do not default
    /// new downloads anywhere」, not 「forget where the files I already have were
    /// written」 — and those files are still there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_save_dirs: Vec<PathBuf>,
}

impl UserSettings {
    /// The directory a new download goes to, and the history behind it.
    ///
    /// Deliberately *not* a field-built list: a hand-edited file can hold a
    /// `save_dir` that is not in `known_save_dirs`, or a current directory that
    /// appears further down the history, and every reader that walked the raw
    /// field would then answer 「which directories should I search」 differently.
    /// This is the one definition of that answer — deduplicated, current first.
    pub fn search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::with_capacity(self.known_save_dirs.len() + 1);
        if let Some(current) = &self.save_dir {
            dirs.push(current.clone());
        }
        for dir in &self.known_save_dirs {
            if !dirs.contains(dir) {
                dirs.push(dir.clone());
            }
        }
        dirs
    }

    /// Record a newly chosen directory: it becomes the current one **and** the
    /// front of the history. `None` clears only the choice.
    ///
    /// One function rather than two assignments at the call site, because the
    /// invariant ("the current directory is the front of the history") is what
    /// `search_dirs` and the migration scan rely on, and a caller that set one
    /// without the other would leave a file that reads as if the operator had
    /// chosen two different directories most recently.
    pub fn remember(&mut self, chosen: Option<PathBuf>) {
        let Some(dir) = chosen else {
            self.save_dir = None;
            return;
        };
        let mut dirs = vec![dir.clone()];
        dirs.extend(
            self.known_save_dirs
                .iter()
                .filter(|old| **old != dir)
                .cloned(),
        );
        // The oldest goes. A directory dropped here is one 「打开文件」 no longer
        // searches, which is the honest cost of a bounded file.
        dirs.truncate(MAX_KNOWN_SAVE_DIRS);
        self.save_dir = Some(dir);
        self.known_save_dirs = dirs;
    }

    /// Record a directory that is to be **searched** without becoming the place
    /// new files are written.
    ///
    /// The case this exists for is a save location that has already changed: the
    /// material downloaded before that change is still in the older directory, and
    /// 「打开文件」、搬运与删除 have to keep looking there. [`remember`](Self::remember)
    /// cannot express it — it makes its argument the current directory as well —
    /// and the two are separate functions rather than one with a flag, because
    /// 「新文件写到哪儿」 and 「这个文件可能在哪些地方」 are two answers and this call
    /// gives only the second one.
    ///
    /// The directory joins at the **front of the history** and [`save`] then
    /// normalizes the file, so it ends up immediately behind the current choice and
    /// ahead of every directory the operator picked earlier. That is where it
    /// needs to be: the eviction below drops the oldest, and the directory
    /// somebody has just said 「我的文件在那儿」 about is the one whose loss they
    /// would notice.
    ///
    /// **A directory that is already searched is a no-op**, and the answer says so
    /// rather than reporting a change: `search_dirs` is the union of the choice and
    /// the history, so 「已经在里面」 is the whole truth for that input, and moving
    /// its position to no observable end would only make the answer harder to
    /// believe.
    ///
    /// **The history stays bounded, so this can evict.** [`SearchNote::dropped`]
    /// names the directory that fell out instead of letting it go in silence: one
    /// dropped here is one 「打开文件」 stops searching.
    pub fn note_search_dir(&mut self, dir: PathBuf) -> SearchNote {
        if self.search_dirs().contains(&dir) {
            return SearchNote {
                added: false,
                dropped: None,
            };
        }
        let mut dirs = vec![dir];
        dirs.extend(self.known_save_dirs.iter().cloned());
        // At most one can fall out: the history never holds more than
        // `MAX_KNOWN_SAVE_DIRS`, and this call adds exactly one.
        let dropped = dirs.get(MAX_KNOWN_SAVE_DIRS).cloned();
        dirs.truncate(MAX_KNOWN_SAVE_DIRS);
        self.known_save_dirs = dirs;
        SearchNote {
            added: true,
            dropped,
        }
    }
}

/// What [`UserSettings::note_search_dir`] did.
///
/// A value rather than `()`, because the answers a person reads are different
/// sentences and one of them is a warning about a directory they will stop being
/// able to find files in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchNote {
    /// False when the directory was already searched and nothing changed.
    pub added: bool,
    /// The oldest directory the bounded history dropped to make room, if any.
    pub dropped: Option<PathBuf>,
}

/// v1's shape, kept so a v1 file can be **upgraded** instead of refused.
///
/// A separate struct rather than a version-tolerant `UserSettings`, because
/// `deny_unknown_fields` is what makes 「this file is not one I understand」 a
/// report: reading a v1 file into the v2 struct would accept `known_save_dirs`
/// from a file that claims not to know about it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserSettingsV1 {
    /// Declared because every v1 file has it, and **deliberately not read**: which
    /// version the file is was decided by [`declared_version`] before this struct
    /// was chosen, and reading the same fact a second time here would be a second
    /// answer to a question that can only have one. The field still has to be
    /// named — a struct with `deny_unknown_fields` refuses a key it does not
    /// declare, so omitting it would make every real v1 file unreadable. But it
    /// must not be *typed* as a `u32`: that would accept `schema_version = 7` here
    /// and upgrade a file whose version nobody agreed to.
    #[serde(rename = "schema_version")]
    _schema_version: serde::de::IgnoredAny,
    #[serde(default)]
    save_dir: Option<PathBuf>,
}

impl From<UserSettingsV1> for UserSettings {
    /// v1's one directory becomes the current choice *and* the whole history: it
    /// is the only directory that version could have written a file into.
    fn from(old: UserSettingsV1) -> Self {
        let mut settings = UserSettings {
            schema_version: SCHEMA_VERSION,
            save_dir: old.save_dir,
            known_save_dirs: Vec::new(),
        };
        settings.known_save_dirs = settings.save_dir.iter().cloned().collect();
        settings
    }
}

impl Default for UserSettings {
    /// The state of a first launch: this build's schema, nothing chosen.
    ///
    /// Written out rather than derived, because `#[derive(Default)]` would make
    /// the version `0` — a first run that immediately claims to be from a schema
    /// that does not exist.
    fn default() -> Self {
        UserSettings {
            schema_version: SCHEMA_VERSION,
            save_dir: None,
            known_save_dirs: Vec::new(),
        }
    }
}

/// Where the settings file lives, given the data root.
pub fn path(data_root: &Path) -> PathBuf {
    data_root.join(FILE_NAME)
}

/// Where a replacement is built before it replaces `target`.
///
/// Public and separate because "same directory as the target" is a correctness
/// requirement (atomic `rename`, one filesystem), not an implementation detail —
/// and `temp_path_is_a_sibling_of_the_target` is what holds it there.
pub fn temp_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(TEMP_SUFFIX);
    PathBuf::from(name)
}

/// Why a directory the user picked is not one a task could write into.
///
/// The rule lives here, beside the field it is about, and is called by the
/// command rather than by [`save`]. That split is deliberate: `save`'s job is to
/// keep what the user chose, and a validator inside the writer would turn 「这块
/// 盘现在没挂载」into 「设置根本存不下来」— which is the failure T-04 registered as
/// boundary 4. So the writer stays permissive and the caller, which can say
/// something to a person, is the one that refuses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveDirProblem {
    /// A relative path resolves against this process's working directory, which
    /// is not a place a task's output belongs and not the same place twice.
    Relative,
    /// Nothing is there, or something that is not a directory is.
    NotADirectory,
}

impl SaveDirProblem {
    /// The sentence the page shows. It names the rule the value broke.
    pub const fn message(self) -> &'static str {
        match self {
            SaveDirProblem::Relative => "请选择绝对路径（例如 /Users/…/Movies/WTMedia）",
            SaveDirProblem::NotADirectory => "这个位置现在不是一个目录：请先创建它，或换一个位置",
        }
    }
}

/// Whether a chosen save directory is one a task could write into.
///
/// Existence is checked; **createdness deliberately is not**. Creating the
/// directory here would mean a mistyped path leaves an empty tree wherever the
/// user happened to be pointing, and the honest place for that tree to appear is
/// wherever they point at on purpose.
///
/// A directory that exists but cannot be written to **passes**: probing it would
/// write a file into a place the user picked to keep their own material, as a
/// side effect of opening a settings page. A task that later fails to write there
/// reports the permission itself, where the failure actually happened.
pub fn check_save_dir(path: &Path) -> Result<(), SaveDirProblem> {
    if !path.is_absolute() {
        return Err(SaveDirProblem::Relative);
    }
    if !path.is_dir() {
        return Err(SaveDirProblem::NotADirectory);
    }
    Ok(())
}

/// Why a settings file could not be used. The path is named; the contents never.
#[derive(Debug)]
pub enum SettingsError {
    /// The file exists and could not be read.
    Unreadable {
        path: PathBuf,
        reason: std::io::Error,
    },
    /// The file exists and is not a settings file this build can parse.
    Unparsable { path: PathBuf, reason: String },
    /// The file was written by a schema this build does not implement.
    UnknownSchema {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    /// A replacement could not be written; the target is unchanged.
    Unwritable {
        path: PathBuf,
        reason: std::io::Error,
    },
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Unreadable { path, reason } => {
                write!(
                    f,
                    "settings file {} could not be read: {reason}",
                    path.display()
                )
            }
            SettingsError::Unparsable { path, reason } => write!(
                f,
                "settings file {} is not a settings file this build can parse: {reason}",
                path.display()
            ),
            SettingsError::UnknownSchema {
                path,
                found,
                supported,
            } => write!(
                f,
                "settings file {} declares schema_version {found}, and this build implements \
                 {supported}; the file has been left unchanged",
                path.display()
            ),
            SettingsError::Unwritable { path, reason } => write!(
                f,
                "settings file {} could not be replaced (the previous contents are intact): \
                 {reason}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Parse settings text. Pure.
pub fn parse(text: &str) -> Result<UserSettings, SettingsError> {
    parse_named(text, Path::new(FILE_NAME))
}

fn parse_named(text: &str, path: &Path) -> Result<UserSettings, SettingsError> {
    // The version is probed **before** a body shape is chosen, and v1 is
    // upgraded rather than refused. Reading a v1 file into `UserSettings` would
    // not work at all — it is `deny_unknown_fields`, so a file that predates
    // `known_save_dirs` would be refused for a key it does not have, and the
    // user's chosen directory would be lost on the first launch after an update.
    //
    // In memory only: nothing is written back here, so a v1 file stays v1 until
    // the next save. That is the same rule `load` keeps for every read.
    if declared_version(text) == Some(1) {
        let old: UserSettingsV1 = toml::from_str(text).map_err(|error| unparsable(path, error))?;
        return Ok(old.into());
    }
    let settings: UserSettings = toml::from_str(text).map_err(|error| unparsable(path, error))?;
    if settings.schema_version != SCHEMA_VERSION {
        return Err(SettingsError::UnknownSchema {
            path: path.to_path_buf(),
            found: settings.schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(settings)
}

/// What a file says its schema is, when it says so in a way that can be read.
///
/// Only an integer in `u32` range counts. Everything else — a missing key, a
/// string, a negative, a file that does not parse at all — answers `None` and is
/// left to `serde`'s own error, which at least names the line it choked on; a
/// probe that guessed here would replace a precise complaint with 「版本不对」.
fn declared_version(text: &str) -> Option<u32> {
    let value: toml::Value = toml::from_str(text).ok()?;
    u32::try_from(value.get("schema_version")?.as_integer()?).ok()
}

/// A parse failure, with the path named and the contents never quoted.
///
/// `message()` only. See this module's header: `toml`'s error `Display` renders
/// the offending line, and a line is content.
fn unparsable(path: &Path, error: toml::de::Error) -> SettingsError {
    SettingsError::Unparsable {
        path: path.to_path_buf(),
        reason: error.message().to_string(),
    }
}

/// Read the settings, or the defaults if there is no file yet.
///
/// A **missing** file is a first launch and not an error: nothing has been
/// written, so "the user has chosen nothing" is the true answer and no file is
/// created by looking. An **unusable** file is an `Err` and is left alone.
pub fn load(target: &Path) -> Result<UserSettings, SettingsError> {
    match std::fs::read_to_string(target) {
        Ok(text) => parse_named(&text, target),
        // Not found is the one `io::Error` that is not a problem: the user has
        // not changed anything yet. Every other kind — permissions, a directory
        // where the file belongs, a broken encoding — means settings may exist
        // and could not be seen, which is worth refusing over.
        Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => Ok(UserSettings::default()),
        Err(reason) => Err(SettingsError::Unreadable {
            path: target.to_path_buf(),
            reason,
        }),
    }
}

/// Replace the settings file with `settings`, atomically.
///
/// **It will not replace a file it cannot read.** Reading is a precondition, not
/// a courtesy: the failure this module exists to prevent is 配置写入一半损坏用户设置
/// 且静默清空, and half of that failure is a *caller* that reads an error, carries
/// on with defaults and writes them down. So the rule is enforced here, in the
/// one place that could break it, instead of being a paragraph a later caller has
/// to have read: a corrupt file, a file from another schema, or a file that
/// cannot be opened all make `save` an `Err` and leave the bytes alone. The
/// deliberate cost is that a user who *wants* to reset must remove the file —
/// and that is the right manual step, because only a person can decide to throw
/// settings away. `save_refuses_to_replace_a_file_it_cannot_read` pins it.
///
/// The version is stamped from [`SCHEMA_VERSION`] rather than taken from the
/// argument, so a caller cannot write a file that claims a schema this binary
/// does not implement — the file says what the writer is, not what the caller
/// wishes it were. `save_stamps_the_version_this_build_understands` pins it.
///
/// Does not create the parent directory; see this module's header.
pub fn save(target: &Path, settings: &UserSettings) -> Result<(), SettingsError> {
    // A missing file reads as the defaults, so this is also the first-launch
    // path; it never creates the file (see `load`).
    load(target)?;
    let mut stamped = UserSettings {
        schema_version: SCHEMA_VERSION,
        ..settings.clone()
    };
    // The history is normalized here for the same reason the version is stamped
    // here: 「现在这个是历史里最新的那个」 is a property of every file this module
    // writes, not a discipline every caller has to keep. `remember` is
    // idempotent, so a caller that already did it gets the same file.
    stamped.remember(stamped.save_dir.clone());
    // Serializing a struct this small cannot fail except for a path that is not
    // valid UTF-8, which `serde` refuses rather than mangling. Reported as
    // "unwritable" because that is what the caller's next step is either way.
    let text = toml::to_string(&stamped).map_err(|error| SettingsError::Unwritable {
        path: target.to_path_buf(),
        reason: std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()),
    })?;
    write_atomically(target, text.as_bytes(), |path, bytes| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        std::io::Write::write_all(&mut file, bytes)?;
        // Sync before the rename: without it a power cut can leave the rename
        // durable and the *contents* not, which is the half-written file this
        // whole arrangement exists to prevent.
        file.sync_all()
    })
}

/// Build the replacement beside the target, then rename it over the target.
///
/// The write is a parameter so the one failure that cannot be produced on
/// demand — dying partway through a write — is a test rather than a hope.
///
/// On any failure the temp file is removed (best effort: if the directory
/// itself is unwritable there is nothing to remove it with) and the target is
/// reported unchanged. The target is opened for writing nowhere in this module.
fn write_atomically(
    target: &Path,
    bytes: &[u8],
    write: impl Fn(&Path, &[u8]) -> std::io::Result<()>,
) -> Result<(), SettingsError> {
    let temp = temp_path(target);
    if let Err(reason) = write(&temp, bytes) {
        std::fs::remove_file(&temp).ok();
        return Err(SettingsError::Unwritable {
            path: target.to_path_buf(),
            reason,
        });
    }
    if let Err(reason) = std::fs::rename(&temp, target) {
        std::fs::remove_file(&temp).ok();
        return Err(SettingsError::Unwritable {
            path: target.to_path_buf(),
            reason,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first launch that has chosen one directory, built through the same
    /// `remember` the command uses — so the fixture cannot hold a history that
    /// disagrees with its current directory.
    fn chosen(dir: &str) -> UserSettings {
        let mut settings = UserSettings::default();
        settings.remember(Some(PathBuf::from(dir)));
        settings
    }

    /// A scratch directory of this test's own, named after the test binary's pid
    /// so two concurrent runs cannot collide, and removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-settings-{}-{}-{}",
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

    #[test]
    fn a_chosen_directory_round_trips() {
        let text = toml::to_string(&chosen("/Users/operator/Movies/WTMedia")).expect("serialize");
        assert_eq!(
            parse(&text).expect("parse"),
            chosen("/Users/operator/Movies/WTMedia")
        );
    }

    /// An unchosen directory is an absent key, not an empty string, and comes
    /// back as `None` rather than as the empty path.
    #[test]
    fn an_unchosen_directory_round_trips_as_none() {
        let text = toml::to_string(&UserSettings::default()).expect("serialize");
        assert!(
            !text.contains("save_dir"),
            "an unchosen directory must not be written at all: {text}"
        );
        assert_eq!(parse(&text).expect("parse").save_dir, None);
    }

    /// A v1 file keeps its directory, and answers as this build's schema.
    ///
    /// This is the whole point of the version probe: refusing v1 would lose the
    /// user's choice on the first launch after an update, and the directory in
    /// that file is still exactly where their files are.
    #[test]
    fn a_v1_file_is_read_as_the_directory_it_knew() {
        let got =
            parse("schema_version = 1\nsave_dir = \"/tmp/old\"\n").expect("a v1 file is usable");

        assert_eq!(got.save_dir, Some(PathBuf::from("/tmp/old")));
        assert_eq!(
            got.known_save_dirs,
            vec![PathBuf::from("/tmp/old")],
            "the one directory v1 could have written into is the whole history"
        );
        assert_eq!(got.schema_version, SCHEMA_VERSION);
        assert_eq!(got.search_dirs(), vec![PathBuf::from("/tmp/old")]);
    }

    /// A v1 file with no directory chooses nothing, and is still not refused.
    #[test]
    fn a_v1_file_that_chose_nothing_reads_as_unchosen() {
        let got = parse("schema_version = 1\n").expect("a v1 file is usable");

        assert_eq!(got.save_dir, None);
        assert!(got.known_save_dirs.is_empty());
    }

    /// Reading a v1 file leaves it a v1 file.
    ///
    /// 「看不会创建东西」 in the module header, extended to 「看也不会改写」: an
    /// upgrade that happened on a read would rewrite the user's file on startup,
    /// and a file that is rewritten by looking at it is one nobody can diff.
    #[test]
    fn reading_a_v1_file_does_not_write_it_back() {
        let root = scratch("v1-readonly");
        let target = path(&root);
        let original = "schema_version = 1\nsave_dir = \"/tmp/old\"\n";
        std::fs::write(&target, original).expect("plant it");

        let got = load(&target).expect("a v1 file loads");

        let after = std::fs::read_to_string(&target).expect("still there");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(got.save_dir, Some(PathBuf::from("/tmp/old")));
        assert_eq!(after, original, "a read must not upgrade the file on disk");
    }

    /// The next save is what upgrades it, and the directory survives that.
    #[test]
    fn the_next_save_upgrades_a_v1_file_to_this_schema() {
        let root = scratch("v1-upgrade");
        let target = path(&root);
        std::fs::write(&target, "schema_version = 1\nsave_dir = \"/tmp/old\"\n").expect("plant it");

        save(&target, &load(&target).expect("read the v1 file")).expect("save");

        let text = std::fs::read_to_string(&target).expect("read back");
        let got = load(&target).expect("load");
        std::fs::remove_dir_all(&root).ok();

        assert!(
            text.contains(&format!("schema_version = {SCHEMA_VERSION}")),
            "{text}"
        );
        assert_eq!(got.save_dir, Some(PathBuf::from("/tmp/old")));
        assert_eq!(got.known_save_dirs, vec![PathBuf::from("/tmp/old")]);
    }

    /// A v1 file may not carry the key v1 did not have.
    ///
    /// This is why the upgrade uses a separate struct rather than a
    /// version-tolerant one: a file that says it is v1 while holding
    /// `known_save_dirs` is not a v1 file, and reading it as one — or as v2 —
    /// would mean half-understanding a file, which is the failure the version
    /// check exists to prevent.
    #[test]
    fn a_v1_file_carrying_the_newer_key_is_refused() {
        let error = parse("schema_version = 1\nknown_save_dirs = [\"/tmp/x\"]\n")
            .expect_err("v1 did not have that key");
        assert!(
            matches!(error, SettingsError::Unparsable { .. }),
            "{error:?}"
        );
    }

    /// The second choice keeps the first, and the current one leads.
    ///
    /// A file downloaded while `/tmp/old` was current is still there afterwards;
    /// without the history 「打开文件」 could only look at the directory that is
    /// current now and would call that file 「可能已被移动或删除」.
    #[test]
    fn choosing_a_second_directory_keeps_the_first_in_the_history() {
        let mut settings = chosen("/tmp/old");
        settings.remember(Some(PathBuf::from("/tmp/new")));

        assert_eq!(settings.save_dir, Some(PathBuf::from("/tmp/new")));
        assert_eq!(
            settings.known_save_dirs,
            vec![PathBuf::from("/tmp/new"), PathBuf::from("/tmp/old")]
        );
    }

    /// Choosing a directory that is already in the history moves it to the
    /// front instead of listing it twice.
    #[test]
    fn a_directory_chosen_again_moves_to_the_front_without_duplicating() {
        let mut settings = chosen("/tmp/a");
        settings.remember(Some(PathBuf::from("/tmp/b")));
        settings.remember(Some(PathBuf::from("/tmp/a")));

        assert_eq!(settings.save_dir, Some(PathBuf::from("/tmp/a")));
        assert_eq!(
            settings.known_save_dirs,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    /// The history is bounded, and the **oldest** is what goes.
    ///
    /// The consequence is not cosmetic: a directory dropped here is one
    /// 「打开文件」 stops searching, so the test asserts which end is dropped as
    /// well as the length.
    #[test]
    fn the_history_is_bounded_and_drops_the_oldest() {
        let dirs: Vec<PathBuf> = (0..MAX_KNOWN_SAVE_DIRS + 2)
            .map(|index| PathBuf::from(format!("/tmp/d{index}")))
            .collect();
        let mut settings = UserSettings::default();
        for dir in &dirs {
            settings.remember(Some(dir.clone()));
        }

        assert_eq!(settings.known_save_dirs.len(), MAX_KNOWN_SAVE_DIRS);
        assert_eq!(settings.save_dir, Some(dirs[dirs.len() - 1].clone()));
        assert_eq!(
            settings.known_save_dirs.first(),
            Some(&dirs[dirs.len() - 1]),
            "the newest leads"
        );
        for dropped in dirs.iter().take(dirs.len() - MAX_KNOWN_SAVE_DIRS) {
            assert!(
                !settings.known_save_dirs.contains(dropped),
                "{} should have been dropped",
                dropped.display()
            );
        }
    }

    /// A directory can be searched without becoming the place new files go.
    ///
    /// The whole point of the call: `save_dir` is untouched, the directory is in
    /// the history, and `search_dirs` — the one definition of 「这个文件可能在哪些
    /// 地方」 — answers with both.
    #[test]
    fn a_search_directory_is_added_without_becoming_the_choice() {
        let mut settings = chosen("/tmp/now");

        let note = settings.note_search_dir(PathBuf::from("/tmp/before"));

        assert_eq!(
            note,
            SearchNote {
                added: true,
                dropped: None
            }
        );
        assert_eq!(
            settings.save_dir,
            Some(PathBuf::from("/tmp/now")),
            "new downloads must keep going where they went"
        );
        assert_eq!(
            settings.known_save_dirs,
            vec![PathBuf::from("/tmp/before"), PathBuf::from("/tmp/now")],
            "and the history holds it, ahead of anything picked earlier"
        );
        assert_eq!(
            settings.search_dirs(),
            vec![PathBuf::from("/tmp/now"), PathBuf::from("/tmp/before")],
            "the search space is the choice first, then the history"
        );
    }

    /// A directory that is already searched is answered as no change.
    ///
    /// Two inputs, one answer, and both are the honest one: the current choice is
    /// searched (`search_dirs` leads with it) and so is a directory already in the
    /// history. Reporting 「已加入」 for either would tell the operator something
    /// happened that did not.
    #[test]
    fn a_directory_already_searched_is_not_added_again() {
        let mut settings = chosen("/tmp/now");
        settings.remember(Some(PathBuf::from("/tmp/before")));
        let before = settings.clone();

        let the_choice = settings.note_search_dir(PathBuf::from("/tmp/now"));
        let in_history = settings.note_search_dir(PathBuf::from("/tmp/before"));

        assert_eq!(
            the_choice,
            SearchNote {
                added: false,
                dropped: None
            }
        );
        assert_eq!(
            in_history,
            SearchNote {
                added: false,
                dropped: None
            }
        );
        assert_eq!(
            settings, before,
            "a no-op has to leave the file it would have written exactly as it was"
        );
    }

    /// Thinking a directory into the search space can push the oldest out, and
    /// the answer names it.
    ///
    /// The pair of assertions is the point: with room to spare nothing is dropped,
    /// and with the history full the directory that goes is the oldest — not the
    /// one just added, and not the current choice.
    #[test]
    fn adding_a_search_directory_reports_the_oldest_it_evicted() {
        let dirs: Vec<PathBuf> = (0..MAX_KNOWN_SAVE_DIRS)
            .map(|index| PathBuf::from(format!("/tmp/d{index}")))
            .collect();
        let mut settings = UserSettings::default();
        for dir in &dirs {
            settings.remember(Some(dir.clone()));
        }
        let oldest = dirs.first().cloned().expect("a full history");

        let note = settings.note_search_dir(PathBuf::from("/tmp/older-still"));

        assert_eq!(
            note,
            SearchNote {
                added: true,
                dropped: Some(oldest.clone()),
            },
            "{:?}",
            settings.known_save_dirs
        );
        assert_eq!(settings.known_save_dirs.len(), MAX_KNOWN_SAVE_DIRS);
        assert!(!settings.known_save_dirs.contains(&oldest));
        assert!(settings
            .search_dirs()
            .contains(&PathBuf::from("/tmp/older-still")));
        assert_eq!(
            settings.save_dir,
            Some(dirs[dirs.len() - 1].clone()),
            "and the choice is still the one that was chosen"
        );
    }

    /// Saving after a note keeps the noted directory, behind the choice.
    ///
    /// `save` normalizes the history so the current directory leads, and that
    /// rewrite is where a note could quietly be lost. The order asserted here is
    /// the file's, read back from disk rather than from the struct that was handed
    /// in.
    #[test]
    fn a_save_keeps_a_noted_directory_behind_the_choice() {
        let root = scratch("note-then-save");
        let target = path(&root);
        let mut settings = chosen("/tmp/now");
        // Noted in this order, so the second is the more recent note and is the
        // one that must end up nearest the choice.
        settings.note_search_dir(PathBuf::from("/tmp/first"));
        settings.note_search_dir(PathBuf::from("/tmp/second"));

        save(&target, &settings).expect("save");
        let got = load(&target).expect("load");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            got.known_save_dirs,
            vec![
                PathBuf::from("/tmp/now"),
                PathBuf::from("/tmp/second"),
                PathBuf::from("/tmp/first"),
            ],
            "the choice leads and both notes are still searched"
        );
    }

    /// Clearing the choice does not forget where the files already are.
    ///
    /// 「清除」 says 「do not default new downloads anywhere」, not 「forget the
    /// files I already have」 — and those files are still in those directories.
    #[test]
    fn clearing_the_choice_keeps_the_history() {
        let mut settings = chosen("/tmp/a");
        settings.remember(Some(PathBuf::from("/tmp/b")));

        settings.remember(None);

        assert_eq!(settings.save_dir, None);
        assert_eq!(
            settings.known_save_dirs,
            vec![PathBuf::from("/tmp/b"), PathBuf::from("/tmp/a")]
        );
        assert_eq!(
            settings.search_dirs(),
            vec![PathBuf::from("/tmp/b"), PathBuf::from("/tmp/a")],
            "the directories are still searched"
        );
    }

    /// A hand-edited file cannot make the search miss the directory in use.
    ///
    /// `save_dir` and the front of `known_save_dirs` can disagree only in a file
    /// a person typed; `search_dirs` answers with the union either way, current
    /// first, and never twice.
    #[test]
    fn search_dirs_is_the_current_directory_then_the_history_deduplicated() {
        let hand_edited = UserSettings {
            schema_version: SCHEMA_VERSION,
            save_dir: Some(PathBuf::from("/tmp/b")),
            known_save_dirs: vec![
                PathBuf::from("/tmp/a"),
                PathBuf::from("/tmp/b"),
                PathBuf::from("/tmp/a"),
            ],
        };

        assert_eq!(
            hand_edited.search_dirs(),
            vec![PathBuf::from("/tmp/b"), PathBuf::from("/tmp/a"),]
        );
    }

    /// Every file this module writes has a history headed by the directory in
    /// use — the caller does not have to have done it.
    #[test]
    fn save_normalizes_a_history_that_disagrees_with_the_choice() {
        let root = scratch("normalize");
        let target = path(&root);
        let inconsistent = UserSettings {
            schema_version: SCHEMA_VERSION,
            save_dir: Some(PathBuf::from("/tmp/second")),
            known_save_dirs: vec![PathBuf::from("/tmp/first")],
        };

        save(&target, &inconsistent).expect("save");

        let got = load(&target).expect("load");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            got.known_save_dirs,
            vec![PathBuf::from("/tmp/second"), PathBuf::from("/tmp/first")]
        );
    }

    /// The first launch has no file, and looking must not create one — a read
    /// that writes is how a diagnostic turns into a change.
    #[test]
    fn a_missing_file_is_a_first_launch_and_is_not_created_by_looking() {
        let root = scratch("missing");
        let target = path(&root);

        let got = load(&target);

        let exists = target.exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(got.expect("defaults"), UserSettings::default());
        assert!(!exists, "load must not create the file");
    }

    /// [`UserSettings::default`] is a first launch, so it is *this* build's
    /// schema and not zero. A derived `Default` would claim a schema that has
    /// never existed, and the first save would write that claim to disk.
    #[test]
    fn the_default_is_this_builds_schema() {
        assert_eq!(UserSettings::default().schema_version, SCHEMA_VERSION);
        assert_ne!(UserSettings::default().schema_version, 0);
    }

    /// A file that does not parse is reported, and is left byte-identical.
    ///
    /// This is the milestone's 配置写入一半损坏用户设置且静默清空 turned into an
    /// assertion: a caller that resets on `Err` loses the user's choices, so what
    /// is pinned here is that the bytes are still there to recover from.
    #[test]
    fn corrupt_settings_are_an_error_and_the_file_is_left_untouched() {
        let root = scratch("corrupt");
        let target = path(&root);
        let original = b"schema_version = 1\nsave_dir = /this_is_not_quoted\n";
        std::fs::write(&target, original).expect("plant it");

        let error = load(&target).expect_err("must not be reported as usable");
        let after = std::fs::read(&target).expect("still there");
        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(error, SettingsError::Unparsable { .. }),
            "{error:?}"
        );
        assert_eq!(after, original, "a refused file must be left as it was");
    }

    /// A newer schema is refused, not read for the keys this build happens to
    /// recognise: half-understanding a file is how an upgrade loses settings.
    #[test]
    fn a_file_from_a_newer_schema_is_refused_and_left_untouched() {
        let root = scratch("newer");
        let target = path(&root);
        let original = format!(
            "schema_version = {}\nsave_dir = \"/tmp/x\"\n",
            SCHEMA_VERSION + 1
        );
        std::fs::write(&target, &original).expect("plant it");

        let error = load(&target).expect_err("must not be read as this schema");
        let after = std::fs::read_to_string(&target).expect("still there");
        std::fs::remove_dir_all(&root).ok();

        match error {
            SettingsError::UnknownSchema {
                found, supported, ..
            } => {
                assert_eq!(found, SCHEMA_VERSION + 1);
                assert_eq!(supported, SCHEMA_VERSION);
            }
            other => panic!("expected an unknown schema, got {other:?}"),
        }
        assert_eq!(after, original, "and the file is left as it was");
    }

    /// Zero is not "older but close enough" — there has never been a version 0.
    #[test]
    fn a_file_claiming_version_zero_is_refused() {
        let error = parse_named("schema_version = 0\n", Path::new("settings.toml"))
            .expect_err("no such schema");
        assert!(
            matches!(error, SettingsError::UnknownSchema { found: 0, .. }),
            "{error:?}"
        );
    }

    /// An unrecognised key is refused rather than dropped.
    ///
    /// Dropping is the quiet failure: the user edited the file, the app read
    /// half of it, and the next save writes the other half away.
    ///
    /// `save_dirs` is the sample because it is 「`save_dir`」 with an `s` — the
    /// typo this schema's own field invites, and the name a careless rename of
    /// `known_save_dirs` would take over. Note which struct refuses it: with
    /// `schema_version = 1` this now goes down the **v1** path, so what refuses
    /// it is `UserSettingsV1`'s `deny_unknown_fields`. The next test is what
    /// holds the same rule for a current file.
    #[test]
    fn an_unknown_key_is_refused_rather_than_dropped() {
        let error = parse_named(
            "schema_version = 1\nsave_dirs = \"/tmp/x\"\n",
            Path::new("s.toml"),
        )
        .expect_err("a typo must not be ignored");
        assert!(
            matches!(error, SettingsError::Unparsable { .. }),
            "{error:?}"
        );
    }

    /// The same refusal for a file this build's schema wrote.
    #[test]
    fn an_unknown_key_in_a_current_file_is_refused_too() {
        let error = parse_named(
            &format!("schema_version = {SCHEMA_VERSION}\nsave_dirs = \"/tmp/x\"\n"),
            Path::new("s.toml"),
        )
        .expect_err("a typo must not be ignored");
        assert!(
            matches!(error, SettingsError::Unparsable { .. }),
            "{error:?}"
        );
    }

    /// The error text names the file and never quotes what is in it.
    ///
    /// The positive control is the first assertion: `toml`'s own `Display` *does*
    /// print the offending line, so a later change that forwards it back to the
    /// user — or into a log — is caught rather than looking harmless. This is
    /// `config::parse_error`'s reasoning applied to a second file, and it holds
    /// for the same reason even though this class is not supposed to hold a
    /// credential: the guarantee should not depend on that being true forever.
    #[test]
    fn an_error_never_echoes_the_files_contents() {
        let planted = "hunter2-looks-like-a-secret";
        let text = format!("schema_version = 1\nsave_dir = {planted}\n");

        let raw = toml::from_str::<UserSettings>(&text).expect_err("unquoted value");
        assert!(
            format!("{raw}").contains(planted),
            "the control must show that toml's Display would leak the line: {raw}"
        );

        let error =
            parse_named(&text, Path::new("/data/settings.toml")).expect_err("still unparsable");
        let shown = error.to_string();
        assert!(
            !shown.contains(planted),
            "our error must not quote the contents: {shown}"
        );
        assert!(
            shown.contains("/data/settings.toml"),
            "but it must name the file: {shown}"
        );
    }

    /// A file this build cannot read is never replaced.
    ///
    /// This is the forbidden failure reached the other way round: `load` reports
    /// the corruption, a caller ignores it and saves the defaults it fell back
    /// to, and the file the user could have fixed by hand is gone. Both halves of
    /// that are pinned — this test for the write, the two above for the read.
    #[test]
    fn save_refuses_to_replace_a_file_it_cannot_read() {
        let root = scratch("refuse");
        let target = path(&root);
        let original = b"schema_version = 1\nsave_dir = /this_is_not_quoted\n";
        std::fs::write(&target, original).expect("plant it");

        let error = save(&target, &chosen("/tmp/x")).expect_err("must not overwrite it");

        let after = std::fs::read(&target).expect("still there");
        let leftover = temp_path(&target).exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(error, SettingsError::Unparsable { .. }),
            "{error:?}"
        );
        assert_eq!(after, original, "the file must survive the attempt");
        assert!(!leftover, "and no temp file may be left for it either");
    }

    /// The same refusal for a file written by a schema we do not implement —
    /// the case an upgrade produces, where overwriting would be the app
    /// silently downgrading a newer file it does not understand.
    #[test]
    fn save_refuses_to_replace_a_file_from_another_schema() {
        let root = scratch("refuse-schema");
        let target = path(&root);
        let original = format!("schema_version = {}\n", SCHEMA_VERSION + 1);
        std::fs::write(&target, &original).expect("plant it");

        let error = save(&target, &chosen("/tmp/x")).expect_err("must not overwrite it");

        let after = std::fs::read_to_string(&target).expect("still there");
        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(error, SettingsError::UnknownSchema { .. }),
            "{error:?}"
        );
        assert_eq!(after, original);
    }

    /// A directory where the file belongs is an error, not "no settings yet".
    ///
    /// Made reachable by a directory occupying the path rather than by a
    /// permissions trick, following the precedent in `paths.rs` and `app_paths`:
    /// this behaves the same for a root user and in CI.
    #[test]
    fn a_directory_where_the_file_belongs_is_an_error_not_a_default() {
        let root = scratch("occupied");
        let target = path(&root);
        std::fs::create_dir_all(&target).expect("occupy the path");

        let error = load(&target).expect_err("must not be read as defaults");
        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(error, SettingsError::Unreadable { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn save_then_load_round_trips_through_a_file() {
        let root = scratch("roundtrip");
        let target = path(&root);

        save(&target, &chosen("/Users/operator/Movies/WTMedia")).expect("save");

        let got = load(&target);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(got.expect("load"), chosen("/Users/operator/Movies/WTMedia"));
    }

    /// Replacing a longer file with a shorter one must not leave the old tail
    /// behind: a settings file with the previous value's last characters still
    /// on the end does not parse, and it would look like corruption the app
    /// caused.
    #[test]
    fn a_replacement_does_not_leave_the_previous_contents_behind() {
        let root = scratch("shorter");
        let target = path(&root);
        save(
            &target,
            &chosen("/Users/operator/A-very-long-directory-name/with/many/parts"),
        )
        .expect("first");
        save(&target, &chosen("/tmp/x")).expect("second");

        let text = std::fs::read_to_string(&target).expect("read back");
        let got = load(&target);
        std::fs::remove_dir_all(&root).ok();

        assert!(
            !text.contains("A-very-long"),
            "no tail of the old file: {text}"
        );
        assert_eq!(got.expect("load"), chosen("/tmp/x"));
    }

    /// The write we cannot stage on purpose: a replacement that dies partway
    /// through. The previous file has to be byte-identical, no temp file may be
    /// left, and the caller has to be told.
    #[test]
    fn a_half_written_replacement_leaves_the_previous_file_untouched() {
        let root = scratch("half");
        let target = path(&root);
        let original = b"schema_version = 1\nsave_dir = \"/tmp/kept\"\n";
        std::fs::write(&target, original).expect("the file that must survive");
        let temp = temp_path(&target);

        let error = write_atomically(
            &target,
            b"schema_version = 1\nsave_dir = \"/tmp/new",
            |path, bytes| {
                // Half of the new text reaches the disk, then the write fails —
                // which is what a full disk or a killed process looks like.
                std::fs::write(path, &bytes[..bytes.len() / 2])?;
                Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "no space",
                ))
            },
        )
        .expect_err("a failed write must be reported");

        let after = std::fs::read(&target).expect("still there");
        let leftover = temp.exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(error, SettingsError::Unwritable { .. }),
            "{error:?}"
        );
        assert_eq!(after, original, "the previous file must be byte-identical");
        assert!(!leftover, "and the half-written temp file must be gone");
    }

    /// A temp file a crash left behind must not break the next save — and must
    /// not be mistaken for the settings file itself.
    ///
    /// The residue is deliberately **longer** than what the next save writes,
    /// which is the shape a real crash leaves: it died while replacing a settings
    /// file holding a longer path. A writer that reused that file without
    /// truncating it would publish the old tail as part of the new file, and the
    /// result would be an unparsable settings file this app caused. Asserted on
    /// the raw text because that is the only place a tail is visible — `load`
    /// would report it as corruption the *user* is accused of.
    #[test]
    fn a_stale_temp_file_does_not_break_the_next_save() {
        let root = scratch("stale");
        let target = path(&root);
        std::fs::write(
            temp_path(&target),
            b"schema_version = 1\nsave_dir = \"/Users/operator/Movies/WTMedia/chosen-before-the-crash\"\n",
        )
        .expect("plant it");

        save(&target, &chosen("/tmp/x")).expect("the next save must succeed");

        let text = std::fs::read_to_string(&target).expect("read back");
        let got = load(&target);
        let leftover = temp_path(&target).exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(got.expect("load"), chosen("/tmp/x"));
        assert!(
            !text.contains("chosen-before-the-crash"),
            "the stale file's tail must not survive into the new one: {text}"
        );
        assert!(
            !leftover,
            "the stale temp file is gone once it has been replaced by rename"
        );
    }

    /// A successful save leaves exactly one file behind.
    #[test]
    fn a_successful_save_leaves_no_temp_file() {
        let root = scratch("clean");
        let target = path(&root);

        save(&target, &chosen("/tmp/x")).expect("save");

        let names: Vec<String> = std::fs::read_dir(&root)
            .expect("read back")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(names, vec![FILE_NAME.to_string()]);
    }

    /// The temp file is a sibling, so the rename that publishes it is atomic.
    ///
    /// A temp file in the system temp directory would make the rename a copy
    /// across filesystems on any machine where they differ, and a copy is the
    /// half-written file this module exists to prevent. Asserted as the parent
    /// being the same one, which is the property that matters — not the name.
    #[test]
    fn temp_path_is_a_sibling_of_the_target() {
        let target =
            Path::new("/home/operator/Library/Application Support/WTMedia/Desktop/settings.toml");
        assert_eq!(temp_path(target).parent(), target.parent());
        assert_ne!(temp_path(target), target);
    }

    /// The version in the file is the writer's, not the caller's.
    #[test]
    fn save_stamps_the_version_this_build_understands() {
        let root = scratch("stamp");
        let target = path(&root);
        let lying = UserSettings {
            schema_version: SCHEMA_VERSION + 99,
            ..chosen("/tmp/x")
        };

        save(&target, &lying).expect("save");

        let text = std::fs::read_to_string(&target).expect("read back");
        let got = load(&target);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            got.expect("load").schema_version,
            SCHEMA_VERSION,
            "a caller must not be able to write a version this build does not implement"
        );
        assert!(
            text.contains(&format!("schema_version = {SCHEMA_VERSION}")),
            "{text}"
        );
    }

    /// Saving into a directory that does not exist is reported, and creates
    /// nothing.
    ///
    /// `app_paths::prepare` owns directory creation; a settings writer that
    /// created the tree silently would hide a data root that is missing for a
    /// reason somebody needs to see.
    #[test]
    fn save_reports_a_missing_directory_rather_than_creating_it() {
        let root = scratch("nodir");
        let target = path(&root.join("not-created"));

        let error = save(&target, &chosen("/tmp/x")).expect_err("must not succeed");

        let created = root.join("not-created").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(error, SettingsError::Unwritable { .. }),
            "{error:?}"
        );
        assert!(!created, "save must not create the directory");
    }

    /// The path handed to the caller is the one `load` and `save` use, so a
    /// caller cannot read one file and write another.
    #[test]
    fn the_path_is_the_data_roots_settings_file() {
        let root = Path::new("/Users/operator/Library/Application Support/WTMedia/Desktop");
        assert_eq!(path(root), root.join(FILE_NAME));
        assert_eq!(
            path(root).file_name().and_then(|n| n.to_str()),
            Some(FILE_NAME)
        );
    }

    /// Where the file actually lands once the wiring is `AppPaths`.
    ///
    /// [`path`] takes a data root and neither knows nor asks which one; this is
    /// the statement that the data root is the one the milestone named, spelled
    /// out in both layouts. It reads `app_paths` rather than repeating its rules,
    /// so if that module's data root ever moves, this fails instead of the two
    /// silently disagreeing about where a user's settings live.
    #[test]
    fn the_settings_file_lands_in_the_data_root_app_paths_resolves() {
        let home = Path::new("/Users/operator");
        let manifest = Path::new("/build/src-tauri");
        let system = crate::system_paths::SystemPaths::from_parts(
            crate::system_paths::Platform::Darwin,
            Some(home.to_path_buf()),
            None,
            manifest.to_path_buf(),
        );

        let installed =
            crate::app_paths::resolve(&system, crate::config::Environment::Production, manifest)
                .expect("an installed layout with a home resolves");
        let development =
            crate::app_paths::resolve(&system, crate::config::Environment::Development, manifest)
                .expect("a development layout resolves");

        assert_eq!(
            path(&installed.data),
            Path::new("/Users/operator/Library/Application Support/WTMedia/Desktop/settings.toml")
        );
        assert_eq!(
            path(&development.data),
            Path::new("/build/src-tauri/.local/data/settings.toml")
        );
        // And it is not one of the other three roots: a settings file under
        // `cache` would sit inside the one directory tree the cleanup command is
        // allowed to empty.
        assert_ne!(path(&installed.data), path(&installed.cache));
    }

    /// The picker's rule, in both directions, with the accepting case **first**.
    ///
    /// The positive control is the load-bearing half: two refusals on their own
    /// would also pass for a checker that refuses everything, which is a page
    /// that can never save a choice. It runs on a real directory this test made,
    /// so "absolute and exists" is the case a person actually produces.
    #[test]
    fn an_existing_absolute_directory_is_accepted() {
        let root = scratch("accepted");
        let result = check_save_dir(&root);
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(
            result,
            Ok(()),
            "a directory this test just made was refused"
        );
    }

    /// A relative path is refused, whatever it happens to resolve to here.
    ///
    /// Both spellings a person can produce: a bare name, and one that climbs.
    /// Neither is judged by whether something exists at the joined path — a
    /// relative path that *does* exist is still refused, which is why the test
    /// uses one (`"."` and this crate's own directory) rather than only names
    /// that are absent.
    #[test]
    fn a_relative_save_dir_is_refused() {
        for relative in [".", "..", "Movies/WTMedia", "./build"] {
            assert_eq!(
                check_save_dir(Path::new(relative)),
                Err(SaveDirProblem::Relative),
                "{relative:?}"
            );
        }
        // The same directory, named absolutely, is accepted: what is refused is
        // the relative *spelling*, not the place.
        let here = std::env::current_dir().expect("a working directory");
        assert_eq!(check_save_dir(&here), Ok(()), "{here:?}");
    }

    /// A path that names no directory is refused — absent, and a file.
    ///
    /// The two are one variant because one sentence covers both, and the test
    /// asserts both so that "is not a directory" is not quietly an "is not
    /// there": a regular file is the case a person reaches by picking the wrong
    /// thing in a picker that showed files.
    #[test]
    fn a_path_that_is_not_a_directory_is_refused() {
        let root = scratch("nondir");

        let missing = root.join("not-yet");
        assert_eq!(
            check_save_dir(&missing),
            Err(SaveDirProblem::NotADirectory),
            "an absent path must be refused rather than created"
        );
        assert!(!missing.exists(), "the checker must not create it");

        let file = root.join("a-file");
        std::fs::write(&file, b"x").expect("write a file");
        assert_eq!(check_save_dir(&file), Err(SaveDirProblem::NotADirectory));

        std::fs::remove_dir_all(&root).ok();
    }

    /// Every refusal has a sentence, and the sentences are not the same one.
    ///
    /// A page that showed one message for both would tell a person who picked a
    /// file that their path was relative.
    #[test]
    fn each_problem_says_something_of_its_own() {
        let problems = [SaveDirProblem::Relative, SaveDirProblem::NotADirectory];
        let messages: Vec<&str> = problems.iter().map(|problem| problem.message()).collect();
        assert_eq!(
            messages
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            problems.len(),
            "{messages:?}"
        );
        for message in messages {
            assert!(!message.is_empty(), "an empty message is not a message");
        }
    }
}
