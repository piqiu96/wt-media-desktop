//! Reading the logs back: what files are there, and what the last of them say.
//!
//! Writing is [`crate::logging::rolling`]'s job and reading is this module's, but
//! the two are not independent: the reader recognises an archive by the rule the
//! *writer's crate* uses to name one. That rule is not "the name starts with the
//! live file's name" — `file-rotate` splits the suffix at its first dot, accepts
//! a numeric collision index after it (`desktop.log.2026-09-24-19.1`, which is
//! what a second rotation inside one hour produces), and accepts a *truncated*
//! stamp as well (`chrono`'s `NotEnough`). [`archive_stamp`] reproduces those
//! three behaviours on purpose rather than approximating them, because the two
//! answers are not allowed to disagree: the crate decides what to delete, this
//! decides what the page shows and what T-06 may clean. A file this called
//! "other" that the crate considers its own archive would be listed as foreign
//! and silently exempted from retention.
//!
//! ## Both components write one shape of line
//!
//! ```text
//! 2026-09-24T20:47:36 [INFO] desktop.startup: 配置来自开发树 resources/
//! ```
//!
//! Desktop writes it from `logging::backend` (`STAMP_FORMAT` + `[LEVEL]`) and the
//! Agent writes it from `runtime/logging.py` (`FMT`), which is why one parser
//! serves both trees. The one difference found by reading real lines from both —
//! not by reading the format strings — is the spelling of a level: `tracing`
//! writes `WARN` and Python writes `WARNING` (and has `CRITICAL`, which `tracing`
//! does not). [`Level`] accepts all of them and folds `CRITICAL` into `ERROR`,
//! because a filter whose order differs per tree would sort a page of Agent logs
//! by a different rule than a page of Desktop logs.
//!
//! ## What the tail costs, and what it refuses to hide
//!
//! [`tail`] reads backwards from the end, so opening the viewer does not read a
//! month of history — and it stops at [`TAIL_MAX_BYTES`] as well as at the line
//! count, because one file with a single very long record would otherwise be
//! read whole. When that ceiling is what stopped it, the result says so
//! ([`LogTail::truncated`]) instead of returning fewer lines than asked for with
//! no explanation. Every line it returns is a whole line: a window that begins
//! mid-line drops that fragment, which means a file consisting of one record
//! longer than the ceiling reads as no lines and a truncation flag — the honest
//! pair, since the last ten lines of such a file are that one record.
//!
//! Invalid UTF-8 in a log file is not an error here: a truncated multi-byte
//! character (which is exactly what the byte ceiling can produce) and a genuinely
//! corrupt byte become U+FFFD. A log viewer that refused to show a file because
//! one byte in it was bad would be refusing exactly when a reader most needs to
//! see what is in it.
//!
//! ## Level inheritance
//!
//! A line that does not carry a level belongs to the record above it: a record
//! whose message contains newlines (a webview stack trace, a Python traceback)
//! is several lines on disk and one record in the log. [`entries`] therefore
//! gives such a line the level of the nearest preceding classified line, and
//! marks it ([`Entry::continues`]) so a viewer can indent it. Filtering by level
//! keeps a block together — dropping the continuation of an error would show the
//! head of a traceback and nothing else.

use crate::logging::rolling::{ARCHIVE_FORMAT, LOG_FILE_NAME};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How many lines the viewer opens with.
///
/// A page of history is what a person reads; the whole file is what they page
/// through. Named here rather than in the command layer so the number has one
/// home and the tests can use it.
pub const TAIL_LINES: usize = 500;

/// The most the tail will read from the end, whatever the line count says.
///
/// Guards against the one file that is large because a single record was: the
/// record cap (`rolling::Limits::max_record_bytes`) is 1 MiB, so four records of
/// that size still fit inside this, and a file that hits the ceiling hits it for
/// a reason the reader reports.
pub const TAIL_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// How much is read from the file at a time while walking backwards.
const TAIL_CHUNK_BYTES: u64 = 8 * 1024;

/// Which component's tree a file belongs to.
///
/// Carries the live file names, because they differ: Desktop has one
/// (`desktop.log`) and the Agent has three (`agent.log`, `error.log`,
/// `task.log` — one per handler in `runtime/logging.py`, which is where they come
/// from). Still one *shape* on both sides: a live name, then `.`, then the stamp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Desktop,
    Agent,
}

impl Source {
    /// The names this component writes as its live files.
    ///
    /// The Agent's three are **its** names, mirrored here: nothing checks the two
    /// lists against each other at build time, so a rename on the Python side
    /// would show up as an Agent whose files are all "other". The check that
    /// catches it is running against a real tree — `~/Library/Logs/WTMedia/Agent`
    /// holds these three names on this machine, recorded in
    /// `evidence/task-05-storage-read.md`.
    pub const fn live_names(self) -> &'static [&'static str] {
        match self {
            Source::Desktop => &[LOG_FILE_NAME],
            Source::Agent => &["agent.log", "error.log", "task.log"],
        }
    }

    /// What the page calls this tree.
    pub const fn label(self) -> &'static str {
        match self {
            Source::Desktop => "desktop",
            Source::Agent => "agent",
        }
    }

    /// The inverse of [`Source::label`], for a value arriving over the wire.
    ///
    /// `None` rather than a default: a command that fell back to Desktop would
    /// read a file from the wrong tree and present it as the one that was asked
    /// for, which is worse than telling the caller the value was not understood.
    pub fn from_label(label: &str) -> Option<Source> {
        match label.trim() {
            "desktop" => Some(Source::Desktop),
            "agent" => Some(Source::Agent),
            _ => None,
        }
    }
}

/// What a file in a log directory is.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FileKind {
    /// The file being written now.
    Live,
    /// A rotated hour, named by the rotator.
    Archive,
    /// A file this app did not write, or a name it does not recognise.
    ///
    /// A real case, not a theoretical one: a pre-T-02 install leaves
    /// `desktop-20260924-1.log` behind, and the rotator — which recognises only
    /// its own names — will never age it out. Listed so a person can see it, and
    /// kept out of the two recognised kinds so nothing treats it as ours.
    Other,
}

impl FileKind {
    /// The spelling the page filters and badges with.
    pub const fn as_str(self) -> &'static str {
        match self {
            FileKind::Live => "live",
            FileKind::Archive => "archive",
            FileKind::Other => "other",
        }
    }
}

/// A record's severity, ordered from the quietest to the loudest.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// The spelling the page filters with, matching `tracing`'s live output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    /// Parse a level token from either side of the product.
    ///
    /// `WARNING` and `CRITICAL` are Python's, `WARN` is Rust's. Case is not
    /// ignored: the token comes from a formatter either side, not from a person,
    /// and accepting `info` would also accept a message that happens to say
    /// `[info]` in brackets.
    pub fn from_token(token: &str) -> Option<Level> {
        match token {
            "TRACE" => Some(Level::Trace),
            "DEBUG" => Some(Level::Debug),
            "INFO" => Some(Level::Info),
            "WARN" | "WARNING" => Some(Level::Warn),
            // `CRITICAL` is Python's top level and `tracing` has no equivalent, so
            // it folds into `ERROR` rather than getting an order of its own: one
            // filter has to sort both trees.
            "ERROR" | "CRITICAL" => Some(Level::Error),
            _ => None,
        }
    }

    /// Parse the page's own spelling, for a filter that arrives over the wire.
    pub fn from_str_opt(value: &str) -> Option<Level> {
        match value.trim().to_ascii_lowercase().as_str() {
            "trace" => Some(Level::Trace),
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error" | "critical" => Some(Level::Error),
            _ => None,
        }
    }
}

/// One file in a log directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogFile {
    /// The file name, as it is on disk.
    pub name: String,
    /// The full path, so a caller never joins the two again by hand.
    pub path: PathBuf,
    pub kind: FileKind,
    /// Size on disk. A file that shrank between listing and reading is not an
    /// error here — this is a reading, not a promise.
    pub bytes: u64,
    /// Last modification, when the filesystem would say. `None` is not an error:
    /// the name is what orders the list, this is only shown.
    pub modified: Option<SystemTime>,
}

/// One line of a log, with the level that applies to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// The level of this line's record, or `None` for a line before the first
    /// record that carried one.
    pub level: Option<Level>,
    /// True when this line is a continuation of the record above it, rather than
    /// a record that carried its own level. See this module's header.
    pub continues: bool,
    /// The line, without its newline.
    pub text: String,
}

/// The last lines of a file, and whether a ceiling stopped the read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogTail {
    pub lines: Vec<String>,
    /// True when [`TAIL_MAX_BYTES`] was what ended the read, so the caller can
    /// say the view is a window rather than quote it as the whole end.
    pub truncated: bool,
}

/// Why a log could not be read. Holds no credentials: a path is a path.
#[derive(Debug)]
pub enum LogReadError {
    /// The directory or file could not be read.
    Unreadable {
        path: PathBuf,
        reason: std::io::Error,
    },
}

impl std::fmt::Display for LogReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogReadError::Unreadable { path, reason } => {
                write!(f, "{} could not be read: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for LogReadError {}

impl From<std::io::Error> for LogReadError {
    fn from(reason: std::io::Error) -> Self {
        LogReadError::Unreadable {
            // The caller has the path; this is the variant that has none.
            path: PathBuf::new(),
            reason,
        }
    }
}

/// The stamp of an archive of `live_name`, or `None` if the name is not one.
///
/// Mirrors `file-rotate`'s own `AppendTimestamp::parse` and its call site in
/// `scan_suffixes`: strip `"{live}."`, take a trailing `.<number>` as the
/// collision index (**split at the first dot**, which is what the crate does),
/// and accept the stamp if `chrono` parses it *or* reports `NotEnough`.
///
/// The two accepted kinds are worth being precise about, because the neighbouring
/// two are not accepted and the difference is invisible without measuring it.
/// `NotEnough` means "the whole format matched and the fields that were filled are
/// in range, but they do not fill a `NaiveDateTime`" — which is exactly what
/// `%Y-%m-%d-%H` produces, since a date and an hour are missing minute and second.
/// Measured, all through `ARCHIVE_FORMAT`:
///
/// | suffix | `NaiveDateTime::parse_from_str` | the crate's verdict |
/// | --- | --- | --- |
/// | `2026-09-24-19` | `NotEnough` | its own archive |
/// | `2026-09` | `TooShort` | not its own |
/// | `2026-09-24` | `TooShort` | not its own |
/// | `2026` | `TooShort` | not its own |
/// | `2026-09-24-19-00` | `TooLong` | not its own |
/// | `nonsense` | `Invalid` | not its own |
///
/// So a truncated *name* is not adopted: only a stamp the format consumes whole
/// is. A future change of `ARCHIVE_FORMAT` away from hour granularity has to move
/// this function with it — a date-only format would make the crate reject its own
/// archives for the same reason, so the two are already tied together.
///
/// The `.gz` suffix the crate also strips is **not** mirrored: rotation here is
/// configured with `Compression::None`, so a `.gz` file in this directory would
/// be somebody else's and is better listed as "other" than adopted.
pub fn archive_stamp<'a>(live_name: &str, name: &'a str) -> Option<&'a str> {
    let suffix = name.strip_prefix(&format!("{live_name}."))?;
    let stamp = match suffix.find('.') {
        Some(dot) => {
            // The collision index must parse; anything else after the first dot
            // is not a name this rotator produces.
            suffix[dot + 1..].parse::<usize>().ok()?;
            &suffix[..dot]
        }
        None => suffix,
    };
    match chrono::NaiveDateTime::parse_from_str(stamp, ARCHIVE_FORMAT) {
        Ok(_) => Some(stamp),
        Err(error) if error.kind() == chrono::format::ParseErrorKind::NotEnough => Some(stamp),
        Err(_) => None,
    }
}

/// Which kind of file `name` is in `source`'s tree.
pub fn classify(source: Source, name: &str) -> FileKind {
    if source.live_names().contains(&name) {
        return FileKind::Live;
    }
    if source
        .live_names()
        .iter()
        .any(|live| archive_stamp(live, name).is_some())
    {
        return FileKind::Archive;
    }
    FileKind::Other
}

/// The length of `%Y-%m-%dT%H:%M:%S`.
const STAMP_LEN: usize = 19;

/// The level a line carries, if it carries one.
///
/// The shape is read at fixed positions rather than by searching for `[`: the
/// stamp is 19 characters, then a space, then a bracketed token, then a space.
/// Requiring all four of those is what keeps a message that happens to contain
/// the text `[INFO]` from being read as a second record — which matters because
/// an inherited level is what a traceback's continuation lines are shown under,
/// so a false record here would relabel a block of real text.
pub fn record_level(line: &str) -> Option<Level> {
    let bytes = line.as_bytes();
    if bytes.len() < STAMP_LEN || !is_stamp(&bytes[..STAMP_LEN]) {
        return None;
    }
    // Slicing as a `str` is in bounds and on a character boundary because the
    // stamp check passed: those 19 bytes are ASCII, so byte 19 begins a
    // character.
    let rest = line[STAMP_LEN..].strip_prefix(' ')?;
    let token = rest.strip_prefix('[')?;
    let end = token.find(']')?;
    let word = &token[..end];
    if !word.bytes().all(|byte| byte.is_ascii_uppercase()) {
        // A token is a word. Anything else is text that happens to be bracketed.
        return None;
    }
    if !token[end + 1..].starts_with(' ') {
        return None;
    }
    Level::from_token(word)
}

/// Whether these bytes look like `%Y-%m-%dT%H:%M:%S`.
///
/// Digits and separators in the right positions — no calendar arithmetic. A
/// stamp is *recognisable*, not *valid*: a record written at a leap second, or
/// by a clock that was badly wrong, is still a record, and rejecting it would
/// hide it behind "not a record at all".
fn is_stamp(bytes: &[u8]) -> bool {
    bytes.iter().enumerate().all(|(index, byte)| match index {
        4 | 7 => *byte == b'-',
        10 => *byte == b'T',
        13 | 16 => *byte == b':',
        _ => byte.is_ascii_digit(),
    })
}

/// Every file in a log directory, in the order a viewer shows them.
///
/// A directory that is not there is an empty list, not an error: an Agent that
/// has never run has no log directory, and that is a fact about the machine
/// rather than a failure. Everything else — permissions, a file where the
/// directory belongs — is an `Err`, following `storage::directory_bytes`: an
/// unreadable tree must not read as an empty one.
///
/// A subdirectory inside the tree is skipped. The rotator writes files; a
/// directory there is not one, and following it would let the listing walk off
/// to wherever a link pointed.
pub fn list(directory: &Path, source: Source) -> Result<Vec<LogFile>, LogReadError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(reason) => {
            return Err(LogReadError::Unreadable {
                path: directory.to_path_buf(),
                reason,
            })
        }
    };

    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|reason| LogReadError::Unreadable {
            path: directory.to_path_buf(),
            reason,
        })?;
        let metadata = entry
            .metadata()
            .map_err(|reason| LogReadError::Unreadable {
                path: entry.path(),
                reason,
            })?;
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        files.push(LogFile {
            kind: classify(source, &name),
            path: entry.path(),
            name,
            bytes: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }

    // Newest first, and by the name rather than by mtime: the stamp in an
    // archive's name *is* its hour, zero-padded so that lexicographic order is
    // chronological order (the same property the rotator's age rule relies on),
    // while an mtime moves when a file is copied or touched. The live files come
    // first — the one being written is the newest thing there is — and unknown
    // names last.
    files.sort_by(|left, right| {
        let key = |file: &LogFile| match file.kind {
            // The Agent has three live files and Desktop one, so this group is
            // not always a single entry: the name is the only thing that orders
            // them, and `read_dir`'s order is the filesystem's to choose.
            FileKind::Live => (0_u8, file.name.clone()),
            FileKind::Archive => (
                1,
                source
                    .live_names()
                    .iter()
                    .find_map(|live| archive_stamp(live, &file.name))
                    .unwrap_or_default()
                    .to_string(),
            ),
            FileKind::Other => (2, file.name.clone()),
        };
        let (left_group, left_key) = key(left);
        let (right_group, right_key) = key(right);
        // Only the archives are reversed: time is in their key, and newest first
        // is what a viewer wants. The other two groups are alphabetical — their
        // keys carry no time — which is at least the same order on every call and
        // on every filesystem.
        match left_group.cmp(&right_group) {
            std::cmp::Ordering::Equal if left_group == 1 => right_key.cmp(&left_key),
            std::cmp::Ordering::Equal => left_key.cmp(&right_key),
            other => other,
        }
    });
    Ok(files)
}

/// The last `lines` lines of a file, read backwards from the end.
pub fn tail(path: &Path, lines: usize) -> Result<LogTail, LogReadError> {
    let mut file = std::fs::File::open(path).map_err(|reason| LogReadError::Unreadable {
        path: path.to_path_buf(),
        reason,
    })?;
    let size = file
        .metadata()
        .map_err(|reason| LogReadError::Unreadable {
            path: path.to_path_buf(),
            reason,
        })?
        .len();

    let mut window: Vec<u8> = Vec::new();
    let mut offset = size;
    let mut newlines = 0_usize;
    let mut stopped_by_bytes = false;
    while offset > 0 {
        let take = TAIL_CHUNK_BYTES.min(offset);
        offset -= take;
        let mut chunk = vec![0_u8; take as usize];
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.read_exact(&mut chunk))
            .map_err(|reason| LogReadError::Unreadable {
                path: path.to_path_buf(),
                reason,
            })?;
        // Counted as each chunk arrives rather than over the window, which grows
        // to megabytes: re-scanning it every time is quadratic, and an ordinary
        // log file is the case where it would be.
        newlines += chunk.iter().filter(|byte| **byte == b'\n').count();
        chunk.extend_from_slice(&window);
        window = chunk;
        if newlines > lines {
            break;
        }
        if window.len() as u64 >= TAIL_MAX_BYTES {
            stopped_by_bytes = true;
            break;
        }
    }

    // A window that does not reach the start of the file *may* begin mid-line,
    // and only the byte just before it says which. Probing costs one byte read
    // and buys back the case where the byte ceiling was what stopped the read:
    // without it, a whole line would be dropped along with the fragment, and the
    // view would be one line shorter than the file actually allows for no reason
    // a reader could see.
    let complete = offset == 0 || {
        let mut before = [0_u8; 1];
        file.seek(SeekFrom::Start(offset - 1))
            .and_then(|_| file.read_exact(&mut before))
            .is_ok()
            && before[0] == b'\n'
    };
    let text = String::from_utf8_lossy(&window);
    let mut collected: Vec<String> = text.split('\n').map(str::to_string).collect();
    if !complete && !collected.is_empty() {
        collected.remove(0);
    }
    // A file that ends with a newline ends with an empty piece, which is not a
    // line. Interior empty lines are kept: a blank line in a log is real.
    if collected.last().is_some_and(String::is_empty) {
        collected.pop();
    }
    let start = collected.len().saturating_sub(lines);
    Ok(LogTail {
        lines: collected.split_off(start),
        truncated: stopped_by_bytes,
    })
}

/// Give every line the level of the record it belongs to.
pub fn entries(lines: &[String]) -> Vec<Entry> {
    let mut current: Option<Level> = None;
    lines
        .iter()
        .map(|line| {
            let own = record_level(line);
            let (level, continues) = match own {
                Some(level) => {
                    current = Some(level);
                    (Some(level), false)
                }
                None => (current, true),
            };
            Entry {
                level,
                continues,
                text: line.clone(),
            }
        })
        .collect()
}

/// Keep the entries at `min` or louder.
///
/// An entry whose level is `None` — a line before the first record that carried
/// one — is kept whatever the filter says. Dropping it would hide the beginning
/// of a file behind a filter that cannot classify it, and "the first lines are
/// missing and nothing says why" is the failure mode a filter must not have.
pub fn filter_min(entries: &[Entry], min: Level) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| entry.level.is_none_or(|level| level >= min))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-reader-{}-{}-{}",
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

    /// A record in the shape both components write.
    fn record(stamp: &str, level: &str, message: &str) -> String {
        format!("{stamp} [{level}] desktop.startup: {message}")
    }

    /// The wire spelling round-trips, and an unknown one is `None` rather than a
    /// default — a command that defaulted would read the wrong tree.
    #[test]
    fn the_wire_spelling_round_trips_and_an_unknown_one_does_not() {
        for source in [Source::Desktop, Source::Agent] {
            assert_eq!(Source::from_label(source.label()), Some(source));
        }
        assert_eq!(Source::from_label(" desktop "), Some(Source::Desktop));
        assert_eq!(Source::from_label("sidecar"), None);
        assert_eq!(Source::from_label(""), None);
    }

    #[test]
    fn the_live_name_is_the_live_kind_for_each_component() {
        assert_eq!(classify(Source::Desktop, "desktop.log"), FileKind::Live);
        for name in ["agent.log", "error.log", "task.log"] {
            assert_eq!(classify(Source::Agent, name), FileKind::Live, "{name}");
        }
    }

    /// The other component's live name is not a live name here: the two trees are
    /// read with the same code, and a shared classifier must not call the Agent's
    /// file "live" when listing Desktop's tree.
    #[test]
    fn one_components_live_name_is_not_the_others() {
        assert_eq!(classify(Source::Agent, "desktop.log"), FileKind::Other);
        assert_eq!(classify(Source::Desktop, "agent.log"), FileKind::Other);
    }

    #[test]
    fn an_archive_is_recognised_by_the_stamp_the_rotator_writes() {
        assert_eq!(
            classify(Source::Desktop, "desktop.log.2026-09-24-19"),
            FileKind::Archive
        );
        assert_eq!(
            classify(Source::Agent, "error.log.2026-01-02-03"),
            FileKind::Archive
        );
    }

    /// A second rotation inside one hour is a `<live>.<stamp>.<n>` file, and the
    /// index is split off at the **first** dot — the crate's rule, not the last
    /// one. The stamp the index leaves behind is the one it is ordered and aged
    /// by, so a reader that guessed wrong here would take `24-19.1` as the stamp.
    #[test]
    fn a_collision_index_is_an_archive_and_leaves_the_stamp_behind_it() {
        assert_eq!(
            classify(Source::Desktop, "desktop.log.2026-09-24-19.1"),
            FileKind::Archive
        );
        assert_eq!(
            archive_stamp("desktop.log", "desktop.log.2026-09-24-19.1"),
            Some("2026-09-24-19")
        );
        // The whole name is `TooLong` for the format on its own; the crate's split
        // is the only reason it is recognised at all.
        assert_eq!(
            chrono::NaiveDateTime::parse_from_str("2026-09-24-19.1", ARCHIVE_FORMAT)
                .map(|_| ())
                .map_err(|error| error.kind()),
            Err(chrono::format::ParseErrorKind::TooLong)
        );
    }

    /// Where the mirror's line is: which stamp shapes the *crate* calls its own.
    ///
    /// Written as a table because the interesting part is the two kinds that look
    /// like they should be accepted and are not. `NotEnough` is accepted (the
    /// crate accepts it, and it is what `%Y-%m-%d-%H` produces); `TooShort` — a
    /// name that stops mid-stamp — is not, which is the case I first got wrong by
    /// assuming "a prefix of a stamp is a stamp".
    #[test]
    fn only_the_stamp_shapes_the_crate_accepts_are_archives() {
        let cases = [
            ("2026-09-24-19", true, "a complete name for this format"),
            ("2026-09", false, "TooShort: the format is not consumed"),
            ("2026-09-24", false, "TooShort: no hour"),
            ("2026", false, "TooShort: nothing after the year"),
            ("2026-09-24-19-00", false, "TooLong: input left over"),
            ("nonsense", false, "Invalid"),
        ];
        for (stamp, expected, why) in cases {
            let name = format!("desktop.log.{stamp}");
            let kind = classify(Source::Desktop, &name);
            let wanted = if expected {
                FileKind::Archive
            } else {
                FileKind::Other
            };
            assert_eq!(kind, wanted, "{stamp} ({why})");
            assert_eq!(
                archive_stamp("desktop.log", &name).is_some(),
                expected,
                "{stamp} ({why})"
            );
        }
    }

    /// Names that are not the rotator's are "other", including the two that look
    /// closest to being one.
    #[test]
    fn names_the_rotator_never_writes_are_other() {
        for name in [
            // The pre-T-02 shape, which exists on a machine that ran an older build.
            "desktop-20260924-1.log",
            // A dot-N index whose stamp is not a stamp.
            "desktop.log.yesterday.1",
            // A dot-N index that is not a number.
            "desktop.log.2026-09-24-19.x",
            // A bare suffix with no stamp at all.
            "desktop.log.tmp",
            // Someone's unrelated file.
            "notes.txt",
        ] {
            assert_eq!(classify(Source::Desktop, name), FileKind::Other, "{name}");
        }
    }

    /// Listing an absent directory is an empty list — the Agent that never ran —
    /// and must not create it.
    #[test]
    fn an_absent_directory_lists_as_empty_and_is_not_created() {
        let root = scratch("listing-absent");
        let missing = root.join("never-written");

        let listed = list(&missing, Source::Agent);

        let created = missing.exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(listed.expect("an empty list").is_empty());
        assert!(!created);
    }

    /// An unreadable directory is an error, not an empty listing: "no logs" and
    /// "logs I cannot see" must not look the same on the page.
    #[test]
    fn an_unreadable_directory_is_an_error_rather_than_empty() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("listing-unreadable");
        std::fs::write(root.join("desktop.log"), b"x").expect("plant");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).expect("chmod");

        let readable = std::fs::read_dir(&root).is_ok();
        let listed = if readable {
            None
        } else {
            Some(list(&root, Source::Desktop))
        };

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).ok();
        std::fs::remove_dir_all(&root).ok();

        match listed {
            None => eprintln!("premise failed: this process reads a 0o000 directory"),
            Some(listed) => assert!(
                matches!(listed, Err(LogReadError::Unreadable { .. })),
                "expected an error, got {listed:?}"
            ),
        }
    }

    /// The order a viewer wants: the live file, then the archives newest first,
    /// then anything unrecognised.
    #[test]
    fn the_listing_is_live_then_newest_archive_then_the_unknown() {
        let root = scratch("listing-order");
        for name in [
            "desktop.log.2026-09-24-18",
            "desktop.log.2026-09-24-20",
            "desktop.log.2026-09-24-19",
            "desktop.log",
            "desktop-20260924-1.log",
        ] {
            std::fs::write(root.join(name), b"x").expect("plant");
        }

        let listed = list(&root, Source::Desktop).expect("a readable tree");
        let names: Vec<&str> = listed.iter().map(|file| file.name.as_str()).collect();
        let sizes: Vec<u64> = listed.iter().map(|file| file.bytes).collect();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            names,
            vec![
                "desktop.log",
                "desktop.log.2026-09-24-20",
                "desktop.log.2026-09-24-19",
                "desktop.log.2026-09-24-18",
                "desktop-20260924-1.log",
            ]
        );
        assert_eq!(sizes, vec![1, 1, 1, 1, 1], "the size is reported per file");
    }

    /// The Agent has three live files, and their order is the name's rather than
    /// the directory's: `read_dir` returns them in whatever order the filesystem
    /// chose, so a viewer that took that order would show the three in a
    /// different order on another machine — or after a delete and a rewrite.
    #[test]
    fn the_agents_three_live_files_are_listed_by_name() {
        let root = scratch("listing-live-names");
        for name in ["task.log", "agent.log", "error.log"] {
            std::fs::write(root.join(name), b"x").expect("plant");
        }

        let listed = list(&root, Source::Agent).expect("a readable tree");
        std::fs::remove_dir_all(&root).ok();

        let names: Vec<&str> = listed.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, vec!["agent.log", "error.log", "task.log"]);
        assert!(
            listed.iter().all(|file| file.kind == FileKind::Live),
            "all three are the Agent's own live names: {listed:?}"
        );
    }

    /// A subdirectory in the tree is not a log file, and the listing does not
    /// walk into it.
    #[test]
    fn a_subdirectory_is_not_listed() {
        let root = scratch("listing-subdir");
        std::fs::write(root.join("desktop.log"), b"x").expect("plant");
        std::fs::create_dir_all(root.join("archive/desktop.log.2026-09-24-19")).expect("plant dir");

        let listed = list(&root, Source::Desktop).expect("a readable tree");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(listed.len(), 1, "{listed:?}");
    }

    #[test]
    fn a_tail_is_the_last_lines_without_the_trailing_newline() {
        let root = scratch("tail");
        let path = root.join("desktop.log");
        let text: String = (1..=10)
            .map(|n| {
                format!(
                    "{}\n",
                    record("2026-09-24T20:00:00", "INFO", &format!("line {n}"))
                )
            })
            .collect();
        std::fs::write(&path, text).expect("plant");

        let tail = tail(&path, 3);
        std::fs::remove_dir_all(&root).ok();

        let tail = tail.expect("readable");
        assert!(!tail.truncated);
        assert_eq!(tail.lines.len(), 3);
        assert!(tail.lines[2].ends_with("line 10"), "{:?}", tail.lines);
        assert!(tail.lines[0].ends_with("line 8"), "{:?}", tail.lines);
    }

    /// More lines asked for than exist is the whole file, not an error and not
    /// padded.
    #[test]
    fn asking_for_more_lines_than_there_are_gives_what_there_is() {
        let root = scratch("tail-short");
        let path = root.join("desktop.log");
        std::fs::write(&path, "one\ntwo\n").expect("plant");

        let tail = tail(&path, 500);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(tail.expect("readable").lines, vec!["one", "two"]);
    }

    /// The tail crosses chunk boundaries intact.
    ///
    /// 8 KiB per chunk, so a file larger than that forces more than one read
    /// backwards — which is where an off-by-one in the window would show up.
    #[test]
    fn a_tail_longer_than_one_chunk_keeps_its_lines_whole() {
        let root = scratch("tail-chunks");
        let path = root.join("desktop.log");
        let mut text = String::new();
        for n in 1..=2000 {
            text.push_str(&record(
                "2026-09-24T20:00:00",
                "INFO",
                &format!("record {n:04} {}", "x".repeat(20)),
            ));
            text.push('\n');
        }
        assert!(
            text.len() as u64 > TAIL_CHUNK_BYTES * 3,
            "the premise: a multi-chunk file"
        );
        std::fs::write(&path, text).expect("plant");

        let tail = tail(&path, 5);
        std::fs::remove_dir_all(&root).ok();

        let tail = tail.expect("readable");
        assert_eq!(tail.lines.len(), 5);
        assert!(tail.lines[4].contains("record 2000"), "{:?}", tail.lines[4]);
        assert!(tail.lines[0].contains("record 1996"), "{:?}", tail.lines[0]);
        assert!(
            tail.lines
                .iter()
                .all(|line| line.starts_with("2026-09-24T20:00:00 [INFO]")),
            "no line may be a fragment: {:?}",
            tail.lines
        );
    }

    /// A file over the byte ceiling still yields whole lines, and says that the
    /// view was cut short by the ceiling rather than by the file ending.
    ///
    /// The numbers are geometry, not decoration. Five records of 1.5 MiB make a
    /// 7.5 MiB file; the window is the last 4 MiB (plus at most one 8 KiB chunk),
    /// so it begins 3.5 MiB in — half a megabyte past the start of record 3 and a
    /// megabyte before its end. That margin is what keeps the expected count from
    /// depending on how much the last chunk overshot: with a record length that
    /// divides the ceiling evenly, the window head lands *on* a boundary and the
    /// answer changes with the overshoot. The first record in the window is the
    /// one the window cut in half, so it is dropped and the two after it are the
    /// whole lines this expects.
    #[test]
    fn a_tail_stopped_by_the_byte_ceiling_keeps_whole_lines_and_says_so() {
        let root = scratch("tail-ceiling");
        let path = root.join("desktop.log");
        let record_bytes = 1024 * 1024 + 512 * 1024;
        let mut text = String::new();
        for n in 1..=5 {
            text.push_str(&record(
                "2026-09-24T20:00:00",
                "ERROR",
                &format!("record {n} "),
            ));
            text.push_str(&"y".repeat(record_bytes));
            text.push('\n');
        }
        std::fs::write(&path, text).expect("plant");
        let premise =
            std::fs::metadata(&path).expect("planted").len() > TAIL_MAX_BYTES + record_bytes as u64;

        let tail = tail(&path, 10);
        std::fs::remove_dir_all(&root).ok();

        assert!(premise, "the premise: a file well over the ceiling");
        let tail = tail.expect("readable");
        assert!(tail.truncated, "the ceiling must be reported");
        assert_eq!(tail.lines.len(), 2, "the whole records the window holds");
        assert!(
            tail.lines[0].contains("record 4"),
            "{:?}",
            &tail.lines[0][..40]
        );
        assert!(
            tail.lines[1].contains("record 5"),
            "{:?}",
            &tail.lines[1][..40]
        );
        assert!(tail.lines.iter().all(|line| line.ends_with('y')));
    }

    /// The one file that reads as no lines: a single record longer than the
    /// ceiling. Its last ten lines are that record, and that record is what the
    /// ceiling refused to read — so the empty view plus the flag is the honest
    /// pair, and the page has to say so rather than looking like an empty file.
    #[test]
    fn one_record_longer_than_the_ceiling_reads_as_no_lines_and_a_flag() {
        let root = scratch("tail-one-record");
        let path = root.join("desktop.log");
        let mut text = record("2026-09-24T20:00:00", "ERROR", "start");
        text.push_str(&"y".repeat(TAIL_MAX_BYTES as usize + 1024));
        text.push('\n');
        std::fs::write(&path, text).expect("plant");

        let tail = tail(&path, 10);
        std::fs::remove_dir_all(&root).ok();

        let tail = tail.expect("readable");
        assert!(tail.truncated, "the ceiling must be reported");
        assert!(
            tail.lines.is_empty(),
            "the fragment is dropped, not shown as a line: {:?}",
            tail.lines.len()
        );
    }

    /// A file that vanished between listing and reading is an error, so the page
    /// can say to refresh rather than showing an empty view.
    #[test]
    fn a_tail_of_a_missing_file_is_an_error() {
        let root = scratch("tail-missing");
        let missing = root.join("desktop.log");

        let tail = tail(&missing, 10);
        std::fs::remove_dir_all(&root).ok();

        assert!(
            matches!(tail, Err(LogReadError::Unreadable { .. })),
            "{tail:?}"
        );
    }

    /// Files with no trailing newline, empty files, and a single line: the three
    /// edges a split-based tail gets wrong if it assumes a trailing newline.
    #[test]
    fn the_tails_edges_are_handled() {
        let root = scratch("tail-edges");

        let no_newline = root.join("a"); // one line, unterminated
        std::fs::write(&no_newline, "only line").expect("plant");
        let empty = root.join("b");
        std::fs::write(&empty, "").expect("plant");
        let blank = root.join("c"); // an interior blank line is real
        std::fs::write(&blank, "first\n\nthird\n").expect("plant");

        let results = (tail(&no_newline, 10), tail(&empty, 10), tail(&blank, 10));
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(results.0.expect("readable").lines, vec!["only line"]);
        assert!(results.1.expect("readable").lines.is_empty());
        assert_eq!(
            results.2.expect("readable").lines,
            vec!["first", "", "third"]
        );
    }

    /// Lines are given the level of the record they belong to, and a record's own
    /// line is marked as not a continuation.
    #[test]
    fn a_continuation_line_inherits_the_record_above_it() {
        let lines = vec![
            record("2026-09-24T20:00:00", "INFO", "starting"),
            "  File \"/somewhere.py\", line 3, in run".to_string(),
            "    raise ValueError".to_string(),
            record("2026-09-24T20:00:01", "ERROR", "failed"),
            "ValueError: bad".to_string(),
        ];

        let entries = entries(&lines);

        assert_eq!(entries[0].level, Some(Level::Info));
        assert!(!entries[0].continues);
        assert_eq!(entries[1].level, Some(Level::Info));
        assert!(entries[1].continues);
        assert_eq!(entries[2].level, Some(Level::Info));
        assert_eq!(entries[3].level, Some(Level::Error));
        assert!(!entries[3].continues);
        assert_eq!(entries[4].level, Some(Level::Error));
        assert!(entries[4].continues);
    }

    /// A line before any record that carried a level has no level, rather than a
    /// guessed one.
    #[test]
    fn a_line_before_the_first_record_has_no_level() {
        let lines = vec![
            "garbage at the top of the file".to_string(),
            record("2026-09-24T20:00:00", "INFO", "first record"),
        ];

        let entries = entries(&lines);

        assert_eq!(entries[0].level, None);
        assert!(entries[0].continues, "it is not a record of its own either");
        assert_eq!(entries[1].level, Some(Level::Info));
    }

    /// Both spellings, from the lines the two components really write.
    #[test]
    fn both_components_level_spellings_are_recognised() {
        // The Desktop line, verbatim from a real `.local/logs/desktop.log`.
        let desktop = "2026-09-24T20:47:36 [INFO] desktop.startup: 配置来自开发树 resources/";
        // The Agent line, verbatim from a real `~/Library/Logs/WTMedia/Agent/agent.log`.
        let agent =
            "2026-09-24T19:49:44 [WARNING] wt_media_agent.local_api.server: profile_open.failure";

        assert_eq!(record_level(desktop), Some(Level::Info));
        assert_eq!(record_level(agent), Some(Level::Warn));
        assert_eq!(
            record_level(&record("2026-01-01T00:00:00", "CRITICAL", "x")),
            Some(Level::Error)
        );
        assert_eq!(
            record_level(&record("2026-01-01T00:00:00", "TRACE", "x")),
            Some(Level::Trace)
        );
    }

    /// A message that mentions a level is not a second record.
    ///
    /// This is what the fixed-position parse buys: the stamp has to be shaped
    /// like a stamp, so `[INFO]` inside a message is just text.
    #[test]
    fn a_level_inside_a_message_is_not_a_record() {
        let line = record(
            "2026-09-24T20:00:00",
            "ERROR",
            "the agent answered [INFO] which is not what was asked",
        );
        assert_eq!(record_level(line.as_str()), Some(Level::Error));

        for not_a_record in [
            "the agent said [INFO] at some point",
            "2026-09-24T20:00:00[INFO] no space before the bracket",
            "2026-09-24T20:00:00 [NOTALEVEL] invented",
            "2026-99-99T99:99:99 [INFO] digits, but the shape is all this checks",
            "2026-09-24T20:00:00 [INFO]no space after the bracket",
        ] {
            let level = record_level(not_a_record);
            if not_a_record.starts_with("2026-99") {
                // Registered: the shape check is a shape check, not a calendar.
                assert_eq!(level, Some(Level::Info), "{not_a_record}");
            } else {
                assert_eq!(level, None, "{not_a_record}");
            }
        }
    }

    /// The filter keeps blocks together and never hides what it cannot classify.
    #[test]
    fn the_filter_keeps_blocks_whole_and_never_hides_the_unclassified() {
        let lines = vec![
            "a line before any record".to_string(),
            record("2026-09-24T20:00:00", "DEBUG", "noisy"),
            "  a continuation of the noisy debug record".to_string(),
            record("2026-09-24T20:00:01", "ERROR", "visible"),
            "  a continuation of the error".to_string(),
        ];
        let entries = entries(&lines);

        let errors_only = filter_min(&entries, Level::Error);
        let texts: Vec<&str> = errors_only.iter().map(|e| e.text.as_str()).collect();

        assert_eq!(texts.len(), 3, "{texts:?}");
        assert!(texts[0].contains("before any record"), "{texts:?}");
        assert!(texts[1].contains("visible"), "{texts:?}");
        assert!(texts[2].contains("continuation of the error"), "{texts:?}");

        // And the opposite direction: `trace` keeps everything, because nothing
        // is quieter than the quietest level.
        assert_eq!(filter_min(&entries, Level::Trace).len(), lines.len());
    }

    /// The page's own spelling parses, and an unknown one is `None` rather than a
    /// silent default — a filter that fell back to `Info` would drop records the
    /// caller asked for.
    #[test]
    fn the_page_spelling_parses_and_an_unknown_one_does_not() {
        assert_eq!(Level::from_str_opt(" info "), Some(Level::Info));
        assert_eq!(Level::from_str_opt("WARNING"), Some(Level::Warn));
        assert_eq!(Level::from_str_opt("critical"), Some(Level::Error));
        assert_eq!(Level::from_str_opt("verbose"), None);
        assert_eq!(Level::from_str_opt(""), None);
        for level in [
            Level::Trace,
            Level::Debug,
            Level::Info,
            Level::Warn,
            Level::Error,
        ] {
            assert_eq!(Level::from_str_opt(level.as_str()), Some(level));
        }
    }

    /// The levels order from quietest to loudest, which is what the filter's
    /// comparison means.
    #[test]
    fn the_levels_are_ordered() {
        assert!(Level::Trace < Level::Debug);
        assert!(Level::Debug < Level::Info);
        assert!(Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
    }
}
