//! One plain-text log file, bounded four ways, and the history it leaves behind.
//!
//! The four bounds are the user's ruling 六: a file rolls **on a date change**,
//! rolls when the next record would take it past **20 MB**, truncates a single
//! record that cannot fit and marks it, and defers history to a **14-day /
//! 100 MB** budget. The stdlib-style policy (`backupCount` files of `maxBytes`
//! each) expresses none of them: no time window, no total, and an oversized
//! single record is written whole, taking the file past its own cap.
//!
//! Naming is `desktop-YYYYMMDD-N.log`, UTC, and **every** file carries a date —
//! including the one being written. The Agent has a stable `agent.log` plus
//! dated rolled names; Desktop cannot, because two copies of the app may be open
//! at once and a single stable name would have both of them appending to one
//! file that either may then rename. Instead each writer *claims* its file with
//! `create_new`, so the second instance lands on `-2` rather than on the first
//! instance's file. The claim is the whole reason the index is in the name.
//!
//! The pieces are split the way `paths` is: [`date_of`], [`fit`], [`rotation`]
//! and [`expired`] are pure — injected inputs in, decision out — and [`Writer`]
//! is the thin part that touches the filesystem. A bound that can only be tested
//! by writing files is a bound that gets tested by the launch that needed it.
//! The clock is injected for the same reason: "what happens on the 15th day"
//! has to be answerable without waiting 15 days.
//!
//! Two things are **not** covered, both registered rather than implied:
//!
//! - Windows. The layout question is `paths`' and is already registered; the
//!   separation here (append, rename-free rolling) is portable, but unmeasured.
//! - A second instance that started before midnight and is still running after
//!   it. Its live file is dated yesterday, so the day rule will delete it. Two
//!   instances must be open across midnight for this to bite, and the writer
//!   cannot tell a peer's live file from history. Files dated *today* are the
//!   case it can tell, and it spares those unless this writer rolled them
//!   itself; see [`expired`].

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The name every file starts with.
pub const FILE_STEM: &str = "desktop";

/// The name every file ends with. Kept as `.log` so a log directory full of
/// them still reads as a log directory to whatever is looking.
pub const FILE_SUFFIX: &str = ".log";

/// One file's cap, as shipped (the ruling 六).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// How many days of history survive, as shipped (the ruling 六). A 14-day
/// window is today plus the 13 days before it.
pub const DEFAULT_RETENTION_DAYS: i64 = 14;

/// The whole directory's cap, as shipped (Q-01: Desktop 100 MB, the Agent 400 MB
/// — one budget per component, so the two do not silently share one number).
pub const DEFAULT_TOTAL_BYTES: u64 = 100 * 1024 * 1024;

/// What a truncated record is marked with. Byte-for-byte the Agent's marker
/// (`runtime/logging.py`), because this is one ruling read by one person: a
/// reader who has learned the marker in `error.log` must not meet a second
/// spelling in `desktop-*.log`.
pub const TRUNCATION_MARKER: &str = " truncate=true original_size=";

/// How many files one day may hold before the writer gives up and reports.
///
/// Not a bound the ruling asks for: it exists so that a directory stuffed with
/// `desktop-<today>-1.log … -N.log` cannot spin the claim loop forever. Turning
/// a hang into an error matters more here than the number, which is far above
/// anything a day of Desktop records can reach.
const MAX_INDEX_PER_DAY: u32 = 10_000;

/// Where the writer reads the time. Injected so a test can stand on day 15
/// without waiting for it.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// The clock a real launch uses.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A day in UTC, as the file names and the retention window both spell it.
///
/// UTC rather than local time: a name has to keep its meaning across a timezone
/// change (a laptop that crosses a border does not get a second day), and the
/// window is compared against the same clock the names are written with.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Date {
    year: i64,
    month: u32,
    day: u32,
}

impl Date {
    /// A calendar date, or `None` if it is not one.
    ///
    /// Validating rather than trusting is what lets the name parser reject
    /// `desktop-20261324-1.log`: a month of 13 is not a date, and treating it as
    /// one would put a nonsense name into the retention ordering.
    pub fn from_ymd(year: i64, month: u32, day: u32) -> Option<Date> {
        if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
            return None;
        }
        Some(Date { year, month, day })
    }

    pub fn year(&self) -> i64 {
        self.year
    }

    pub fn month(&self) -> u32 {
        self.month
    }

    pub fn day(&self) -> u32 {
        self.day
    }

    /// `YYYYMMDD`, zero-padded — the form the file names use.
    pub fn stamp(&self) -> String {
        format!("{:04}{:02}{:02}", self.year, self.month, self.day)
    }

    /// Days since 1970-01-01, negative before it.
    pub fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// This date, `count` days earlier.
    pub fn minus_days(&self, count: i64) -> Date {
        date_of_days(self.days() - count)
    }
}

/// The day a moment belongs to, in UTC.
pub fn date_of(time: SystemTime) -> Date {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => date_of_seconds(since.as_secs() as i64),
        // Before the epoch the elapsed duration is the distance *backwards*.
        Err(before) => date_of_seconds(-(before.duration().as_secs() as i64)),
    }
}

fn date_of_seconds(seconds: i64) -> Date {
    // `div_euclid`, not `/`: for a negative timestamp the two disagree, and the
    // one that floors is the one that keeps 1969-12-31 from becoming 1970-01-01.
    date_of_days(seconds.div_euclid(86_400))
}

/// The civil date `days` after 1970-01-01. Howard Hinnant's `civil_from_days`:
/// shift the epoch to 0000-03-01, which puts the leap day at the end of the
/// year and makes the month arithmetic exact, then undo the shift.
fn date_of_days(days: i64) -> Date {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153; // [0, 11], March is 0
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32;
    Date {
        year: if month <= 2 { year + 1 } else { year },
        month,
        day,
    }
}

/// Days from 1970-01-01 to the given civil date: the inverse of [`date_of_days`].
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400; // [0, 399]
    let month_position = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * month_position + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// The file name for one day and one index.
pub fn file_name(date: Date, index: u32) -> String {
    format!("{}-{}-{}{}", FILE_STEM, date.stamp(), index, FILE_SUFFIX)
}

/// The day and index a name carries, or `None` if the name is not ours.
///
/// A name that fails to parse is one the budget will not delete: it is not
/// provably history, and deleting it would be deleting someone else's file on
/// the strength of a guess.
pub fn parse_file_name(name: &str) -> Option<(Date, u32)> {
    let middle = name.strip_prefix(FILE_STEM)?.strip_prefix('-')?;
    let middle = middle.strip_suffix(FILE_SUFFIX)?;
    let (stamp, index) = middle.split_once('-')?;
    if stamp.len() != 8 || index.is_empty() {
        return None;
    }
    let digits = stamp.parse::<i64>().ok()?;
    // A stamp of fewer than eight digits would still parse as a number, so the
    // length is checked above; the index is checked for a leading sign here,
    // which `parse` would otherwise accept as `+1`.
    if !index.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let index = index.parse::<u32>().ok()?;
    if index < 1 {
        return None;
    }
    let date = Date::from_ymd(digits / 10_000, (digits / 100 % 100) as u32, (digits % 100) as u32)?;
    Some((date, index))
}

/// What one file may occupy, and how long the directory may keep them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// One file's cap. Also the cap for a single record: a record larger than
    /// an empty file could never be written at all, and a smaller number would
    /// invent a bound the ruling does not have.
    pub max_file_bytes: u64,
    pub retention_days: i64,
    /// The directory's cap, counting the file being written.
    pub total_bytes: u64,
}

impl Limits {
    /// The shipped numbers. `[logging]` (T-14) may lower or raise them; what it
    /// may not do is ship something else without the baselines changing too.
    pub const SHIPPED: Limits = Limits {
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        retention_days: DEFAULT_RETENTION_DAYS,
        total_bytes: DEFAULT_TOTAL_BYTES,
    };
}

/// A record as it will be written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fitted {
    /// The line, without its newline.
    pub line: String,
    /// The byte length the record *had*, when it did not fit. `None` means the
    /// line went in whole — the only way a reader can tell a truncated record
    /// from a short one.
    pub original_size: Option<u64>,
}

/// The record as it will be written: cut to one file's worth, and marked.
pub fn fit(line: &str, max_file_bytes: u64) -> Fitted {
    // At least one byte, so a caller that skipped the configuration layer's
    // validation cannot hand the arithmetic a zero.
    let limit = max_file_bytes.max(1) - 1; // the newline that ends the line
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

/// Where a record goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rotation {
    /// Into the file that is open.
    Keep,
    /// Into a new file: either the day changed or this record does not fit.
    Roll,
}

/// Whether a record belongs in the open file.
///
/// The date check comes first: a file named for yesterday must not keep
/// collecting today's records, whatever room it has left.
pub fn rotation(
    open_date: Date,
    today: Date,
    current_bytes: u64,
    next_bytes: u64,
    max_file_bytes: u64,
) -> Rotation {
    if open_date != today {
        return Rotation::Roll;
    }
    // `next_bytes` has already been cut to the cap by [`fit`], so an empty file
    // always takes the record. Without this the caller would roll forever: every
    // new file would be empty, every record would "not fit", and the loop would
    // manufacture files. This is also the ruling's 超长单条不得新开文件.
    if current_bytes == 0 || current_bytes + next_bytes <= max_file_bytes {
        return Rotation::Keep;
    }
    Rotation::Roll
}

/// One file found in the log directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub date: Date,
    pub index: u32,
    pub bytes: u64,
}

/// Which files to delete, oldest first.
///
/// Pure: a listing in, names out. The caller does the unlinking, so a test can
/// ask the question without a filesystem and a reader can see the rule instead
/// of inferring it from what disappeared.
///
/// - `keep` is the file a writer holds open. It is never a candidate (the
///   ruling 六), though its size still counts towards the total — it is on disk.
/// - `rolled_by_us` is what *this* writer closed during this run. It exists for
///   the total rule only, and only for files dated today: an index file dated
///   today that this writer did not roll may be another instance's live file,
///   and deleting it would destroy a running app's records. Yesterday's files
///   are history whoever wrote them.
pub fn expired(
    candidates: &[Candidate],
    today: Date,
    limits: Limits,
    keep: Option<&str>,
    rolled_by_us: &BTreeSet<String>,
) -> Vec<String> {
    let mut total: u64 = 0;
    let mut deletable: Vec<&Candidate> = Vec::new();
    for candidate in candidates {
        // Counted before the `keep` check: the open file is not deletable, but
        // it does occupy the directory the budget is about.
        total = total.saturating_add(candidate.bytes);
        if Some(candidate.name.as_str()) != keep {
            deletable.push(candidate);
        }
    }
    // Index as the tie-break, so two rolls on one day retire in the order they
    // were written rather than in whatever order the directory listed them.
    deletable.sort_by_key(|candidate| (candidate.date, candidate.index));

    let cutoff = today.minus_days(limits.retention_days);
    let mut doomed: Vec<String> = Vec::new();
    let mut remaining: Vec<&Candidate> = Vec::new();
    for candidate in deletable {
        // `<=`: a 14-day window is today plus the 13 days before it. A file
        // dated exactly `retention_days` ago is the 15th day and goes.
        if candidate.date <= cutoff {
            doomed.push(candidate.name.clone());
        } else {
            remaining.push(candidate);
        }
    }

    for candidate in remaining {
        if total <= limits.total_bytes {
            break;
        }
        if candidate.date >= today && !rolled_by_us.contains(&candidate.name) {
            // Someone else's file, possibly someone else's *open* file. Sparing
            // it can leave the directory over budget until tomorrow; deleting it
            // would silently cut off a running app's log. The first is a bound
            // that is late, the second is a record that is gone.
            continue;
        }
        doomed.push(candidate.name.clone());
        total = total.saturating_sub(candidate.bytes);
    }
    doomed
}

/// Why the log file could not be written. Holds no credentials: a log path is a
/// path.
#[derive(Debug)]
pub struct RollingError {
    pub path: PathBuf,
    pub reason: std::io::Error,
}

impl std::fmt::Display for RollingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot write {}: {}", self.path.display(), self.reason)
    }
}

impl std::error::Error for RollingError {}

/// The file a writer holds open.
struct Open {
    file: std::fs::File,
    path: PathBuf,
    name: String,
    date: Date,
}

/// The directory's writer: claims a file, writes records, rolls, prunes.
///
/// Every failure is a returned `Err` (or a reported-and-continued prune), never
/// a panic: the caller's response to all of them is the same — say so on stderr
/// and carry on, because the user's ruling 五 is that logging may never be the
/// reason a launch fails.
pub struct Writer {
    directory: PathBuf,
    limits: Limits,
    clock: Box<dyn Clock + Send + Sync>,
    open: Option<Open>,
    rolled_by_us: BTreeSet<String>,
}

impl Writer {
    /// A writer for this directory, with the startup prune already done.
    ///
    /// No file is claimed here: a file that exists before there is anything to
    /// put in it is a file that says "Desktop ran and logged nothing", and a
    /// directory of those is worse than an empty one.
    pub fn open(
        directory: &Path,
        limits: Limits,
        // `Send + Sync` come with `Clock`, so only the lifetime has to be said.
        clock: impl Clock + 'static,
    ) -> Result<Writer, RollingError> {
        let mut writer = Writer {
            directory: directory.to_path_buf(),
            limits,
            clock: Box::new(clock),
            open: None,
            rolled_by_us: BTreeSet::new(),
        };
        // The second of the two points the budget is enforced at; the other is
        // a roll. Between them the open file may grow to its own cap, which is
        // why the bound is `total_bytes` plus what the open files may still
        // hold, and not `total_bytes` to the byte.
        writer.prune();
        Ok(writer)
    }

    /// The file this writer is holding open, if any.
    pub fn open_path(&self) -> Option<&Path> {
        self.open.as_ref().map(|open| open.path.as_path())
    }

    /// Write one record. The line must not contain a newline.
    pub fn write_line(&mut self, line: &str) -> Result<(), RollingError> {
        let today = date_of(self.clock.now());
        let fitted = fit(line, self.limits.max_file_bytes);
        let next_bytes = fitted.line.len() as u64 + 1; // the newline
        let current_bytes = self.open_bytes();

        let roll = match self.open.as_ref() {
            None => true,
            Some(open) => {
                rotation(
                    open.date,
                    today,
                    current_bytes,
                    next_bytes,
                    self.limits.max_file_bytes,
                ) == Rotation::Roll
            }
        };
        if roll {
            self.roll(today)?;
        }

        let open = match self.open.as_mut() {
            Some(open) => open,
            // Unreachable: `roll` either set `self.open` or returned `Err`.
            None => return Ok(()),
        };
        open.file
            .write_all(fitted.line.as_bytes())
            .and_then(|()| open.file.write_all(b"\n"))
            // Flushed per record, not buffered: a log that only reaches the disk
            // on a clean exit is no use for the crash it was written to explain,
            // and Desktop's volume is a handful of records per launch.
            .and_then(|()| open.file.flush())
            .map_err(|reason| RollingError {
                path: open.path.clone(),
                reason,
            })
    }

    /// Delete what is out of the window, then what is over the budget.
    ///
    /// Returns what it deleted, so the decision is visible to the caller instead
    /// of being inferred from the directory afterwards.
    pub fn prune(&mut self) -> Vec<String> {
        let today = date_of(self.clock.now());
        let keep = self.open.as_ref().map(|open| open.name.clone());
        let candidates = match self.list() {
            Ok(candidates) => candidates,
            Err(error) => {
                self.report(&format!("cannot list {}: {error}", self.directory.display()));
                return Vec::new();
            }
        };
        let doomed = expired(
            &candidates,
            today,
            self.limits,
            keep.as_deref(),
            &self.rolled_by_us,
        );

        let mut deleted = Vec::new();
        for name in doomed {
            let path = self.directory.join(&name);
            match std::fs::remove_file(&path) {
                Ok(()) => deleted.push(name),
                // Not fatal, but not quiet either: if the deletion never
                // succeeds the directory has no bound left, and a reader
                // wondering why it is full needs this line.
                Err(error) => self.report(&format!(
                    "cannot delete {} ({error})",
                    path.display()
                )),
            }
        }
        deleted
    }

    /// Close the open file and start a new one for `today`.
    fn roll(&mut self, today: Date) -> Result<(), RollingError> {
        if let Some(open) = self.open.take() {
            // Closing before the claim, and remembering the name: the file just
            // became history, so the budget may hold it to the total.
            drop(open.file);
            self.rolled_by_us.insert(open.name);
        }
        let claimed = self.claim(today)?;
        self.open = Some(claimed);
        // After the claim, not before: the new file has to be on disk when the
        // budget counts what the directory holds.
        self.prune();
        Ok(())
    }

    /// Claim a name for this day, or fail.
    ///
    /// `create_new` is what makes the claim atomic. Two instances starting at
    /// the same second both reach for `-1`; exactly one gets it, and the other
    /// is sent to `-2` by `AlreadyExists` rather than appending to a file that
    /// is not its own.
    fn claim(&mut self, date: Date) -> Result<Open, RollingError> {
        if let Err(reason) = std::fs::create_dir_all(&self.directory) {
            return Err(RollingError {
                path: self.directory.clone(),
                reason,
            });
        }
        let mut index = 1u32;
        loop {
            let name = file_name(date, index);
            let path = self.directory.join(&name);
            match std::fs::OpenOptions::new()
                .create_new(true)
                .append(true)
                .open(&path)
            {
                Ok(file) => {
                    return Ok(Open {
                        file,
                        path,
                        name,
                        date,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if index >= MAX_INDEX_PER_DAY {
                        return Err(RollingError { path, reason: error });
                    }
                    index += 1;
                }
                Err(reason) => return Err(RollingError { path, reason }),
            }
        }
    }

    /// What the open file currently holds.
    fn open_bytes(&self) -> u64 {
        match self.open.as_ref() {
            Some(open) => open.file.metadata().map(|meta| meta.len()).unwrap_or(0),
            None => 0,
        }
    }

    /// Every file in the directory that is ours, with its date and size.
    fn list(&self) -> std::io::Result<Vec<Candidate>> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((date, index)) = parse_file_name(&name) else {
                // Someone else's file, or a name from a version that spelled it
                // differently. Not provably history, so never a candidate.
                continue;
            };
            // A file that vanished between the listing and the stat counts as
            // zero rather than aborting the prune: another writer may rename it
            // away, and an error here would lose the record whose write
            // triggered this.
            let bytes = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            found.push(Candidate {
                name,
                date,
                index,
                bytes,
            });
        }
        Ok(found)
    }

    /// Tell the operator something without going back through the logger.
    ///
    /// This runs on the emit path: the writer being written is the sink these
    /// records would go to, so the only destination that is both visible and
    /// safe is stderr — the same choice `logging.Handler.handleError` makes on
    /// the Agent's side, for the same reason.
    fn report(&self, message: &str) {
        eprintln!("[wt-media-desktop] logging: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// A clock a test can move, and keep a handle on after handing it to a
    /// writer. `AtomicI64` because `Clock::now` takes `&self`; `Arc` because the
    /// writer takes ownership of the (boxed) clock.
    #[derive(Clone)]
    struct Frozen(Arc<AtomicI64>);

    impl Frozen {
        fn at(seconds: i64) -> Frozen {
            Frozen(Arc::new(AtomicI64::new(seconds)))
        }

        /// Midnight UTC on this date.
        fn on(year: i64, month: u32, day: u32) -> Frozen {
            Frozen::at(
                Date::from_ymd(year, month, day)
                    .expect("a real date")
                    .days()
                    * 86_400,
            )
        }

        fn advance_days(&self, days: i64) {
            self.0.fetch_add(days * 86_400, Ordering::SeqCst);
        }
    }

    impl Clock for Frozen {
        fn now(&self) -> SystemTime {
            let seconds = self.0.load(Ordering::SeqCst);
            if seconds >= 0 {
                UNIX_EPOCH + Duration::from_secs(seconds as u64)
            } else {
                UNIX_EPOCH - Duration::from_secs((-seconds) as u64)
            }
        }
    }

    fn date(year: i64, month: u32, day: u32) -> Date {
        Date::from_ymd(year, month, day).expect("a real date")
    }

    /// A scratch directory of this test's own, named after the process so two
    /// concurrent runs cannot collide. Removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-rolling-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        path
    }

    fn shipped() -> Limits {
        // Small enough to roll without writing 20 MB: the *rule* is what is
        // under test, and the shipped numbers are asserted separately.
        Limits {
            max_file_bytes: 200,
            retention_days: 14,
            total_bytes: 100 * 1024 * 1024,
        }
    }

    // ---- the date, which every other rule is expressed in ----

    #[test]
    fn the_epoch_is_1970_01_01() {
        assert_eq!(date_of(UNIX_EPOCH), date(1970, 1, 1));
        assert_eq!(date_of(UNIX_EPOCH + Duration::from_secs(86_399)), date(1970, 1, 1));
        assert_eq!(date_of(UNIX_EPOCH + Duration::from_secs(86_400)), date(1970, 1, 2));
    }

    /// Before the epoch the arithmetic has to floor, not truncate towards zero:
    /// a second before 1970-01-01 is 1969-12-31, and `-1 / 86_400` is 0.
    #[test]
    fn dates_before_the_epoch_are_converted_rather_than_wrapped() {
        assert_eq!(date_of(UNIX_EPOCH - Duration::from_secs(1)), date(1969, 12, 31));
        assert_eq!(date_of(UNIX_EPOCH - Duration::from_secs(86_400)), date(1969, 12, 31));
        assert_eq!(date_of(UNIX_EPOCH - Duration::from_secs(86_401)), date(1969, 12, 30));
    }

    /// The century rule, which is the half of "leap year" that is easy to get
    /// wrong: 2000 is a leap year and 1900 is not.
    #[test]
    fn leap_years_follow_the_century_rule() {
        assert_eq!(date_of_seconds(days_of(2024, 2, 29)), date(2024, 2, 29));
        assert_eq!(date_of_seconds(days_of(2024, 3, 1)), date(2024, 3, 1));
        assert_eq!(date_of_seconds(days_of(2000, 2, 29)), date(2000, 2, 29));
        assert_eq!(date_of_seconds(days_of(1900, 2, 28)), date(1900, 2, 28));
        assert_eq!(Date::from_ymd(1900, 2, 29), None);
        assert_eq!(Date::from_ymd(2024, 2, 30), None);
    }

    fn days_of(year: i64, month: u32, day: u32) -> i64 {
        date(year, month, day).days() * 86_400
    }

    /// Round trip over a stretch that includes a leap day, so the two halves of
    /// the conversion are pinned against each other and not just against a
    /// hand-written table.
    #[test]
    fn every_day_of_a_leap_year_round_trips() {
        let start = date(2024, 1, 1).days();
        for offset in 0..366 {
            let days = start + offset;
            let converted = date_of_days(days);
            assert_eq!(converted.days(), days, "{converted:?} did not round trip");
        }
        assert_eq!(date_of_days(start + 365).stamp(), "20241231");
    }

    #[test]
    fn the_stamp_is_zero_padded() {
        assert_eq!(date(2026, 9, 24).stamp(), "20260924");
        assert_eq!(date(2026, 1, 5).stamp(), "20260105");
    }

    #[test]
    fn a_date_moves_back_by_a_whole_number_of_days() {
        assert_eq!(date(2026, 3, 1).minus_days(1), date(2026, 2, 28));
        assert_eq!(date(2024, 3, 1).minus_days(1), date(2024, 2, 29));
        assert_eq!(date(2026, 1, 1).minus_days(1), date(2025, 12, 31));
        assert_eq!(date(2026, 9, 24).minus_days(14), date(2026, 9, 10));
    }

    // ---- names ----

    #[test]
    fn a_file_name_states_its_date_and_index() {
        assert_eq!(file_name(date(2026, 9, 24), 1), "desktop-20260924-1.log");
        assert_eq!(file_name(date(2026, 1, 5), 12), "desktop-20260105-12.log");
    }

    #[test]
    fn a_name_we_did_not_write_is_not_ours() {
        // Accepted.
        assert_eq!(
            parse_file_name("desktop-20260924-1.log"),
            Some((date(2026, 9, 24), 1))
        );
        assert_eq!(
            parse_file_name("desktop-20260105-120.log"),
            Some((date(2026, 1, 5), 120))
        );

        // Rejected: another component's file, a name with no index, index zero,
        // a stamp that is not a date, a suffix that is not ours, and a name
        // whose parts are the right shape but not digits.
        for name in [
            "agent-20260924-1.log",
            "desktop.log",
            "desktop-20260924.log",
            "desktop-20260924-0.log",
            "desktop-20261324-1.log",
            "desktop-20260932-1.log",
            "desktop-2026924-1.log",
            "desktop-20260924-1.log.txt",
            "desktop-20260924-1.txt",
            "desktop--1.log",
            "desktop-20260924-.log",
            "desktop-20260924-1.5.log",
            "desktop-+20260924-1.log",
            "desktop-20260924-+1.log",
        ] {
            assert_eq!(parse_file_name(name), None, "{name} must not parse");
        }
    }

    // ---- one record, cut to fit ----

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
            "the marked record must still fit the file: {with_newline} bytes"
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
    /// record with no size information, and the configuration layer rejects
    /// caps this small.
    #[test]
    fn a_cap_too_small_for_the_marker_still_marks_the_record() {
        let fitted = fit("a long record", 4);

        assert_eq!(fitted.original_size, Some(13));
        assert_eq!(fitted.line, " truncate=true original_size=13");
    }

    // ---- where a record goes ----

    #[test]
    fn a_record_that_fits_stays_in_the_open_file() {
        assert_eq!(
            rotation(date(2026, 9, 24), date(2026, 9, 24), 100, 50, 200),
            Rotation::Keep
        );
    }

    #[test]
    fn a_record_that_would_pass_the_cap_rolls() {
        assert_eq!(
            rotation(date(2026, 9, 24), date(2026, 9, 24), 180, 50, 200),
            Rotation::Roll
        );
    }

    /// Exactly filling a file is not overflowing it. An off-by-one here would
    /// roll a record early, which is invisible until someone counts files.
    #[test]
    fn a_record_that_exactly_fills_the_file_is_kept() {
        assert_eq!(
            rotation(date(2026, 9, 24), date(2026, 9, 24), 150, 50, 200),
            Rotation::Keep
        );
    }

    #[test]
    fn the_day_changing_rolls_even_a_file_with_room_left() {
        assert_eq!(
            rotation(date(2026, 9, 24), date(2026, 9, 25), 10, 50, 200),
            Rotation::Roll
        );
    }

    /// An oversized record must not start a file of its own: every new file is
    /// empty, so "it does not fit" would be true forever.
    #[test]
    fn a_record_too_large_for_an_empty_file_is_still_written() {
        assert_eq!(
            rotation(date(2026, 9, 24), date(2026, 9, 24), 0, 5_000, 200),
            Rotation::Keep
        );
    }

    // ---- what the budget deletes ----

    fn candidate(name: &str, bytes: u64) -> Candidate {
        let (date, index) = parse_file_name(name).expect("a name we wrote");
        Candidate {
            name: name.to_owned(),
            date,
            index,
            bytes,
        }
    }

    fn no_set() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn budget(retention_days: i64, total_bytes: u64) -> Limits {
        Limits {
            max_file_bytes: 20 * 1024 * 1024,
            retention_days,
            total_bytes,
        }
    }

    #[test]
    fn the_window_is_today_plus_the_days_before_it() {
        let today = date(2026, 9, 24);
        let candidates = vec![
            candidate("desktop-20260910-1.log", 10), // 14 days ago: the 15th day
            candidate("desktop-20260911-1.log", 10), // 13 days ago: inside
        ];

        let doomed = expired(&candidates, today, budget(14, u64::MAX), None, &no_set());

        assert_eq!(
            doomed,
            vec!["desktop-20260910-1.log"],
            "the day exactly at the window's edge goes, the one inside stays"
        );
    }

    #[test]
    fn files_are_deleted_oldest_first_until_the_total_fits() {
        let today = date(2026, 9, 24);
        let candidates = vec![
            candidate("desktop-20260923-2.log", 100),
            candidate("desktop-20260923-1.log", 100),
            candidate("desktop-20260922-1.log", 100),
        ];

        let doomed = expired(&candidates, today, budget(14, 250), None, &no_set());

        assert_eq!(
            doomed,
            vec!["desktop-20260922-1.log"],
            "the day before yesterday goes; the two that fit stay"
        );
    }

    /// The total rule stops when it has deleted enough. A version that emptied
    /// the directory whenever it was over budget would pass a one-file test and
    /// lose every record.
    #[test]
    fn the_total_rule_deletes_no_more_than_it_has_to() {
        let today = date(2026, 9, 24);
        let candidates = vec![
            candidate("desktop-20260922-1.log", 100),
            candidate("desktop-20260923-1.log", 100),
            candidate("desktop-20260924-1.log", 100),
        ];

        let doomed = expired(&candidates, today, budget(14, 250), None, &no_set());

        assert_eq!(doomed, vec!["desktop-20260922-1.log"]);
        assert_eq!(doomed.len(), 1, "250 fits two files; only one had to go");
    }

    /// The ruling 六, as a two-way assertion: the open file is not deleted even
    /// when it is the oldest thing in the directory *and* out of the window,
    /// and the same file, not open, is.
    #[test]
    fn the_open_file_is_never_a_candidate() {
        let today = date(2026, 9, 24);
        let only = vec![candidate("desktop-20260101-1.log", 10_000)];
        let live = Some("desktop-20260101-1.log");

        assert_eq!(
            expired(&only, today, budget(14, 100), live, &no_set()),
            Vec::<String>::new(),
            "an open file is not history, however old it is"
        );
        assert_eq!(
            expired(&only, today, budget(14, 100), None, &no_set()),
            vec!["desktop-20260101-1.log"],
            "the same file, closed, is out of the window and over budget"
        );

        // And the filter is specific rather than a blanket refusal to prune:
        // with a second file present, that one still goes.
        let both = vec![
            candidate("desktop-20260101-1.log", 10_000),
            candidate("desktop-20260923-1.log", 10),
        ];
        assert_eq!(
            expired(&both, today, budget(14, 100), live, &no_set()),
            vec!["desktop-20260923-1.log"]
        );
    }

    /// A file dated today that this writer did not roll may be another
    /// instance's open file. Two-way: a file this writer *did* roll goes.
    #[test]
    fn todays_file_is_spared_unless_this_writer_rolled_it() {
        let today = date(2026, 9, 24);
        let candidates = vec![candidate("desktop-20260924-1.log", 10_000)];

        let spared = expired(&candidates, today, budget(14, 100), None, &no_set());
        assert_eq!(spared, Vec::<String>::new());

        let mut rolled = BTreeSet::new();
        rolled.insert("desktop-20260924-1.log".to_owned());
        let doomed = expired(&candidates, today, budget(14, 100), None, &rolled);
        assert_eq!(doomed, vec!["desktop-20260924-1.log"]);
    }

    /// Yesterday's file is history whoever wrote it, so the exemption above is
    /// about *today's* files only and not about sparing everything.
    #[test]
    fn yesterdays_file_is_history_whoever_wrote_it() {
        let today = date(2026, 9, 24);
        let candidates = vec![candidate("desktop-20260923-1.log", 10_000)];

        let doomed = expired(&candidates, today, budget(14, 100), None, &no_set());

        assert_eq!(doomed, vec!["desktop-20260923-1.log"]);
    }

    // ---- the writer, on a real directory ----

    #[test]
    fn the_shipped_limits_are_the_ones_the_ruling_names() {
        assert_eq!(Limits::SHIPPED.max_file_bytes, 20 * 1024 * 1024);
        assert_eq!(Limits::SHIPPED.retention_days, 14);
        assert_eq!(Limits::SHIPPED.total_bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn nothing_is_created_before_the_first_record() {
        let root = scratch("lazy");
        std::fs::create_dir_all(&root).expect("scratch");
        let writer = Writer::open(&root, shipped(), Frozen::on(2026, 9, 24)).expect("open");

        let created = std::fs::read_dir(&root)
            .expect("the directory exists")
            .count();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(created, 0, "an empty log file is not a log");
        assert_eq!(writer.open_path(), None);
    }

    #[test]
    fn the_first_record_lands_in_todays_first_file() {
        let root = scratch("first");
        let mut writer = Writer::open(&root, shipped(), Frozen::on(2026, 9, 24)).expect("open");

        writer.write_line("desktop.startup ready").expect("write");

        let contents = std::fs::read_to_string(root.join("desktop-20260924-1.log")).expect("read");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(contents, "desktop.startup ready\n");
    }

    /// One line per record, which is what the plain-text format promises the
    /// reader (and what a grep-based reading of the file depends on).
    #[test]
    fn records_are_one_line_each() {
        let root = scratch("lines");
        let mut writer = Writer::open(&root, shipped(), Frozen::on(2026, 9, 24)).expect("open");

        writer.write_line("first").expect("write");
        writer.write_line("second").expect("write");

        let contents = std::fs::read_to_string(root.join("desktop-20260924-1.log")).expect("read");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(contents, "first\nsecond\n");
    }

    /// The multi-instance rule: the second writer must take the next index
    /// rather than append to a file that is not its own.
    #[test]
    fn a_second_writer_takes_the_next_index_rather_than_the_same_file() {
        let root = scratch("two-writers");
        let mut first = Writer::open(&root, shipped(), Frozen::on(2026, 9, 24)).expect("open");
        let mut second = Writer::open(&root, shipped(), Frozen::on(2026, 9, 24)).expect("open");

        first.write_line("from the first").expect("write");
        second.write_line("from the second").expect("write");

        let one = std::fs::read_to_string(root.join("desktop-20260924-1.log")).expect("read");
        let two = std::fs::read_to_string(root.join("desktop-20260924-2.log")).expect("read");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(one, "from the first\n");
        assert_eq!(two, "from the second\n");
    }

    /// One writer, one launch, midnight in between: the date changing is enough
    /// to move the record into a file named for the new day, and yesterday's
    /// records are still there afterwards.
    #[test]
    fn the_day_changing_starts_a_new_file_and_keeps_the_old_one() {
        let root = scratch("day-roll");
        let clock = Frozen::on(2026, 9, 24);
        let mut writer =
            Writer::open(&root, shipped(), clock.clone()).expect("open");
        writer.write_line("yesterday").expect("write");

        clock.advance_days(1);
        writer.write_line("today").expect("write");

        let first = std::fs::read_to_string(root.join("desktop-20260924-1.log"));
        let second = std::fs::read_to_string(root.join("desktop-20260925-1.log"));
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(first.expect("read"), "yesterday\n");
        assert_eq!(second.expect("read"), "today\n");
    }

    #[test]
    fn a_full_file_rolls_and_the_next_record_starts_a_new_one() {
        let root = scratch("size-roll");
        let limits = Limits {
            max_file_bytes: 20,
            ..shipped()
        };
        let mut writer = Writer::open(&root, limits, Frozen::on(2026, 9, 24)).expect("open");

        writer.write_line("aaaaaaaaa").expect("write"); // 10 bytes
        writer.write_line("bbbbbbbbb").expect("write"); // 10 bytes: 20 with newlines
        writer.write_line("ccccccccc").expect("write");

        let first = std::fs::read_to_string(root.join("desktop-20260924-1.log")).expect("read");
        let second = std::fs::read_to_string(root.join("desktop-20260924-2.log")).expect("read");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(first, "aaaaaaaaa\nbbbbbbbbb\n");
        assert_eq!(second, "ccccccccc\n");
    }

    /// The record that triggered the roll is in the *new* file, not lost and not
    /// in the old one.
    #[test]
    fn an_oversized_record_is_marked_in_the_file_it_lands_in() {
        let root = scratch("truncate");
        let limits = Limits {
            max_file_bytes: 40,
            ..shipped()
        };
        let mut writer = Writer::open(&root, limits, Frozen::on(2026, 9, 24)).expect("open");

        writer.write_line(&"z".repeat(500)).expect("write");

        let written = std::fs::read_to_string(root.join("desktop-20260924-1.log")).expect("read");
        std::fs::remove_dir_all(&root).ok();

        assert!(written.ends_with("\n"), "one line, newline-terminated");
        assert!(written.contains("truncate=true original_size=500"));
        assert!(written.len() <= 40, "the file must stay within its cap: {}", written.len());
    }

    /// The other half of the budget: a file this writer closed today is history
    /// it may reclaim, and the file it holds open is not. Without the roll being
    /// recorded, today's files would be spared forever and the directory would
    /// have no bound at all until the day turned — which is exactly the case a
    /// runaway log produces.
    #[test]
    fn a_file_this_writer_rolled_today_is_reclaimable_under_pressure() {
        let root = scratch("reclaim");
        let limits = Limits {
            max_file_bytes: 20,
            total_bytes: 30,
            ..shipped()
        };
        let mut writer = Writer::open(&root, limits, Frozen::on(2026, 9, 24)).expect("open");

        for index in 1..=5 {
            writer.write_line(&format!("record {index}")).expect("write");
        }

        let oldest = root.join("desktop-20260924-1.log").exists();
        let middle = root.join("desktop-20260924-2.log").exists();
        let open = writer.open_path().expect("a file is open").to_path_buf();
        let newest = std::fs::read_to_string(&open).expect("read");
        std::fs::remove_dir_all(&root).ok();

        assert!(
            !oldest,
            "the oldest file this writer rolled is what the budget takes"
        );
        assert!(middle, "and it stops as soon as the total fits");
        assert!(
            newest.contains("record 5"),
            "the open file still holds the newest record: {newest:?}"
        );
    }

    /// The startup prune, end to end: an expired file is gone before anything is
    /// written, and a file inside the window is still there.
    #[test]
    fn opening_the_writer_prunes_what_is_expired() {
        let root = scratch("prune-startup");
        std::fs::create_dir_all(&root).expect("scratch");
        std::fs::write(root.join("desktop-19990101-1.log"), b"ancient").expect("plant");
        std::fs::write(root.join("desktop-20260924-1.log"), b"recent").expect("plant");

        let _writer = Writer::open(&root, shipped(), Frozen::on(2026, 9, 24)).expect("open");

        let ancient = root.join("desktop-19990101-1.log").exists();
        let recent = root.join("desktop-20260924-1.log").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(!ancient, "a file from 1999 is out of every window");
        assert!(recent, "a file from today is not");
    }

    /// The ruling's other half, end to end: the file being written survives a
    /// prune that deletes everything else around it.
    #[test]
    fn a_record_written_after_a_prune_is_still_there() {
        let root = scratch("prune-keeps-open");
        let limits = Limits {
            max_file_bytes: 20,
            total_bytes: 60,
            ..shipped()
        };
        let mut writer = Writer::open(&root, limits, Frozen::on(2026, 9, 24)).expect("open");

        for index in 0..6 {
            writer.write_line(&format!("record {index}")).expect("write");
        }

        let open = writer.open_path().expect("a file is open").to_path_buf();
        let contents = std::fs::read_to_string(&open).expect("the open file must exist");
        std::fs::remove_dir_all(&root).ok();

        assert!(
            contents.contains("record 5"),
            "the newest record is in the file the writer holds: {contents:?}"
        );
    }

    #[test]
    fn a_directory_that_cannot_be_created_is_an_error_rather_than_a_panic() {
        let root = scratch("occupied");
        std::fs::create_dir_all(&root).expect("scratch");
        // A file where the directory should be: reachable for a root user and in
        // CI, unlike a mode bit (`paths`' precedent).
        let occupied = root.join("logs");
        std::fs::write(&occupied, b"not a directory").expect("occupy the path");

        // Opening is lazy — no file is claimed until there is a record — so an
        // unusable directory shows up at the first record rather than at startup.
        let mut writer = Writer::open(&occupied, shipped(), Frozen::on(2026, 9, 24))
            .expect("nothing is claimed yet, so nothing can fail yet");
        let error = writer.write_line("anything").expect_err("must not succeed");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(error.path, occupied, "the error names the directory it could not use");
        // The *kind* is asserted, not just "some error", and the value is the
        // one measured for `create_dir_all` on a path occupied by a file (T-11
        // measured the same kind for the same call, on this machine).
        assert_eq!(
            error.reason.kind(),
            std::io::ErrorKind::AlreadyExists,
            "the error names the real cause, not a downstream one"
        );
        assert_eq!(writer.open_path(), None, "and no file was opened");
    }

    #[test]
    fn a_writer_that_cannot_list_its_directory_still_writes() {
        let root = scratch("no-list");
        // The directory does not exist yet, so the startup prune cannot list it.
        // That must not be fatal: a log directory that cannot be read is not a
        // reason for the app to stop (the ruling 五).
        let mut writer = Writer::open(&root, shipped(), Frozen::on(2026, 9, 24)).expect("open");

        writer.write_line("still works").expect("write");

        let written = std::fs::read_to_string(root.join("desktop-20260924-1.log")).expect("read");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(written, "still works\n");
    }

    /// What the budget is allowed to see. A file that is not ours is not
    /// history, and deleting it would be deleting someone else's file.
    #[test]
    fn the_listing_holds_our_files_and_nothing_else() {
        let root = scratch("list");
        std::fs::create_dir_all(&root).expect("scratch");
        std::fs::write(root.join("desktop-20260924-1.log"), b"ours").expect("plant");
        std::fs::write(root.join("notes.txt"), b"someone else's").expect("plant");
        std::fs::write(root.join("agent-20260924-1.log"), b"another component's").expect("plant");
        // An old shape from a version that spelled the name differently: also
        // not provably history.
        std::fs::write(root.join("desktop.log"), b"older ours").expect("plant");
        let writer = Writer::open(&root, shipped(), Frozen::on(2026, 9, 24)).expect("open");

        let listed = writer.list().expect("list");
        std::fs::remove_dir_all(&root).ok();

        let names: Vec<&str> = listed.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["desktop-20260924-1.log"]);
        assert_eq!(listed[0].bytes, 4);
        assert_eq!(listed[0].index, 1);
        assert_eq!(listed[0].date, date(2026, 9, 24));
    }
}
