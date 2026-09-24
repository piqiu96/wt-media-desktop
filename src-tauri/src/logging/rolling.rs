//! The live log file, its hourly history, and the one record that must not grow.
//!
//! **The rotator is `file-rotate`, not ours** (the user's ruling D-02: 尽量使用
//! 开源，尽可能不改轮子). This module used to carry a hand-written rotator --
//! a date-and-index name, a `create_new` name claim, an age rule and a byte
//! budget. The ruling that replaced it (D-01/D-03) asks for exactly what the
//! crate already does, so what is left here is the wrapper and the one bound the
//! crate does not have.
//!
//! The shape, on both sides of the product (ruling D-01):
//!
//! ```text
//! desktop.log                        <- stable, the file being written
//! desktop.log.2026-09-24-19          <- the hour that ended, local time
//! ```
//!
//! The crate was chosen over `tracing-appender` for one measured reason: it is
//! the only one of the two with **rename-on-roll**. `tracing-appender` opens the
//! dated file directly, so `Rotation::HOURLY` and a stable name cannot both be
//! had from it. `file-rotate` also brings `FileLimit::Age`, which is **real
//! age-based deletion** -- measured, not taken from its docs: planting archives
//! 20, 15 and 2 days old and rotating once deletes the first two and keeps the
//! third. That is what removed the last hand-rolled rule.
//!
//! Four measured consequences, all of them registered rather than implied:
//!
//! - **The hour is local time.** The crate formats `chrono::Local` and there is
//!   no UTC switch, so an archive is named in the operator's own clock. The
//!   record's own stamp follows it (see `backend::stamp`) -- a UTC stamp on a
//!   local-named file disagrees with itself for eight hours a day.
//! - **The clock cannot be injected, and the trigger is read once.** `mock_time`
//!   is `#[cfg(test)]` inside the crate, so a test cannot stand on an hour
//!   boundary. What it can do is leave a live file whose mtime is an hour old
//!   *before* the writer is built, which is exactly what the crate reads: it
//!   seeds its remembered instant from that mtime in `new()` and overwrites it
//!   on every write, so backdating afterwards does nothing at all. That is why
//!   every rotation test here builds the previous hour's file first -- not a
//!   convenience, but the only shape in which this path can be driven.
//! - **The archive set is scanned once, at construction.** A file that appears
//!   afterwards is not deleted by *this* writer, and is picked up by the next
//!   one -- both halves are pinned by
//!   `an_archive_is_deleted_by_the_next_writer_and_not_by_the_one_that_missed_it`,
//!   because a reader who assumes either one alone would believe a real gap was
//!   a bug.
//! - **One live file means one writer.** Two instances share `desktop.log` and
//!   the crate renames it out from under whichever handle is still writing. That
//!   is why the single-instance guard is load-bearing rather than a nicety
//!   (ruling D-04).
//!
//! What is **ours** is the truncation marker: a single record that cannot fit is
//! cut and marked with the size it had. It is orthogonal to rotation and it is
//! the ruling's 超长单条 rule (CHG-057 D-05), which the cap ruling did not
//! revoke -- it only removed the number the old threshold came from. The number
//! is now its own, much larger, and configurable.

use std::io::{Error as IoError, ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use file_rotate::compression::Compression;
use file_rotate::suffix::{AppendTimestamp, DateFrom, FileLimit};
use file_rotate::{ContentLimit, FileRotate, TimeFrequency};

/// The file being written. Stable, so a reader has one name to look at and the
/// archives sort beside it (ruling D-01).
pub const LOG_FILE_NAME: &str = "desktop.log";

/// How an archive is stamped: the live file's name, a dot, and the hour that
/// ended, in local time. Zero-padded, so lexicographic order is chronological
/// order -- which is what the crate's age comparison relies on.
pub const ARCHIVE_FORMAT: &str = "%Y-%m-%d-%H";

/// How a record's own stamp is spelled (`backend::stamp`), in local time.
///
/// Kept beside [`ARCHIVE_FORMAT`] on purpose: one is the name of the file and
/// the other is the first field of every line in it, and a reader who has to
/// hold two modules in their head to see that the two agree about the clock will
/// eventually stop checking. The difference between them is only the hour
/// separator, because a name may not contain a colon on Windows and a record is
/// free to.
pub const STAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// How many days of history survive, as shipped. A 14-day window is today plus
/// the 13 days before it. Raised or lowered by `[logging] retention_days`; the
/// ruling names 7 and 14 as the two acceptable numbers (D-03/Q-02).
pub const DEFAULT_RETENTION_DAYS: i64 = 14;

/// How long one record may be, as shipped.
///
/// Not the ruling's number -- the ruling cancelled the cap it used to be (D-03),
/// and a single record still must not be able to grow a file without bound
/// (CHG-057 D-05). 1 MiB is far above any real record (a webview stack trace is
/// kilobytes) and is configurable, so the threshold can be lowered without a
/// code change. Registered as CHG-058 Q-01.
pub const DEFAULT_MAX_RECORD_BYTES: u64 = 1024 * 1024;

/// What a truncated record is marked with. Byte-for-byte the Agent's marker
/// (`runtime/logging.py`), because this is one ruling read by one person: a
/// reader who has learned the marker in `error.log` must not meet a second
/// spelling in `desktop.log`.
pub const TRUNCATION_MARKER: &str = " truncate=true original_size=";

/// How long history is kept, and how long one record may be.
///
/// Two fields where there used to be three: the byte budget across the directory
/// is gone (D-03 -- "不需要控制总量"), and so is the per-file cap that the record
/// cap used to be derived from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Days of archives to keep. Deletion past this is the crate's rule.
    pub retention_days: i64,
    /// The longest a single record may be before it is cut and marked.
    pub max_record_bytes: u64,
}

impl Limits {
    /// The shipped numbers. `[logging]` (T-14) may change them; what it may not
    /// do is ship something else without the baselines changing too.
    pub const SHIPPED: Limits = Limits {
        retention_days: DEFAULT_RETENTION_DAYS,
        max_record_bytes: DEFAULT_MAX_RECORD_BYTES,
    };
}

/// A record as it will be written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fitted {
    /// The line, without its newline.
    pub line: String,
    /// The byte length the record *had*, when it did not fit. `None` means the
    /// line went in whole -- the only way a reader can tell a truncated record
    /// from a short one.
    pub original_size: Option<u64>,
}

/// The record as it will be written: cut to one record's worth, and marked.
pub fn fit(line: &str, max_record_bytes: u64) -> Fitted {
    // At least one byte, so a caller that skipped the configuration layer's
    // validation cannot hand the arithmetic a zero.
    let limit = max_record_bytes.max(1) - 1; // the newline that ends the line
    let raw = line.as_bytes();
    if raw.len() as u64 <= limit {
        return Fitted {
            line: line.to_owned(),
            original_size: None,
        };
    }

    let marker = format!("{}{}", TRUNCATION_MARKER, raw.len());
    // A cap too small to hold the marker leaves no room for the head. The marker
    // is still written: a record marked as truncated and over an unusable cap is
    // more use than a record with no size information at all, and the
    // configuration layer rejects caps that small (T-14).
    let room = (limit as usize).saturating_sub(marker.len());
    let head = &raw[..room.min(raw.len())];
    // The cut is on a byte boundary and may land inside a character. Dropping
    // that one character is the point: emitting half of it would put a
    // replacement byte into a plain-text file.
    let head = match std::str::from_utf8(head) {
        Ok(text) => text,
        // `valid_up_to` is by definition the end of the valid prefix, so this
        // second conversion cannot fail.
        Err(error) => std::str::from_utf8(&head[..error.valid_up_to()]).unwrap_or(""),
    };
    Fitted {
        line: format!("{head}{marker}"),
        original_size: Some(raw.len() as u64),
    }
}

/// Why the log file could not be written. Holds no credentials: a log path is a
/// path.
#[derive(Debug)]
pub struct RollingError {
    pub path: PathBuf,
    pub reason: IoError,
}

impl std::fmt::Display for RollingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot write {}: {}", self.path.display(), self.reason)
    }
}

impl std::error::Error for RollingError {}

/// The live log file and the rotation policy over its directory.
///
/// Every failure is a returned `Err`, never a panic: the caller's response to
/// all of them is the same -- say so on stderr and carry on, because the user's
/// ruling 五 is that logging may never be the reason a launch fails. The one
/// place that rule needs saying out loud is [`Writer::open`], because the crate
/// underneath *panics* on a directory it cannot create and *silently drops every
/// record* when its file cannot be opened. Both are asked about here, before
/// there is nothing left to ask.
pub struct Writer {
    directory: PathBuf,
    path: PathBuf,
    limits: Limits,
    log: FileRotate<AppendTimestamp>,
}

impl Writer {
    /// A writer for this directory.
    ///
    /// `directory` must be a real path with a parent -- the crate unwraps the
    /// parent -- and the caller is expected to have proved it writable already
    /// (`logging::paths::prepare`). The check is repeated here anyway, because
    /// this function's contract is "never panic and never lose a record
    /// quietly", and a contract that depends on a caller's earlier call is a
    /// contract with a hole in it.
    pub fn open(directory: &Path, limits: Limits) -> Result<Writer, RollingError> {
        let refused = |reason: IoError, path: PathBuf| RollingError { path, reason };
        if let Err(reason) = std::fs::create_dir_all(directory) {
            return Err(refused(reason, directory.to_path_buf()));
        }
        let path = directory.join(LOG_FILE_NAME);

        let log = FileRotate::new(
            &path,
            AppendTimestamp::with_format(
                ARCHIVE_FORMAT,
                // The crate's own age rule. `chrono::Duration`, not
                // `std::time::Duration`: that is what `FileLimit::Age` takes,
                // and it is why `chrono` is a direct dependency.
                FileLimit::Age(chrono::Duration::days(limits.retention_days)),
                DateFrom::DateHourAgo,
            ),
            ContentLimit::Time(TimeFrequency::Hourly),
            Compression::None,
            None,
        );

        // `FileRotate` holds `None` in place of its file when the open failed,
        // and every later write then reports success while writing nothing. That
        // is the one failure this wrapper must not inherit, so it is asked about
        // here -- while there is still a caller to tell.
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => Ok(Writer {
                directory: directory.to_path_buf(),
                path,
                limits,
                log,
            }),
            Ok(_) => Err(refused(
                IoError::new(
                    ErrorKind::AlreadyExists,
                    "a directory is where the log file should be",
                ),
                path,
            )),
            Err(reason) => Err(refused(reason, path)),
        }
    }

    /// The directory this writer's files live in.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The file this writer is appending to. These days that is one stable name
    /// for the whole run rather than one per day (D-01).
    pub fn open_path(&self) -> Option<&Path> {
        Some(self.path.as_path())
    }

    /// Write one record. The line must not contain a newline.
    ///
    /// The record is cut to `max_record_bytes` first and handed over whole: the
    /// crate decides when to rotate, and by the time it sees the line the line
    /// is already one that fits.
    pub fn write_line(&mut self, line: &str) -> Result<(), RollingError> {
        let fitted = fit(line, self.limits.max_record_bytes);
        writeln!(self.log, "{}", fitted.line)
            // Flushed per record, not buffered: a log that only reaches the disk
            // on a clean exit is no use for the crash it was written to explain,
            // and Desktop's volume is a handful of records per launch.
            .and_then(|()| self.log.flush())
            .map_err(|reason| RollingError {
                path: self.path.clone(),
                reason,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    /// A scratch directory of this test's own, named after the process so two
    /// concurrent runs cannot collide. Removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-rolling-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        path
    }

    /// Everything in the directory, sorted, so an assertion can name it exactly.
    fn listing(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// The rolled files, by the same rule the crate uses to recognise one: the
    /// live name, a dot, and something that starts like a stamp. `desktop.log.old`
    /// is deliberately not one of them, and neither is another component's file.
    fn archives(directory: &Path) -> Vec<String> {
        let prefix = format!("{LOG_FILE_NAME}.");
        listing(directory)
            .into_iter()
            .filter(|name| {
                name.strip_prefix(&prefix)
                    .is_some_and(|stamp| stamp.starts_with(|c: char| c.is_ascii_digit()))
            })
            .collect()
    }

    fn shipped() -> Limits {
        Limits::SHIPPED
    }

    /// A stamp `hours` before now, as the archive names spell it.
    ///
    /// The format is written out here rather than read from [`ARCHIVE_FORMAT`]
    /// on purpose: an expectation built from the constant it is checking moves
    /// with the constant and cannot fail. Measured -- with `.format(ARCHIVE_FORMAT)`
    /// here, mutating the constant to `%Y-%m-%d` was caught by nothing but a
    /// slice panic in a sibling test.
    fn stamp(hours_ago: i64) -> String {
        (chrono::Local::now() - chrono::Duration::hours(hours_ago))
            .format("%Y-%m-%d-%H")
            .to_string()
    }

    /// Move a file's mtime back. `set_modified` has been stable since Rust 1.75,
    /// so this needs no dev-dependency on `filetime`.
    fn backdate(path: &Path, hours: i64) {
        let when: SystemTime = (chrono::Local::now() - chrono::Duration::hours(hours)).into();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("the file to backdate")
            .set_modified(when)
            .expect("set the mtime");
    }

    /// A live file as a run that stopped in a previous hour leaves it: some
    /// records, and an mtime an hour old.
    ///
    /// This is the only lever a test has on the rotation trigger, and it is the
    /// **real** one, not a stand-in. The crate compares the wall clock's hour
    /// against the live file's mtime -- but it reads that mtime **once**, when
    /// the writer is built, and overwrites its remembered value on every write
    /// (measured: `ensure_log_directory_exists` seeds `self.modified`, and the
    /// `Time` arm of `write` then sets it to the hour it just wrote in). So
    /// backdating the file *after* a writer exists does nothing at all, which is
    /// why this file has to be created before the writer that should rotate it.
    ///
    /// Creating it first is not a testing convenience: it is the production
    /// scenario, a process that last wrote in the previous hour and has just
    /// been launched again -- which is also the launch on which the age rule
    /// runs, since the age rule runs inside a rotation and nowhere else.
    fn live_file_from_the_previous_hour(directory: &Path) -> PathBuf {
        std::fs::create_dir_all(directory).expect("scratch");
        let live = directory.join(LOG_FILE_NAME);
        std::fs::write(&live, "the hour that ended\n").expect("the live file");
        backdate(&live, 1);
        live
    }

    /// Plant an archive `days` old, before the writer exists.
    fn plant_archive(directory: &Path, days: i64, contents: &str) -> PathBuf {
        std::fs::create_dir_all(directory).expect("scratch");
        let path = directory.join(format!(
            "{}.{}",
            LOG_FILE_NAME,
            (chrono::Local::now() - chrono::Duration::days(days)).format(ARCHIVE_FORMAT)
        ));
        std::fs::write(&path, contents).expect("plant the archive");
        path
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("the file")
    }

    /// The refusal, or a panic saying the directory should have been refused.
    ///
    /// Not `expect_err`: `FileRotate` is not `Debug`, so the `Ok` arm has no
    /// printable form and `Err(error)` is unwrapped by hand.
    fn refused(directory: &Path) -> RollingError {
        match Writer::open(directory, shipped()) {
            Ok(_) => panic!("{} must be refused", directory.display()),
            Err(error) => error,
        }
    }

    // ---- one record, cut to fit (pure: no filesystem, no clock) ----

    #[test]
    fn a_record_that_fits_is_written_whole_and_unmarked() {
        let fitted = fit("hello", 200);
        assert_eq!(fitted.line, "hello");
        assert_eq!(fitted.original_size, None);
    }

    #[test]
    fn a_record_at_exactly_the_cap_is_still_whole() {
        // The newline has to fit too, so the cap for the text is one less.
        let line = "x".repeat(199);
        let fitted = fit(&line, 200);
        assert_eq!(fitted.original_size, None, "199 + newline is exactly 200");

        let over = "x".repeat(200);
        let fitted = fit(&over, 200);
        assert_eq!(fitted.original_size, Some(200), "200 + newline is 201");
    }

    #[test]
    fn an_oversized_record_is_truncated_and_says_how_long_it_was() {
        let line = "y".repeat(5_000);
        let fitted = fit(&line, 200);

        assert_eq!(fitted.original_size, Some(5_000));
        assert!(fitted.line.ends_with(" truncate=true original_size=5000"));
        let with_newline = fitted.line.len() + 1;
        assert!(
            with_newline <= 200,
            "the marked record must still fit the cap: {with_newline} bytes"
        );
        assert!(
            fitted.line.starts_with("yyy"),
            "the head is kept, not dropped: {:?}",
            &fitted.line[..8]
        );
    }

    /// The cut is on a byte boundary, so it may land inside a character. What
    /// must not happen is half a character reaching a plain-text file.
    #[test]
    fn truncation_never_leaves_half_a_character() {
        // Three bytes each, so any cut that is not a multiple of three lands
        // inside one.
        let line = "中".repeat(100);
        for cap in 20..60 {
            let fitted = fit(&line, cap);
            assert!(
                fitted.line.is_char_boundary(fitted.line.len()),
                "cap {cap} produced a line that is not valid text"
            );
            assert!(
                !fitted.line.contains('\u{fffd}'),
                "cap {cap} replaced a character instead of dropping it"
            );
        }
    }

    /// A cap smaller than the marker itself leaves no room for the head. The
    /// marker is still written: a record that says it was truncated beats a
    /// record with no size information, and the configuration layer rejects caps
    /// this small (T-14).
    #[test]
    fn a_cap_too_small_for_the_marker_still_marks_the_record() {
        let fitted = fit("a long record", 4);

        assert_eq!(fitted.original_size, Some(13));
        assert_eq!(fitted.line, " truncate=true original_size=13");
    }

    #[test]
    fn the_shipped_limits_are_the_ones_the_ruling_names() {
        assert_eq!(Limits::SHIPPED.retention_days, 14);
        assert_eq!(Limits::SHIPPED.max_record_bytes, 1024 * 1024);
    }

    /// The four spellings the user's ruling fixes, written out as literals.
    ///
    /// Every other test in this module builds its expectation from these
    /// constants or from the file names they produce, which is what makes them
    /// read as tests of behaviour — and what would let a wrong constant sail
    /// through all of them at once. This is the one place the values themselves
    /// are the subject: `desktop.log` is the ruling's stable name, the archives
    /// are that name plus the local hour, and a record's stamp is the same clock
    /// with a colon in the middle (`file-rotate` formats `chrono::Local` and has
    /// no UTC switch, so a UTC stamp here would disagree with the file it sits
    /// in).
    #[test]
    fn the_names_and_the_formats_are_the_ones_the_ruling_names() {
        assert_eq!(LOG_FILE_NAME, "desktop.log");
        assert_eq!(ARCHIVE_FORMAT, "%Y-%m-%d-%H");
        assert_eq!(STAMP_FORMAT, "%Y-%m-%dT%H:%M:%S");
        assert_eq!(
            TRUNCATION_MARKER, " truncate=true original_size=",
            "the marker is byte-for-byte the Agent's, so one reader learns one spelling"
        );
    }

    // ---- the live file, and the record landing in it ----

    #[test]
    fn the_first_record_lands_in_the_stable_live_file() {
        let root = scratch("live");
        let mut writer = Writer::open(&root, shipped()).expect("open");

        writer.write_line("desktop.startup ready").expect("write");

        assert_eq!(read(&root.join(LOG_FILE_NAME)), "desktop.startup ready\n");
        assert_eq!(writer.open_path(), Some(root.join(LOG_FILE_NAME).as_path()));
        std::fs::remove_dir_all(&root).ok();
    }

    /// **A behaviour that changed, pinned rather than only registered.**
    ///
    /// The hand-rolled rotator created nothing until the first record arrived
    /// (its test was `nothing_is_created_before_the_first_record`).
    /// `FileRotate::new` opens the live file eagerly, so `Writer::open` now
    /// leaves an empty `desktop.log` behind even on a launch that logs nothing
    /// -- including `logging.level = "off"`. A consequence worth a test and not
    /// only a comment: the shipped config's own note about it would otherwise be
    /// a claim nothing checks.
    #[test]
    fn the_live_file_exists_as_soon_as_the_writer_does() {
        let root = scratch("eager");

        let writer = Writer::open(&root, shipped()).expect("open");

        assert!(
            root.join(LOG_FILE_NAME).is_file(),
            "the live file is created by the writer, not by the first record"
        );
        assert_eq!(read(&root.join(LOG_FILE_NAME)), "", "and it is empty");
        drop(writer);
        std::fs::remove_dir_all(&root).ok();
    }

    /// One line per record, which is what the plain-text format promises the
    /// reader (and what a grep-based reading of the file depends on).
    #[test]
    fn records_are_one_line_each() {
        let root = scratch("lines");
        let mut writer = Writer::open(&root, shipped()).expect("open");

        writer.write_line("first").expect("write");
        writer.write_line("second").expect("write");

        assert_eq!(read(&root.join(LOG_FILE_NAME)), "first\nsecond\n");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Nothing has rolled yet, because the hour has not turned. Asserted as an
    /// absence so a rotator that rolled on every write -- which would also pass
    /// the rotation test below -- is caught here.
    #[test]
    fn records_in_one_hour_stay_in_one_file() {
        let root = scratch("one-hour");
        let mut writer = Writer::open(&root, shipped()).expect("open");

        for index in 0..5 {
            writer
                .write_line(&format!("record {index}"))
                .expect("write");
        }

        assert_eq!(archives(&root), Vec::<String>::new());
        assert_eq!(read(&root.join(LOG_FILE_NAME)).lines().count(), 5);
        std::fs::remove_dir_all(&root).ok();
    }

    // ---- the hour turning ----

    /// The ruling D-01, end to end: the hour's records are renamed to
    /// `desktop.log.<hour that ended>` and the live file starts over.
    ///
    /// The expected name is bracketed by the clock read either side of the write
    /// rather than read once: the crate takes its own clock reading at write
    /// time, so a tick landing between the two is the one case where the hour
    /// can differ, and both values are therefore correct. Every other run pins
    /// the rule to `hour - 1` exactly -- `DateFrom::Now` would produce the hour
    /// that *began*, which is in the bracket's complement.
    #[test]
    fn a_record_in_a_new_hour_rotates_the_file_the_previous_run_left() {
        let root = scratch("hour-roll");
        live_file_from_the_previous_hour(&root);
        let mut writer = Writer::open(&root, shipped()).expect("open");

        let before = stamp(1);
        writer.write_line("the hour that began").expect("write");
        let after = stamp(1);

        let rolled = archives(&root);
        assert_eq!(
            rolled.len(),
            1,
            "one rotation is one archive, not one per write: {rolled:?}"
        );
        let archive = root.join(&rolled[0]);
        assert!(
            rolled[0].starts_with(&format!("{LOG_FILE_NAME}.")),
            "the archive keeps the live name as its prefix: {:?}",
            rolled[0]
        );
        assert!(
            rolled[0] == format!("{LOG_FILE_NAME}.{before}")
                || rolled[0] == format!("{LOG_FILE_NAME}.{after}"),
            "the archive is named for the hour that ended ({before} or {after}), \
             not for whichever hour the crate happened to read: {:?}",
            rolled[0]
        );

        // The halves are the halves: nothing was lost and nothing was duplicated.
        assert_eq!(read(&archive), "the hour that ended\n");
        assert_eq!(read(&root.join(LOG_FILE_NAME)), "the hour that began\n");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The live file is rebuilt after a rotation rather than left as the archive
    /// it was renamed to -- the property that makes `desktop.log` stable across
    /// hours, and the one a rotator that renamed without reopening would break.
    #[test]
    fn the_live_file_is_recreated_after_a_rotation() {
        let root = scratch("recreate");
        live_file_from_the_previous_hour(&root);
        let mut writer = Writer::open(&root, shipped()).expect("open");

        writer.write_line("after").expect("write");
        writer.write_line("after too").expect("write");

        assert_eq!(read(&root.join(LOG_FILE_NAME)), "after\nafter too\n");
        std::fs::remove_dir_all(&root).ok();
    }

    // ---- what the age rule deletes ----

    /// The ruling D-03, on the real path: `FileLimit::Age` deletes past the
    /// window, keeps inside it, and never touches the live file.
    ///
    /// The archives are planted **before** the writer exists, because the crate
    /// scans the directory once at construction -- see
    /// `an_archive_that_appears_after_the_writer_did_is_not_deleted` for the
    /// other half of that fact. The rotation that runs the deletion is the
    /// ordinary one, triggered by the hour turning, so this is the production
    /// path and not a special "prune now" entry point (there is none).
    ///
    /// The files are 20, 15, 8 and 2 days old rather than 20/15/14/2, because
    /// `too_old` is a strict `<` against `now - 14 days` read from the *crate's*
    /// clock: a file planted exactly on the cutoff would be kept or deleted
    /// depending on which side of an hour tick the two readings fell, and a test
    /// that can flake is a test that gets muted. The exact-cutoff semantics are
    /// registered in the evidence as read-from-source instead. A full day of
    /// separation is what makes the rest of the rows deterministic.
    #[test]
    fn history_past_the_window_is_deleted_and_history_inside_it_is_kept() {
        let root = scratch("age");
        let ancient = plant_archive(&root, 20, "twenty days\n");
        let edge = plant_archive(&root, 15, "fifteen days\n");
        let inside = plant_archive(&root, 8, "eight days\n");
        let recent = plant_archive(&root, 2, "two days\n");
        live_file_from_the_previous_hour(&root);

        let mut writer = Writer::open(&root, shipped()).expect("open");
        writer.write_line("this hour").expect("write");

        assert!(!ancient.exists(), "20 days is out of a 14-day window");
        assert!(!edge.exists(), "15 days is out of a 14-day window");
        assert!(inside.exists(), "8 days is inside the window");
        assert!(recent.exists(), "2 days is inside the window");
        assert!(
            root.join(LOG_FILE_NAME).exists(),
            "the file being written is never history"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The other half of the age rule: it is a *window*, not a reset. A run that
    /// deletes something must leave everything else alone.
    #[test]
    fn the_age_rule_deletes_nothing_when_everything_is_inside_the_window() {
        let root = scratch("age-noop");
        let plantings: Vec<PathBuf> = (1..=3)
            .map(|days| plant_archive(&root, days, &format!("{days} days\n")))
            .collect();
        live_file_from_the_previous_hour(&root);

        let mut writer = Writer::open(&root, shipped()).expect("open");
        writer.write_line("this hour").expect("write");

        for path in &plantings {
            assert!(path.exists(), "{} must survive", path.display());
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The window is the *configured* one, not the shipped one.
    ///
    /// Two-way on purpose: with a 1-day window the 2-day file goes and the same
    /// file stays under the shipped 14. A rule that ignored the configuration
    /// would pass one of these and fail the other.
    #[test]
    fn the_window_is_the_configured_one() {
        let short = Limits {
            retention_days: 1,
            ..shipped()
        };
        for (label, limits, expect_deleted) in
            [("short", short, true), ("shipped", shipped(), false)]
        {
            let root = scratch(label);
            let two_days = plant_archive(&root, 2, "two days\n");
            live_file_from_the_previous_hour(&root);

            let mut writer = Writer::open(&root, limits).expect("open");
            writer.write_line("this hour").expect("write");

            assert_eq!(
                !two_days.exists(),
                expect_deleted,
                "{label}: retention_days={} decides this file's fate",
                limits.retention_days
            );
            std::fs::remove_dir_all(&root).ok();
        }
    }

    /// **Registered boundary, pinned from both sides so it is not mistaken for a
    /// bug later.**
    ///
    /// `file-rotate` scans the directory for archives once, in `new()`, and the
    /// deletion pass only ever walks what it scanned. So a file that arrives
    /// *after* a writer was built is invisible to that writer for the rest of
    /// its life -- and is picked up by the next one, because the next `new()`
    /// scans again. Both halves are asserted here, in one directory, because
    /// either one alone can be read as "the last hour is never cleaned up".
    #[test]
    fn an_archive_is_deleted_by_the_next_writer_and_not_by_the_one_that_missed_it() {
        let root = scratch("late-arrival");
        live_file_from_the_previous_hour(&root);
        let mut first = Writer::open(&root, shipped()).expect("open");

        // Planted after `first` scanned: genuinely too old, and genuinely
        // invisible to the writer that is about to rotate.
        let late = plant_archive(&root, 20, "twenty days, planted late\n");
        first.write_line("this hour").expect("write");
        assert!(
            late.exists(),
            "a file the writer never saw at construction is not its to delete"
        );

        // The next launch scans again, and now the same file is history.
        drop(first);
        backdate(&root.join(LOG_FILE_NAME), 1);
        let mut second = Writer::open(&root, shipped()).expect("open");
        second.write_line("the hour after that").expect("write");

        assert!(
            !late.exists(),
            "the next writer scans the directory again, so the same file is now \
             inside its window rule"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A file that is not ours is not history either: another component's log, a
    /// leftover probe, or a name from a version that spelled it differently.
    ///
    /// The rotation has to actually run for this to mean anything -- otherwise
    /// the files survive because nothing happened -- so the live file is left by
    /// a previous hour, as in the tests above.
    #[test]
    fn a_file_that_is_not_an_archive_is_left_alone() {
        let root = scratch("not-ours");
        std::fs::create_dir_all(&root).expect("scratch");
        let foreign = [
            ("notes.txt", "someone else's"),
            ("agent.log.2026-01-01-00", "another component's"),
            ("desktop.log.old", "no timestamp at all"),
            (".wt-media-write-probe-1", "a write probe left behind"),
        ];
        for (name, contents) in foreign {
            std::fs::write(root.join(name), contents).expect("plant");
        }
        live_file_from_the_previous_hour(&root);

        let mut writer = Writer::open(&root, shipped()).expect("open");
        writer.write_line("this hour").expect("write");

        assert_eq!(
            archives(&root).len(),
            1,
            "exactly one rotation happened, so the deletions below are a real pass \
             over this directory"
        );
        for (name, _) in foreign {
            assert!(root.join(name).exists(), "{name} must survive a prune");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    // ---- the one bound the crate does not have ----

    #[test]
    fn a_record_over_the_cap_is_marked_in_the_file_it_lands_in() {
        let root = scratch("truncate");
        let limits = Limits {
            max_record_bytes: 40,
            ..shipped()
        };
        let mut writer = Writer::open(&root, limits).expect("open");

        writer.write_line(&"z".repeat(500)).expect("write");

        let written = read(&root.join(LOG_FILE_NAME));
        assert!(written.ends_with("\n"), "one line, newline-terminated");
        assert!(written.contains("truncate=true original_size=500"));
        assert_eq!(written.lines().count(), 1, "a cut record is still one line");
        assert!(
            archives(&root).is_empty(),
            "an oversized record is cut, not rolled: rolling it would manufacture \
             a file per record and never make progress"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The shipped cap is not the test's, so the shipped one is asked about
    /// directly: a real record must go in untouched.
    #[test]
    fn an_ordinary_record_is_never_marked_under_the_shipped_cap() {
        let root = scratch("under-cap");
        let mut writer = Writer::open(&root, shipped()).expect("open");

        let line = format!("desktop.startup ready {}", "x".repeat(4_000));
        writer.write_line(&line).expect("write");

        let written = read(&root.join(LOG_FILE_NAME));
        assert!(!written.contains("truncate=true"));
        assert_eq!(written, format!("{line}\n"));
        std::fs::remove_dir_all(&root).ok();
    }

    // ---- failures are values, never panics ----

    /// The ruling 五 at the writer: a directory that cannot be used is an `Err`
    /// naming it, not a panic and not a silent sink.
    ///
    /// A file where the directory should be, rather than a mode bit: this
    /// behaves the same for a root user and in CI (`paths`' precedent). The
    /// crate *panics* on this path (`create_dir_all(...).expect("create dir")`),
    /// which is exactly why the check is ours.
    #[test]
    fn an_unusable_directory_is_an_error_rather_than_a_panic() {
        let root = scratch("occupied");
        std::fs::create_dir_all(&root).expect("scratch");
        let occupied = root.join("logs");
        std::fs::write(&occupied, b"not a directory").expect("occupy the path");

        let error = refused(&occupied);

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            error.path, occupied,
            "the error names the directory it could not use"
        );
        // The *kind* is asserted, not just "some error", and the value is the
        // one measured for `create_dir_all` on a path occupied by a file (T-11
        // measured the same kind for the same call, on this machine).
        assert_eq!(
            error.reason.kind(),
            std::io::ErrorKind::AlreadyExists,
            "the error names the real cause, not a downstream one"
        );
    }

    /// The crate hides one failure: with no open file it reports every write as
    /// successful and drops the bytes. `Writer::open` asks, so a live path
    /// occupied by a directory is refused while there is still a caller to tell.
    #[test]
    fn a_live_path_that_is_a_directory_is_an_error_rather_than_a_silent_sink() {
        let root = scratch("live-is-a-dir");
        std::fs::create_dir_all(root.join(LOG_FILE_NAME)).expect("occupy the live name");

        let error = refused(&root);

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(error.path, root.join(LOG_FILE_NAME), "{error}");
        assert_eq!(error.reason.kind(), std::io::ErrorKind::AlreadyExists);
    }

    /// A directory that does not exist yet is created, not refused: the ordinary
    /// first launch of a build whose log directory was never made by hand.
    #[test]
    fn a_directory_that_does_not_exist_yet_is_created() {
        let root = scratch("create-me").join("nested");

        let mut writer = Writer::open(&root, shipped()).expect("open");

        writer.write_line("first ever record").expect("write");
        assert_eq!(read(&root.join(LOG_FILE_NAME)), "first ever record\n");
        std::fs::remove_dir_all(root.parent().expect("parent")).ok();
    }
}
