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
//! Windows coverage is inherited from `app_paths` and absent for the same reason.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The only schema this build understands.
///
/// The check is `!=` rather than `>`: v1 is the first version, so 0 is as
/// unreadable as 2, and "we understand exactly one shape" is the honest
/// statement until a migration exists. When a v2 arrives, this is where the
/// branch goes.
pub const SCHEMA_VERSION: u32 = 1;

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
    let settings: UserSettings =
        toml::from_str(text).map_err(|error| SettingsError::Unparsable {
            path: path.to_path_buf(),
            // `message()` only. See this module's header: the error's `Display`
            // renders the offending line, and a line is content.
            reason: error.message().to_string(),
        })?;
    if settings.schema_version != SCHEMA_VERSION {
        return Err(SettingsError::UnknownSchema {
            path: path.to_path_buf(),
            found: settings.schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(settings)
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
    let stamped = UserSettings {
        schema_version: SCHEMA_VERSION,
        ..settings.clone()
    };
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

    fn chosen(dir: &str) -> UserSettings {
        UserSettings {
            schema_version: SCHEMA_VERSION,
            save_dir: Some(PathBuf::from(dir)),
        }
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
            save_dir: Some(PathBuf::from("/tmp/x")),
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

        let installed =
            crate::app_paths::resolve(Some(home), crate::config::Environment::Production, manifest)
                .expect("an installed layout with a home resolves");
        let development = crate::app_paths::resolve(
            Some(home),
            crate::config::Environment::Development,
            manifest,
        )
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
}
