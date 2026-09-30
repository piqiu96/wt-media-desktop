//! The downloaded files this machine knows about: where they are, and what may
//! be done to them.
//!
//! ## Why this is not in `commands::downloads`
//!
//! It was. 「打开文件」's lookup and 「搬运」's lookup are the same walk over the
//! same directories, and two copies of it would be **two answers to 「这台机器上
//! 有哪些文件」** — the kind of pair that agrees until the day it does not, and
//! then one of them opens a file the other says is missing. `commands::downloads`
//! and `commands::saved_files` both call in here; neither owns it.
//!
//! ## There are several directories, and that is the point
//!
//! `settings.toml` remembers every directory the operator has chosen. A file
//! downloaded before a change is still in the older one, so every question here
//! is asked of the whole list and not only of the directory new downloads go to.
//! The caller passes that list in: this module knows nothing about settings,
//! which is what lets its tests be about files and directories and nothing else.
//!
//! ## What is never done
//!
//! Only **regular files** are opened, moved or deleted. A symlink is not a
//! download: following one is how 「打开下载的文件」 becomes 「打开链接指向的任何
//! 东西」, and deleting one — or the file it points at — is not what a button
//! saying 「删除这个下载」 promised. A name is at most one subdirectory and one
//! component ([`file_name_of`]), so no caller can reach outside the directories
//! it was given however it spells what it asks for. Nothing recurses beyond that
//! one level, and nothing overwrites:
//! 「目标位置已有同名文件」 is a `kept` entry in the report, not a rename — the `(2)`
//! rule belongs to the Agent's sink, and a second one here would be a second
//! answer to *what is this file called*.
//!
//! ## Two questions that look like one
//!
//! [`find_in_known`] answers 「这个文件在哪儿」 and **fails** when it is nowhere.
//! [`scout`] answers 「这个文件在哪儿、那儿是什么」 and succeeds even when the answer
//! is 「哪儿都不在」, because its callers have to tell a person several different
//! things about several files and one of them is that a name is not there.
//! Collapsing the two is how 「没有这个文件」 becomes the answer for a directory
//! nobody could open: see [`Unreadable`].

use rustix::io::Errno;
use std::path::{Path, PathBuf};

/// The suffix a cross-volume move writes before the file is put in place.
///
/// A suffix rather than a dot-prefixed or random name, following `settings`'s
/// `TEMP_SUFFIX` reasoning: a crash between the write and the rename leaves it
/// behind, and the next person to look in that directory should be able to tell
/// it is ours and unfinished.
pub const MOVING_SUFFIX: &str = ".moving";

/// A file name that may be looked up in a save directory, or why it may not.
///
/// The page sends a **name**, never a path, and this is where that is enforced —
/// the same move as `local_log_tail`'s 「列表即白名单」. The rule is read off the
/// path's own components rather than by scanning for separators: a name is one
/// component, or one subdirectory and one component (`20260930/游戏-素材.mp4`),
/// and anything else is refused — a second level, a root, a drive prefix, `.`,
/// `..`, or a separator this platform does not treat as one.
///
/// **Byte for byte, with no trim.** A trimmed name would be a file the download
/// list shows and these commands cannot touch — the names the page sends come
/// from the task records, not from a keyboard, so there is nothing to be lenient
/// about, and `every_name_in_a_listing_is_looked_up_as_it_is_spelled` is the case
/// that holds that.
pub fn file_name_of(name: &str) -> Result<&str, String> {
    let mut normal = 0;
    for component in Path::new(name).components() {
        match component {
            std::path::Component::Normal(_) => normal += 1,
            _ => {
                return Err(format!(
                    "{name:?} 不是一个文件名（只接受保存目录里的名字，不接受路径）"
                ))
            }
        }
    }
    if (1..=2).contains(&normal) {
        Ok(name)
    } else {
        Err(format!(
            "{name:?} 不是一个文件名（只接受保存目录里的名字，不接受路径）"
        ))
    }
}

/// An entry with the right name that is not a downloadable file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotAFile {
    /// A symlink. Named apart from the rest because it is the one kind whose
    /// name suggests it *is* the file, and the one kind that can point somewhere
    /// this app has no business going.
    Symlink,
    /// A directory, a socket, a device — everything else.
    Other,
}

impl NotAFile {
    /// The spelling the page reads — `KEPT_REASON_LABELS`'s vocabulary.
    pub const fn as_str(self) -> &'static str {
        match self {
            NotAFile::Symlink => "symlink",
            NotAFile::Other => "not_a_regular_file",
        }
    }
}

/// What one directory holds under one name.
#[derive(Debug)]
enum Entry {
    /// A regular file: the only thing these commands act on.
    File(PathBuf),
    /// Something with that name that is not one. Never opened, moved or deleted.
    Other(NotAFile),
    /// Nothing with that name.
    Nothing,
}

/// A known directory that could not be looked in, and why.
///
/// The distinction this type exists for: a directory that was read and does not
/// hold the name is **evidence about the file**, and a directory nobody could
/// look in is the absence of evidence. Only the first is a place the name is
/// known not to be, and a person told 「已删除」 re-downloads 230 MB they still
/// have.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unreadable {
    pub directory: PathBuf,
    /// What was wrong with it, as a phrase: the operating system's own words, or
    /// 「现在不是一个目录」. Kept apart from the sentence below because the page
    /// shows it in parentheses after the directory it belongs to.
    pub reason: String,
    /// Whether the directory is simply not a directory any more — the one case
    /// where the person can be told what to do about it.
    pub gone: bool,
}

impl Unreadable {
    /// The sentence for a person: the directory, why, and — when it is gone —
    /// what to do.
    ///
    /// One sentence for one fact. A caller that built its own would be a second
    /// wording of the same failure, and the two would part company the first time
    /// either was edited.
    pub fn sentence(&self) -> String {
        if self.gone {
            format!(
                "保存位置 {} 现在不是一个目录：到本机设置里重新选一个",
                self.directory.display()
            )
        } else {
            format!(
                "无法读取保存位置 {}：{}",
                self.directory.display(),
                self.reason
            )
        }
    }
}

/// One named file and where it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Located {
    pub name: String,
    pub directory: PathBuf,
    /// Its size, or `None` when it could not be measured. Never a `0` that was
    /// not measured: a size is summed into 「需要多少空间」, and a missing one
    /// silently worth nothing is how a space check passes on a disk that is full.
    pub bytes: Option<u64>,
}

/// An entry with the right name that is not a regular file, and where it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Blocking {
    pub directory: PathBuf,
    pub kind: NotAFile,
}

/// One name, walked against every directory the caller knows.
///
/// The walk is done **once** per name and its findings reused, so the plan a
/// person is shown and the action taken on it are the same reading of the same
/// directories: a plan built from one walk and a move from another could move a
/// file the dialog never listed, or refuse one it did.
#[derive(Debug)]
pub struct Scouted {
    pub name: String,
    /// Every regular file with that name, in the order the directories were
    /// walked (the target first, then newest to oldest). More than one is
    /// ordinary: the same material can be in two directories.
    pub files: Vec<Located>,
    /// Entries with that name that are not regular files.
    pub blocking: Vec<Blocking>,
    /// Directories that could not be listed while looking for this name.
    pub unread: Vec<Unreadable>,
}

impl Scouted {
    /// The regular file in `directory`, if the name is one there.
    pub fn in_dir(&self, directory: &Path) -> Option<&Located> {
        self.files.iter().find(|file| file.directory == directory)
    }

    /// The first regular file **outside** `directory` — the one a move would
    /// bring over.
    pub fn outside(&self, directory: &Path) -> Option<&Located> {
        self.files.iter().find(|file| file.directory != directory)
    }

    /// The first entry with that name in `directory` that is not a regular file.
    pub fn blocking_in(&self, directory: &Path) -> Option<&Blocking> {
        self.blocking
            .iter()
            .find(|entry| entry.directory == directory)
    }

    /// The sentence for `directory` when it could not be looked in.
    ///
    /// `plan` and `move` refuse over this rather than degrade: a plan about a
    /// directory nobody has seen is a plan about nothing, and a move writes into
    /// it. `delete` and `presences` do not ask — see their own docs.
    pub fn problem_with(&self, directory: &Path) -> Option<String> {
        self.unread
            .iter()
            .find(|unread| unread.directory == directory)
            .map(Unreadable::sentence)
    }
}

/// What `directory` holds under `wanted`.
///
/// The directory is **listed** and the name matched against what is in it, rather
/// than joined onto the directory and passed to the caller: joining would make any
/// name a caller can spell a path this process touches, and the two are only
/// distinguishable at the moment of the check. `file_name_of` refuses a
/// path-shaped name before this is reached, which is what keeps that property if
/// the lookup below is ever rewritten as a join — a `join` plus `is_file` looks
/// like the same check and is not one.
///
/// `DirEntry::file_type` does not follow symlinks, so a symlink planted in a save
/// directory answers [`NotAFile::Symlink`] rather than being read as the file it
/// points at.
fn entry_in(directory: &Path, wanted: &str) -> Result<Entry, Unreadable> {
    // `wanted` was validated by `file_name_of`, so it is one component or
    // `subdir/name`. A subdirectory names a place to look, never a thing to
    // follow: it has to be a real directory (not a file, not a link), so a name
    // cannot ride a symlink out of the save directory and in the file the link
    // points at.
    let (search_in, wanted_name) = match wanted.split_once('/') {
        Some((subdir, name)) => {
            let candidate = directory.join(subdir);
            match std::fs::symlink_metadata(&candidate) {
                Ok(meta) if meta.is_dir() => (candidate, name),
                // Nothing to look in — absent, a file, or a link. In none of
                // them is there a file with that name to find.
                _ => return Ok(Entry::Nothing),
            }
        }
        None => (directory.to_path_buf(), wanted),
    };
    let entries = match std::fs::read_dir(&search_in) {
        Ok(entries) => entries,
        Err(_) if !search_in.is_dir() => {
            return Err(Unreadable {
                directory: search_in,
                reason: "现在不是一个目录".to_string(),
                gone: true,
            })
        }
        Err(error) => {
            return Err(Unreadable {
                directory: search_in,
                reason: error.to_string(),
                gone: false,
            })
        }
    };
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_str() != Some(wanted_name) {
            continue;
        }
        return match entry.file_type() {
            Ok(kind) if kind.is_file() => Ok(Entry::File(entry.path())),
            Ok(kind) if kind.is_symlink() => Ok(Entry::Other(NotAFile::Symlink)),
            _ => Ok(Entry::Other(NotAFile::Other)),
        };
    }
    Ok(Entry::Nothing)
}

/// A file's size, or `None` when it could not be measured.
///
/// `symlink_metadata` rather than `metadata`: what was matched is the entry that
/// is in the directory, and following a link that appeared between the listing and
/// this call would measure a file this directory does not contain.
fn size_of(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path).ok().map(|meta| meta.len())
}

/// Walk `name` against every directory, in the order given.
///
/// The order is the caller's and is never rearranged here: the list is the target
/// first and then the history newest-to-oldest, and a walk that sorted it would be
/// a second opinion about which copy of a duplicated name matters.
///
/// One name at a time rather than a whole list at once, because every caller's
/// next step is per name (a row per file, an action per file) and a batch answer
/// would only be split apart again.
pub fn scout(dirs: &[PathBuf], name: &str) -> Result<Scouted, String> {
    let wanted = file_name_of(name)?;
    let mut scouted = Scouted {
        name: name.to_string(),
        files: Vec::new(),
        blocking: Vec::new(),
        unread: Vec::new(),
    };
    for directory in dirs {
        match entry_in(directory, wanted) {
            Ok(Entry::File(path)) => scouted.files.push(Located {
                name: name.to_string(),
                directory: directory.clone(),
                bytes: size_of(&path),
            }),
            Ok(Entry::Other(kind)) => scouted.blocking.push(Blocking {
                directory: directory.clone(),
                kind,
            }),
            Ok(Entry::Nothing) => {}
            Err(reason) => scouted.unread.push(reason),
        }
    }
    Ok(scouted)
}

/// Check every name before anything is touched, and drop the repeats.
///
/// Up front, because one refused name must not leave the rest half-done: the
/// caller sent a path where a name belongs, and a partial move is a machine in a
/// state nobody asked for. Repeats are dropped rather than acted on twice — a
/// second attempt at a name the first attempt moved is not a second file, and the
/// page lists a name once however many tasks mention it.
fn validated(names: &[String]) -> Result<Vec<&str>, String> {
    let mut checked: Vec<&str> = Vec::with_capacity(names.len());
    for name in names {
        let wanted = file_name_of(name)?;
        if !checked.contains(&wanted) {
            checked.push(wanted);
        }
    }
    Ok(checked)
}

/// Find `name` in every directory this machine has written downloads into.
///
/// Newest first, and the history is not a courtesy: a file downloaded while an
/// older directory was current is **still there** afterwards, and a lookup that
/// only knew the directory in use now would report it as 「可能已被移动或删除」 —
/// which is the defect this exists to remove. So the first directory that has the
/// file wins, wherever it is in the list.
///
/// A directory that could not be read is not an error by itself: the file may be
/// in the next one, and failing on it would refuse to open a file that is sitting
/// right there. It is only *reported* — in the message, when nothing was found —
/// because 「这个文件不在本机」 and 「有三个地方我没能查」 are different answers and
/// only one of them is about the file.
pub fn find_in_known(dirs: &[PathBuf], name: &str) -> Result<PathBuf, String> {
    let wanted = file_name_of(name)?;
    if dirs.is_empty() {
        return Err("还没有选择下载保存位置".to_string());
    }
    let mut searched: Vec<String> = Vec::with_capacity(dirs.len());
    let mut unreadable: Vec<String> = Vec::new();
    for directory in dirs {
        match entry_in(directory, wanted) {
            Ok(Entry::File(path)) => return Ok(path),
            Ok(_) => searched.push(directory.display().to_string()),
            Err(reason) => unreadable.push(reason.sentence()),
        }
    }
    Err(describe_not_found(wanted, &searched, &unreadable))
}

/// What to say when the name was in none of the directories.
///
/// 「查了哪些地方」 is spelled out rather than summarised. The defect this
/// replaced reported the *current* directory as though it were the only place
/// known, so a person whose file was in their previous folder was told their
/// file might have been deleted — the list is what makes the sentence checkable
/// against what they remember doing.
///
/// The two lists are separate and never merged: a directory that was read and
/// does not hold the name is evidence about the file, and a directory nobody
/// could look in is the absence of evidence. Only the first is a place the name
/// is known not to be.
fn describe_not_found(wanted: &str, searched: &[String], unreadable: &[String]) -> String {
    let mut parts = vec![format!("{wanted} 不在本机已知的保存位置里")];
    if !searched.is_empty() {
        parts.push(format!("已经查过：{}", searched.join("、")))
    }
    if !unreadable.is_empty() {
        parts.push(format!(
            "另有 {} 个位置没能查（那里的文件没看过）：{}",
            unreadable.len(),
            unreadable.join("；")
        ))
    }
    parts.push("可能已被移动或删除；如果它被搬到别处，可以在下载列表里重新下载".to_string());
    parts.join("。")
}

/// Where one name is, as 「文件在哪儿」 answers it.
#[derive(Clone, Debug)]
pub struct Presence {
    pub name: String,
    /// The directory a regular file with that name is in, when there is one.
    ///
    /// `None` also covers an entry of that name that is not a regular file: this
    /// answer is about **a downloadable file**, and a symlink is not one. That is
    /// the honest answer to the question the page is asking (「这个下载还在不在」),
    /// and the plan's finer vocabulary is what anything that has to *act* uses.
    pub directory: Option<PathBuf>,
    /// Whether that directory is the one new downloads go to. `false` when there
    /// is no file, and `false` when nothing is chosen — there is then no directory
    /// it could be in.
    pub current: bool,
    pub bytes: Option<u64>,
}

/// What the directories say about each name.
///
/// No target refusal: this is a **read**, and a read that cannot see one directory
/// still has true things to say about the others. The target is passed only so
/// `current` can be answered, and it is an `Option` because a machine that has
/// never chosen one has no target rather than a default.
pub fn presences(
    dirs: &[PathBuf],
    target: Option<&Path>,
    names: &[String],
) -> Result<Vec<Presence>, String> {
    let mut found = Vec::new();
    for name in validated(names)? {
        let scouted = scout(dirs, name)?;
        let file = scouted.files.first();
        found.push(Presence {
            name: name.to_string(),
            directory: file.map(|file| file.directory.clone()),
            current: match (file, target) {
                (Some(file), Some(target)) => file.directory == target,
                _ => false,
            },
            bytes: file.and_then(|file| file.bytes),
        });
    }
    Ok(found)
}

/// What moving the saved files would do, before anything is touched.
#[derive(Debug)]
pub struct Plan {
    /// Where the files would go.
    pub to: PathBuf,
    /// Free space on the target's volume.
    pub free_bytes: u64,
    /// The sum of the sizes a move would carry. Over-counts nothing; under-counts
    /// only what could not be measured, whose own entry carries `bytes: None`,
    /// which is how the page knows to say 「至少」.
    pub needed_bytes: u64,
    /// Regular files in an older directory, with a free name in the target.
    pub movable: Vec<Located>,
    /// Already in the target: nothing to do.
    pub already_there: Vec<Located>,
    /// Names no known directory holds **as a downloadable file**. An entry of that
    /// name that is not a regular file lands here too: there is no file by that
    /// name anywhere, which is the fact this list carries — and the page has no
    /// other bucket for it, so the alternative would be a row offering a move the
    /// report would refuse.
    pub missing: Vec<String>,
    /// Directories that could not be listed. Not a failure of the plan — the
    /// person is told which files were not looked for.
    pub unreadable: Vec<Unreadable>,
}

/// Plan a move of `names` into `target`.
///
/// `free_bytes` is measured by the caller rather than here: it is `statvfs` on a
/// volume, which is a question about the machine and not about these files, and a
/// plan whose arithmetic is decided by whatever the test machine's disk happens to
/// be doing is a plan no test can hold still.
pub fn plan(
    dirs: &[PathBuf],
    target: &Path,
    names: &[String],
    free_bytes: u64,
) -> Result<Plan, String> {
    let mut planned = Plan {
        to: target.to_path_buf(),
        free_bytes,
        needed_bytes: 0,
        movable: Vec::new(),
        already_there: Vec::new(),
        missing: Vec::new(),
        unreadable: Vec::new(),
    };
    for name in validated(names)? {
        let scouted = scout(dirs, name)?;
        // Refused, not degraded: a move writes into the target, and this is the
        // only case the operator has to do something about before either command
        // can mean anything.
        if let Some(problem) = scouted.problem_with(target) {
            return Err(problem);
        }
        // Once per directory however many names were walked through it: the page
        // renders the list, and the same sentence repeated per file would read as
        // several broken directories.
        for entry in &scouted.unread {
            if !planned
                .unreadable
                .iter()
                .any(|seen| seen.directory == entry.directory)
            {
                planned.unreadable.push(entry.clone());
            }
        }
        // Before the file is looked for: an entry with that name is already in the
        // target and may not be written over, so a file in an older directory could
        // not be moved here whatever else is true — offering it would be offering an
        // action the report would refuse.
        if scouted.blocking_in(target).is_some() {
            planned.missing.push(name.to_string());
            continue;
        }
        if let Some(file) = scouted.in_dir(target) {
            planned.already_there.push(file.clone());
            continue;
        }
        if let Some(file) = scouted.outside(target) {
            planned.needed_bytes += file.bytes.unwrap_or(0);
            planned.movable.push(file.clone());
            continue;
        }
        planned.missing.push(name.to_string());
    }
    Ok(planned)
}

/// Why an entry was deliberately left where it is.
///
/// A closed vocabulary, because these strings cross the wire: the page renders one
/// next to a file the person expected to move, and it is
/// `local-settings-view.js`'s `KEPT_REASON_LABELS` that reads them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeptReason {
    /// Already in the target directory.
    SameDirectory,
    /// The target holds that name and it is not a regular file.
    TargetExists,
    /// Not a regular file at all. Carries which kind, so the two spellings the
    /// page knows (`symlink`, `not_a_regular_file`) come from one place.
    NotAFile(NotAFile),
}

impl KeptReason {
    /// The spelling the page reads.
    pub const fn as_str(self) -> &'static str {
        match self {
            KeptReason::SameDirectory => "same_directory",
            KeptReason::TargetExists => "target_exists",
            KeptReason::NotAFile(kind) => kind.as_str(),
        }
    }
}

/// One entry the action deliberately left alone, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Kept {
    pub name: String,
    pub reason: KeptReason,
}

/// One name the action could not carry out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    /// The **name**, never a path: the whole point of this command family is that
    /// no path crosses it, and a failure names the file the person picked. What
    /// went wrong with it is in `reason`.
    pub name: String,
    /// A sentence rather than the `io::Error`, unlike `cleanup::Failure`: some of
    /// these are not one syscall's outcome but a fact about the directories
    /// (「有一个位置没能查」), and flattening only some of them to `to_string()` on
    /// the way out would lose the difference.
    pub reason: String,
}

/// What one move or one delete did.
#[derive(Debug)]
pub struct Report {
    /// The directory the action was aimed at. Shown for a move (that is where the
    /// files went); the delete report carries it too rather than having a second
    /// shape, and the page names it only for a move — a deletion has no
    /// destination, and naming one next to it would read as 「these were deleted
    /// from there」.
    pub directory: PathBuf,
    pub moved: Vec<String>,
    pub deleted: Vec<String>,
    pub missing: Vec<String>,
    pub kept: Vec<Kept>,
    pub failures: Vec<Failure>,
}

impl Report {
    fn new(directory: &Path) -> Report {
        Report {
            directory: directory.to_path_buf(),
            moved: Vec::new(),
            deleted: Vec::new(),
            missing: Vec::new(),
            kept: Vec::new(),
            failures: Vec::new(),
        }
    }

    /// Record that `name` was left alone. Once per name and reason: a name that is
    /// a symlink in two directories is one thing the person has to hear about.
    fn keep(&mut self, name: &str, reason: KeptReason) {
        if !self
            .kept
            .iter()
            .any(|kept| kept.name == name && kept.reason == reason)
        {
            self.kept.push(Kept {
                name: name.to_string(),
                reason,
            });
        }
    }

    fn fail(&mut self, name: &str, reason: String) {
        self.failures.push(Failure {
            name: name.to_string(),
            reason,
        });
    }

    /// The name was in none of the readable directories.
    ///
    /// A **failure** rather than a `missing` when one directory could not be looked
    /// in, because 「没找到」 and 「没能查」 are different answers about different
    /// things and the report has no second list to put the second one in. The
    /// person can act on either, and the sentence says which it is.
    fn absent(&mut self, scouted: &Scouted) {
        match scouted.unread.first() {
            Some(reason) => self.fail(
                &scouted.name,
                format!("{}，无法确认它是否在那里", reason.sentence()),
            ),
            None => self.missing.push(scouted.name.clone()),
        }
    }
}

/// Whether the error is a rename across two filesystems.
///
/// `EXDEV`, spelled through `rustix` rather than as a number: it is 18 on the
/// platforms this ships on, and that is a coincidence nobody should write down.
/// `rename` can only report it at all because `rustix` is built with the
/// operations feature — without it a cross-volume rename on macOS answers success.
fn is_cross_device(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(Errno::XDEV.raw_os_error())
}

/// Move one file into `target`, across volumes when it has to be.
///
/// `rename` first: it is atomic on one volume and costs no extra space, so the
/// ordinary move (both directories on the same disk) is a single filesystem
/// operation, and `a_move_on_one_volume_renames_rather_than_copying` holds that.
///
/// The existence of the destination is re-checked immediately before either
/// operation, so a file that appeared under that name since the caller looked is
/// refused rather than replaced — the caller's plan and this check are two readings
/// of the same directory, and the one that acts is the later.
fn move_one(source: &Path, target: &Path, name: &str) -> Result<(), String> {
    let destination = target.join(name);
    // A name with a date subdirectory needs that subdirectory in the target too;
    // a flat name's parent is the target itself, where this is a no-op.
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法在目标位置创建文件夹：{error}"))?;
    }
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err("目标位置已有同名文件".to_string());
    }
    match std::fs::rename(source, &destination) {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device(&error) => copy_in_place(source, &destination, target, name),
        Err(error) => Err(error.to_string()),
    }
}

/// The cross-volume half of [`move_one`]: copy, sync, rename, then remove.
///
/// The order is `settings::save`'s — write a sibling, `sync_all`, put it in place —
/// plus the step that has no analogue there: the source is removed **last**, so a
/// failure at any point leaves the file where it was rather than in neither place.
///
/// A `.moving` file left behind by a crash is never reused: `create_new` refuses
/// it, and the refusal is **reported** rather than worked around. The refusal is
/// also the moment the cleanup below must not act — the temporary file is not this
/// call's until `create_new` returns `Ok`, and a `remove_file` on the error path of
/// the open would delete a leftover this move knows nothing about. That is not a
/// hypothetical: the first draft of this function did exactly that, and
/// `a_pending_moving_file_is_never_reused` is the test that caught it.
fn copy_in_place(
    source: &Path,
    destination: &Path,
    target: &Path,
    name: &str,
) -> Result<(), String> {
    let temp = target.join(format!("{name}{MOVING_SUFFIX}"));
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                // Named, and with the way out: a person reading 「File exists
                // (os error 17)」 beside a file name cannot act on it, and the
                // file to delete is not one whose name they know — it carries a
                // suffix only this app writes.
                format!("目标位置有一个没写完的临时文件（{name}{MOVING_SUFFIX}）：先把它删掉再搬")
            } else {
                error.to_string()
            }
        })?;
    let mut copy = || -> std::io::Result<()> {
        let mut from = std::fs::File::open(source)?;
        std::io::copy(&mut from, &mut to)?;
        // Before the rename, not after: without it a power cut can leave the
        // rename durable and the contents not.
        to.sync_all()
    };
    if let Err(error) = copy() {
        std::fs::remove_file(&temp).ok();
        return Err(error.to_string());
    }
    if let Err(error) = std::fs::rename(&temp, destination) {
        std::fs::remove_file(&temp).ok();
        return Err(error.to_string());
    }
    std::fs::remove_file(source)
        .map_err(|error| format!("已复制到目标位置，但原文件删不掉（{error}）；两个位置现在都有它"))
}

/// Move every name that can be moved into `target`.
///
/// Refused over an unreadable target, for [`plan`]'s reason. Everything else is per
/// name and never stops the run: one file that cannot be moved is reported beside
/// the ones that were, which is the shape [`Report`] exists for.
pub fn move_files(dirs: &[PathBuf], target: &Path, names: &[String]) -> Result<Report, String> {
    let mut report = Report::new(target);
    for name in validated(names)? {
        let scouted = scout(dirs, name)?;
        if let Some(problem) = scouted.problem_with(target) {
            return Err(problem);
        }
        if scouted.blocking_in(target).is_some() {
            // The target holds that name and it is not a regular file. Writing over
            // it would destroy something this command was never asked about.
            report.keep(name, KeptReason::TargetExists);
            continue;
        }
        if scouted.in_dir(target).is_some() {
            // Already where it was asked to go. A duplicate in an older directory
            // is left alone here too: 「把这个名字搬到目标」 is not 「删掉别的副本」, and
            // removing a second file is what 「删除」 is for.
            report.keep(name, KeptReason::SameDirectory);
            continue;
        }
        let Some(source) = scouted.outside(target) else {
            match scouted.blocking.first() {
                Some(entry) => report.keep(name, KeptReason::NotAFile(entry.kind)),
                None => report.absent(&scouted),
            }
            continue;
        };
        match move_one(&source.directory.join(name), target, name) {
            Ok(()) => report.moved.push(name.to_string()),
            Err(reason) => report.fail(name, reason),
        }
    }
    Ok(report)
}

/// Delete every regular file with one of these names, wherever it is.
///
/// **Every** directory, including the target: a name the page offered is a file the
/// person said is no longer wanted, and deleting only the copy in one directory
/// would leave the other behind while reporting success.
///
/// Non-regular entries are never deleted (see this module's header). A directory
/// that could not be listed makes the name a failure rather than a deletion — the
/// command cannot claim to have removed a file it could not look for.
///
/// No target refusal here: deleting does not write into the target, so a target
/// nobody can list is one unreadable entry among the others rather than a reason to
/// refuse the whole request — `a_target_that_cannot_be_read_does_not_refuse_a_delete`
/// is that difference.
pub fn delete_files(dirs: &[PathBuf], target: &Path, names: &[String]) -> Result<Report, String> {
    let mut report = Report::new(target);
    for name in validated(names)? {
        let scouted = scout(dirs, name)?;
        let mut removed = 0usize;
        for file in &scouted.files {
            match std::fs::remove_file(file.directory.join(name)) {
                Ok(()) => removed += 1,
                Err(error) => report.fail(name, error.to_string()),
            }
        }
        if removed > 0 {
            report.deleted.push(name.to_string());
        }
        for entry in &scouted.blocking {
            report.keep(name, KeptReason::NotAFile(entry.kind));
        }
        if removed == 0 && scouted.blocking.is_empty() {
            report.absent(&scouted);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory of this test's own, named after the test binary's pid
    /// so two concurrent runs cannot collide, and removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-saved-files-{}-{}-{}",
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

    /// A directory with a file of that name in it.
    fn holding(root: &Path, label: &str, name: &str, body: &[u8]) -> PathBuf {
        let directory = empty(root, label);
        std::fs::write(directory.join(name), body).expect("a file");
        directory
    }

    /// An empty directory, for the arms that need a readable place with nothing in
    /// it.
    fn empty(root: &Path, label: &str) -> PathBuf {
        let directory = root.join(label);
        std::fs::create_dir_all(&directory).expect("a directory");
        directory
    }

    /// A directory whose contents cannot be listed, unlocked by the caller before
    /// it is removed — a directory with no permission bits cannot be deleted, and
    /// leaving one behind fails the next run.
    fn locked(root: &Path, label: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let directory = empty(root, label);
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o000))
            .expect("lock it");
        directory
    }

    /// Let a directory go again, so the scratch tree can be removed.
    fn unlock(directory: &Path) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .expect("unlock");
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    /// The `kept` reasons a report lists, in the order given.
    fn kept_reasons(report: &Report) -> Vec<&'static str> {
        report
            .kept
            .iter()
            .map(|kept| kept.reason.as_str())
            .collect()
    }

    /// The names of a directory's entries, sorted — for the assertions that care
    /// what a move left behind.
    fn listing(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .expect("the directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    fn size_of_file(path: &Path) -> Option<u64> {
        std::fs::metadata(path).ok().map(|meta| meta.len())
    }

    // ---- the lookup -------------------------------------------------------

    /// [`find_in_known`] with one directory — the walk every test below uses.
    ///
    /// A helper rather than a second lookup, because a machine with one known
    /// directory is what a fresh install has and the assertions read better than
    /// `&[root.clone()]` twelve times. What it exercises is the shipped function:
    /// the sentence a miss produces names that one directory, and the tests below
    /// pin it.
    fn find_in_the(directory: &Path, name: &str) -> Result<PathBuf, String> {
        find_in_known(&[directory.to_path_buf()], name)
    }

    #[test]
    fn a_name_in_the_save_directory_comes_back_as_its_path() {
        let root = scratch("found");
        let wanted = root.join("标题-素材.mp4");
        std::fs::write(&wanted, b"x").expect("a downloaded file");

        let found = find_in_the(&root, "标题-素材.mp4");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.expect("it is there"), wanted);
    }

    /// A name one level deep (`20260930/游戏-素材.mp4`) is looked up inside the
    /// subdirectory, and the full path — with the subdirectory — is the answer,
    /// which is what 「打开文件」 opens and what 「在文件夹中显示」 reveals the parent of.
    #[test]
    fn a_name_one_level_deep_comes_back_as_its_full_path() {
        let root = scratch("subdir");
        let subdir = root.join("20260930");
        std::fs::create_dir_all(&subdir).expect("the date directory");
        let wanted = subdir.join("三角洲行动-30.mp4");
        std::fs::write(&wanted, b"x").expect("a downloaded file");

        let found = find_in_the(&root, "20260930/三角洲行动-30.mp4");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.expect("it is there"), wanted);
    }

    /// A subdirectory is a place to look, never a thing to follow.
    ///
    /// `20260930` here is a link to the save directory's parent, where a file of
    /// the name asked for lives: a lookup that descended through the link would
    /// open a file the save directory does not contain.
    #[test]
    fn a_symlinked_subdirectory_is_not_followed() {
        use std::os::unix::fs::symlink;

        let parent = scratch("linked-subdir");
        let save = parent.join("save");
        std::fs::create_dir_all(&save).expect("the save directory");
        std::fs::write(parent.join("secret.mp4"), b"x").expect("a file outside");
        symlink(&parent, save.join("20260930")).expect("a link for the subdirectory");

        let error = find_in_the(&save, "20260930/secret.mp4");

        std::fs::remove_dir_all(&parent).ok();

        let error = error.expect_err("a link must not be descended into");
        assert!(
            error.contains("不在本机已知的保存位置里"),
            "the link was answered as nothing found, not as a file: {error}"
        );
    }

    /// A subdirectory that is a file holds no download under it.
    #[test]
    fn a_subdirectory_that_is_a_file_is_not_descended_into() {
        let root = scratch("file-subdir");
        std::fs::write(root.join("20260930"), b"not a directory").expect("a file");

        let error = find_in_the(&root, "20260930/三角洲行动-30.mp4");

        std::fs::remove_dir_all(&root).ok();

        assert!(
            error
                .expect_err("nothing to look in")
                .contains("不在本机已知的保存位置里"),
            "a file-shaped subdirectory holds no file under it"
        );
    }

    /// Every name a listing gives is looked up **as it is spelled**.
    ///
    /// The property the page depends on: the drawer shows names that came from the
    /// task records, and each of them has to be openable. Driven from the listing
    /// itself rather than from this test's literals, so the fixture and the input
    /// cannot be two different sets of names — and the awkward shapes are in it
    /// deliberately, because a lookup that trimmed its input would pass a fixture of
    /// tidy names and leave 「列表里有、点开说没有」 as the defect.
    #[test]
    fn every_name_in_a_listing_is_looked_up_as_it_is_spelled() {
        let root = scratch("listing");
        for name in [
            "plain.mp4",
            "with space.mp4",
            " leading-space.mp4",
            "trailing-space.mp4 ",
            "a.b.c.mp4",
            ".hidden.mp4",
            "emoji-\u{1f3ac}.mp4",
        ] {
            std::fs::write(root.join(name), b"x").unwrap_or_else(|error| panic!("{name}: {error}"));
        }
        let listed: Vec<(String, PathBuf)> = std::fs::read_dir(&root)
            .expect("the listing")
            .map(|entry| {
                let entry = entry.expect("an entry");
                (
                    entry.file_name().to_str().expect("a name").to_string(),
                    entry.path(),
                )
            })
            .collect();

        let mut looked_up = Vec::new();
        for (name, path) in &listed {
            let found = find_in_the(&root, name)
                .unwrap_or_else(|error| panic!("the listing offered {name:?}: {error}"));
            looked_up.push((name.clone(), found, path.clone()));
        }

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(looked_up.len(), 7, "the fixture itself: {looked_up:?}");
        for (name, found, path) in looked_up {
            assert_eq!(found, path, "「{name}」 resolved to the wrong entry");
        }
    }

    /// A name the directory does not hold is reported as missing, not as a path.
    ///
    /// Both halves matter: the sentence says which directory and which name — a
    /// person has to be able to see whether it is the folder they meant — and it is
    /// a different sentence from the path refusal, so a page (or a log) can tell
    /// 「名字不对」 from 「这种输入根本不收」.
    #[test]
    fn a_name_that_is_not_in_the_directory_is_reported_as_missing() {
        let root = scratch("missing");
        std::fs::write(root.join("kept.mp4"), b"x").expect("a file");

        let error = find_in_the(&root, "gone.mp4").expect_err("must not be found");

        std::fs::remove_dir_all(&root).ok();

        assert!(
            error.contains("gone.mp4 不在本机已知的保存位置里"),
            "{error}"
        );
        assert!(
            error.contains(&root.display().to_string()),
            "the directory that was looked in is named: {error}"
        );
        assert!(!error.contains("不是一个文件名"), "{error}");
    }

    /// A path is not a name, in every spelling that could name one — and the
    /// spellings that **are** a name (one component, or one subdirectory and one
    /// component) are looked up rather than refused.
    ///
    /// Enumerated rather than sampled, and split along the rule's own boundary,
    /// because the check is 「a name is one or two path components」 and not 「the
    /// text looks tidy」: on this platform `\` and a space are ordinary
    /// characters, so `C:\Windows` and `"   "` are names, and a test that claimed
    /// otherwise would pass for the wrong reason. What matters for both groups is
    /// that neither reaches anything outside the save directory.
    #[test]
    fn a_path_is_not_a_name() {
        use std::os::unix::fs::symlink;

        let parent = scratch("shapes");
        let save = parent.join("save");
        std::fs::create_dir_all(&save).expect("the save directory");
        // Outside the save directory, so a lookup that joined the name onto the
        // directory would find something and the refusal would be a lie.
        let outside = parent.join("secret.mp4");
        std::fs::write(&outside, b"x").expect("a file the page must not reach");
        symlink(&outside, save.join("linked.mp4")).expect("a link to it");
        let outside_name = outside.display().to_string();

        let not_a_name = [
            "",
            ".",
            "..",
            "/",
            "/etc/passwd",
            "../secret.mp4",
            "a/b/secret.mp4",
            "20260930/../secret.mp4",
            "file:///etc/passwd",
            outside_name.as_str(),
        ];
        let one_component = [
            "   ",
            "C:\\Windows",
            "..hidden.mp4",
            "sub/secret.mp4",
            "~/secret.mp4",
        ];

        let mut wrong = Vec::new();
        for name in not_a_name {
            match find_in_the(&save, name) {
                Ok(found) => wrong.push(format!("{name:?} was accepted as {}", found.display())),
                Err(error) if !error.contains("不是一个文件名") => {
                    wrong.push(format!("{name:?} got the wrong sentence: {error}"))
                }
                Err(_) => {}
            }
        }
        for name in one_component {
            match find_in_the(&save, name) {
                Ok(found) => wrong.push(format!("{name:?} was accepted as {}", found.display())),
                Err(error) if !error.contains("不在本机已知的保存位置里") => {
                    wrong.push(format!("{name:?} got the wrong sentence: {error}"))
                }
                Err(_) => {}
            }
        }
        // The link is in the directory and is still not a download: a name that is
        // one component does not get to reach whatever it points at.
        let linked = find_in_the(&save, "linked.mp4");

        std::fs::remove_dir_all(&parent).ok();

        assert!(
            wrong.is_empty(),
            "{} shapes, {} wrong: {wrong:?}",
            not_a_name.len() + one_component.len(),
            wrong.len()
        );
        assert_eq!(not_a_name.len(), 10, "the refused group, as written above");
        assert_eq!(
            one_component.len(),
            5,
            "the looked-up group, as written above"
        );
        let linked = linked.expect_err("a symlink is not a downloaded file");
        assert!(
            !linked.contains("secret"),
            "the refusal must not say where the link pointed: {linked}"
        );
    }

    /// A directory that shares the name is not a download either.
    #[test]
    fn a_directory_that_shares_the_name_is_not_a_download() {
        let root = scratch("directory");
        std::fs::create_dir_all(root.join("movie.mp4")).expect("a directory named like one");

        let error = find_in_the(&root, "movie.mp4").expect_err("not a file");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("不在本机已知的保存位置里"), "{error}");
    }

    /// A save directory that is not a directory says so, and says what to do.
    #[test]
    fn a_save_directory_that_is_not_a_directory_is_reported() {
        let root = scratch("stale");
        let file = root.join("was-a-folder");
        std::fs::write(&file, b"x").expect("a file where the directory was");

        let error = find_in_the(&file, "movie.mp4").expect_err("not a directory");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("不是一个目录"), "{error}");
        assert!(
            error.contains("重新选"),
            "it has to say what to do: {error}"
        );
    }

    /// A save directory that cannot be listed at all is reported as that.
    ///
    /// The arm an unmounted volume or a changed permission produces, and the one
    /// that must not be mistaken for 「还没有下载」: the message names the directory,
    /// and the directory is still there afterwards.
    #[test]
    fn a_save_directory_that_cannot_be_listed_is_reported() {
        let root = scratch("locked");
        let directory = locked(&root, "locked");

        let error = find_in_the(&directory, "movie.mp4").expect_err("cannot be listed");

        unlock(&directory);
        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("无法读取保存位置"), "{error}");
        assert!(error.contains(&directory.display().to_string()), "{error}");
    }

    /// A file in the **older** directory is found there.
    ///
    /// This is the defect 「已下载的改完保存路径后就找不到文件了」 as an assertion,
    /// and the older directory is deliberately the *second* one: a lookup that only
    /// ever tried the directory in use would pass a test whose file sits in the
    /// current folder, and would fail this one.
    #[test]
    fn a_file_in_an_older_save_directory_is_found_there() {
        let root = scratch("history-hit");
        let current = empty(&root, "now");
        let older = holding(&root, "before", "标题-素材.mp4", b"x");

        let found = find_in_known(&[current, older.clone()], "标题-素材.mp4");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            found.expect("it is still there"),
            older.join("标题-素材.mp4")
        );
    }

    /// The directory in use wins when both hold the name.
    ///
    /// Order is not decoration: the list is newest-first, and a file re-downloaded
    /// after a move would otherwise be shadowed by the copy it replaced.
    #[test]
    fn the_newest_directory_wins_when_the_name_is_in_two_of_them() {
        let root = scratch("newest-wins");
        let current = holding(&root, "now", "same.mp4", b"new");
        let older = holding(&root, "before", "same.mp4", b"old");

        let found = find_in_known(&[current.clone(), older], "same.mp4");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.expect("found"), current.join("same.mp4"));
    }

    /// A name in none of them names every directory that was looked in.
    ///
    /// 「查了哪些地方」 has to be in the sentence: the defect this replaced named the
    /// current directory as though it were the only one known, so a person whose
    /// file was in their previous folder was told it might have been deleted. Both
    /// directories here are readable and empty, so the message may not hedge.
    #[test]
    fn a_name_in_no_known_directory_names_every_directory_searched() {
        let root = scratch("history-miss");
        let current = empty(&root, "now");
        let older = empty(&root, "before");

        let error = find_in_known(&[current.clone(), older.clone()], "gone.mp4")
            .expect_err("nowhere to be found");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("已经查过"), "{error}");
        assert!(error.contains(&current.display().to_string()), "{error}");
        assert!(error.contains(&older.display().to_string()), "{error}");
        assert!(!error.contains("没能查"), "nothing was unreadable: {error}");
        assert!(
            error.contains("重新下载"),
            "and it has to say what the way out is: {error}"
        );
    }

    /// A directory nobody can read is reported as unread, not as an absence.
    ///
    /// The distinction the whole message turns on: 「已经查过 X」 is evidence that the
    /// file is not there, and 「X 没能查」 is the absence of evidence. Merging them is
    /// how a file sitting on an unplugged volume gets reported as deleted, and how
    /// somebody re-downloads 230 MB they still have.
    #[test]
    fn a_directory_that_cannot_be_read_is_not_reported_as_an_absence() {
        let root = scratch("history-unreadable");
        let current = empty(&root, "now");
        let directory = locked(&root, "locked");

        let error = find_in_known(&[current.clone(), directory.clone()], "gone.mp4")
            .expect_err("nowhere to be found");

        unlock(&directory);
        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("没能查"), "{error}");
        assert!(error.contains(&directory.display().to_string()), "{error}");
        assert!(
            error.contains(&current.display().to_string()),
            "the directory that *was* read is still named: {error}"
        );
    }

    /// A directory that is no longer a directory is one of the unread ones.
    ///
    /// The unmounted-volume shape, and the one a person reaches by deleting the
    /// folder: it is listed with its own sentence rather than swallowed.
    #[test]
    fn a_directory_that_is_no_longer_a_directory_is_named_as_unread() {
        let root = scratch("history-not-a-dir");
        let gone = root.join("was-a-folder");
        std::fs::write(&gone, b"x").expect("a file where the directory was");

        let error = find_in_known(&[gone.clone()], "movie.mp4").expect_err("nowhere to look");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("没能查"), "{error}");
        assert!(error.contains("不是一个目录"), "{error}");
    }

    /// Nothing ever chosen has nowhere to look, and says that.
    #[test]
    fn an_empty_search_space_says_nothing_has_been_chosen() {
        let error = find_in_known(&[], "movie.mp4").expect_err("nowhere to look");

        assert!(error.contains("还没有选择下载保存位置"), "{error}");
    }

    /// A path is refused before any directory is touched.
    ///
    /// The rule does not weaken because there are now several directories to search
    /// — the name is checked once, at the top, and the directories are never
    /// consulted for a name that is not one. Asserted with a directory list that
    /// **would** have found it, so a lookup that searched first and refused later
    /// cannot pass.
    #[test]
    fn a_path_shaped_name_is_refused_before_any_directory_is_searched() {
        let root = scratch("history-name");
        std::fs::write(root.join("movie.mp4"), b"x").expect("a file");

        let error = find_in_known(&[root.clone()], "../movie.mp4").expect_err("not a name");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("不是一个文件名"), "{error}");
        assert!(
            !error.contains(&root.display().to_string()),
            "the directories must not even be listed: {error}"
        );
    }

    // ---- where a file is --------------------------------------------------

    #[test]
    fn a_file_in_the_directory_in_use_is_answered_as_current() {
        let root = scratch("presence-current");
        let current = holding(&root, "now", "a.mp4", b"12345");

        let found =
            presences(&[current.clone()], Some(&current), &names(&["a.mp4"])).expect("a read");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].directory.as_deref(), Some(current.as_path()));
        assert!(found[0].current, "it is in the directory in use");
        assert_eq!(found[0].bytes, Some(5));
    }

    /// A file in an older directory is answered with that directory and **not** as
    /// current — the drawer's 「文件还在原来的保存位置：…」 line is this flag.
    #[test]
    fn a_file_in_an_older_directory_is_answered_as_not_current() {
        let root = scratch("presence-older");
        let current = empty(&root, "now");
        let older = holding(&root, "before", "a.mp4", b"12345");

        let found = presences(
            &[current.clone(), older.clone()],
            Some(&current),
            &names(&["a.mp4"]),
        )
        .expect("a read");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found[0].directory.as_deref(), Some(older.as_path()));
        assert!(
            !found[0].current,
            "the older directory is not where new files go"
        );
    }

    /// A name that is nowhere is answered with no directory — and **not** with the
    /// directory in use, which is how 「文件被删了」 would be said about a file that
    /// was simply never downloaded.
    #[test]
    fn a_name_that_is_nowhere_is_answered_with_no_directory() {
        let root = scratch("presence-absent");
        let current = empty(&root, "now");

        let found =
            presences(&[current.clone()], Some(&current), &names(&["gone.mp4"])).expect("a read");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found[0].directory, None);
        assert!(!found[0].current);
        assert_eq!(found[0].bytes, None, "and no size is invented for it");
    }

    /// A machine that has never chosen has no target, and nothing is current.
    ///
    /// The `Option` target's own case: a `&Path` parameter would have to be some
    /// directory, and a file in it would be reported as 「在当前位置」 on a machine
    /// where there is no current place.
    #[test]
    fn a_machine_with_no_chosen_directory_has_no_current_file() {
        let root = scratch("presence-no-target");
        let older = holding(&root, "before", "a.mp4", b"x");

        let found = presences(&[older.clone()], None, &names(&["a.mp4"])).expect("a read");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found[0].directory.as_deref(), Some(older.as_path()));
        assert!(!found[0].current, "there is no place for it to be current");
    }

    /// A download filed under a date subdirectory is in the save directory, and
    /// its file is that directory's own — `directory` and `current` keep meaning
    /// the save root, not the subdirectory inside it.
    #[test]
    fn a_file_in_a_date_subdirectory_is_present_and_current() {
        let root = scratch("presence-subdir");
        let current = empty(&root, "now");
        std::fs::create_dir_all(current.join("20260930")).expect("the date directory");
        std::fs::write(current.join("20260930/三角洲行动-30.mp4"), b"0123456789").expect("a file");

        let found = presences(
            &[current.clone()],
            Some(&current),
            &names(&["20260930/三角洲行动-30.mp4"]),
        )
        .expect("a read");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found[0].directory.as_deref(), Some(current.as_path()));
        assert!(found[0].current, "it is in the directory in use");
        assert_eq!(found[0].bytes, Some(10));
    }

    /// A symlink is not a downloadable file, so it is answered as 「不在这儿」.
    ///
    /// The answer is about *the download*, and the plan's own finer vocabulary is
    /// what anything that has to act uses. What matters here is that the read does
    /// not follow the link and report its target's size.
    #[test]
    fn a_symlink_is_not_answered_as_a_download() {
        use std::os::unix::fs::symlink;

        let root = scratch("presence-symlink");
        let current = holding(&root, "now", "real.mp4", b"0123456789");
        symlink(current.join("real.mp4"), current.join("linked.mp4")).expect("a link");

        let found =
            presences(&[current.clone()], Some(&current), &names(&["linked.mp4"])).expect("a read");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found[0].directory, None);
        assert_eq!(found[0].bytes, None);
    }

    /// One name asked once, however many times it is listed in `names`.
    #[test]
    fn a_name_asked_twice_is_answered_once() {
        let root = scratch("presence-dupe");
        let current = holding(&root, "now", "a.mp4", b"x");

        let found = presences(
            &[current.clone()],
            Some(&current),
            &names(&["a.mp4", "a.mp4"]),
        )
        .expect("a read");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.len(), 1, "{found:?}");
    }

    // ---- the plan ---------------------------------------------------------

    /// A file in an older directory with a free name in the target is movable, and
    /// its size is what the move needs.
    #[test]
    fn a_file_in_an_older_directory_is_movable_and_its_size_is_counted() {
        let root = scratch("plan-movable");
        let target = holding(&root, "now", "kept.mp4", b"x");
        let older = holding(&root, "before", "moves.mp4", b"0123456789");

        let planned = plan(
            &[target.clone(), older.clone()],
            &target,
            &names(&["moves.mp4"]),
            1024,
        )
        .expect("a plan");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(planned.movable.len(), 1);
        assert_eq!(planned.movable[0].directory, older);
        assert_eq!(planned.movable[0].bytes, Some(10));
        assert_eq!(planned.needed_bytes, 10);
        assert_eq!(planned.free_bytes, 1024);
        assert_eq!(planned.to, target);
    }

    /// A file already in the target is not movable, and is not missing either.
    #[test]
    fn a_file_already_in_the_target_is_not_movable() {
        let root = scratch("plan-in-place");
        let target = holding(&root, "now", "a.mp4", b"x");

        let planned = plan(&[target.clone()], &target, &names(&["a.mp4"]), 0).expect("a plan");

        std::fs::remove_dir_all(&root).ok();

        assert!(planned.movable.is_empty());
        assert_eq!(planned.already_there.len(), 1);
        assert!(planned.missing.is_empty());
        assert_eq!(planned.needed_bytes, 0);
    }

    /// A name in no known directory is missing — with every directory read.
    #[test]
    fn a_name_in_no_known_directory_is_missing() {
        let root = scratch("plan-missing");
        let target = empty(&root, "now");

        let planned = plan(&[target.clone()], &target, &names(&["gone.mp4"]), 0).expect("a plan");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(planned.missing, names(&["gone.mp4"]));
        assert!(planned.movable.is_empty());
    }

    /// A directory nobody could read is listed, and is **not** silently a
    /// 「已不存在」 for the names that were in it.
    ///
    /// Both names are asked at once: they are still `missing` (the plan carries
    /// `unreadable` beside them, and that list is what tells the page to use the
    /// weaker word), and the unreadable directory is named once even though two
    /// names were walked through it.
    #[test]
    fn an_unreadable_directory_is_listed_once_beside_the_plan() {
        let root = scratch("plan-unreadable");
        let target = empty(&root, "now");
        let directory = locked(&root, "locked");

        let planned = plan(
            &[target.clone(), directory.clone()],
            &target,
            &names(&["gone.mp4", "also-gone.mp4"]),
            0,
        )
        .expect("a plan");

        unlock(&directory);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(planned.unreadable.len(), 1, "{:?}", planned.unreadable);
        assert_eq!(planned.unreadable[0].directory, directory);
        assert!(!planned.unreadable[0].gone, "it is still a directory");
        assert_eq!(planned.movable.len() + planned.already_there.len(), 0);
        assert_eq!(planned.missing.len(), 2, "the names are still reported");
    }

    /// The target itself being unreadable is refused, not planned around.
    ///
    /// The one case the operator has to act on: a move writes into that directory,
    /// and a plan about a directory nobody has seen is a plan about nothing.
    #[test]
    fn a_target_that_cannot_be_read_refuses_the_plan() {
        let root = scratch("plan-target-locked");
        let target = locked(&root, "now");

        let planned = plan(&[target.clone()], &target, &names(&["a.mp4"]), 0);

        unlock(&target);
        std::fs::remove_dir_all(&root).ok();

        let error = planned.expect_err("must be refused");
        assert!(error.contains("无法读取保存位置"), "{error}");
    }

    /// An entry in the target that is not a regular file blocks the name.
    ///
    /// A directory called `a.mp4` in the target is not a download, and moving a file
    /// onto that name would either destroy it or fail — so the name is not offered
    /// as movable even though the file it names is sitting in an older directory.
    #[test]
    fn an_entry_in_the_target_that_is_not_a_file_blocks_the_move() {
        let root = scratch("plan-target-taken");
        let target = empty(&root, "now");
        std::fs::create_dir_all(target.join("a.mp4")).expect("a directory named like a download");
        let older = holding(&root, "before", "a.mp4", b"x");

        let planned =
            plan(&[target.clone(), older], &target, &names(&["a.mp4"]), 0).expect("a plan");

        std::fs::remove_dir_all(&root).ok();

        assert!(planned.movable.is_empty(), "{:?}", planned.movable);
        assert_eq!(
            planned.missing,
            names(&["a.mp4"]),
            "the name is reported, and the plan's own lists are all it has"
        );
    }

    /// A path-shaped name is refused before the plan is built at all.
    #[test]
    fn a_path_shaped_name_refuses_the_plan() {
        let root = scratch("plan-name");
        let target = holding(&root, "now", "a.mp4", b"x");

        let planned = plan(&[target.clone()], &target, &names(&["../a.mp4"]), 0);

        std::fs::remove_dir_all(&root).ok();

        assert!(planned.expect_err("refused").contains("不是一个文件名"));
    }

    // ---- moving -----------------------------------------------------------

    /// One file moves, and only that one.
    ///
    /// The positive control is the second file in the same directory: a command that
    /// swept the folder (or walked it by prefix) would take it too, and the
    /// assertion that it is still there is what makes 「只动它拿到的那些名字」 a
    /// measurement rather than a hope.
    #[test]
    fn a_move_carries_the_named_files_and_leaves_the_others() {
        let root = scratch("move-one");
        let target = holding(&root, "now", "kept.mp4", b"x");
        let older = holding(&root, "before", "moves.mp4", b"0123456789");
        std::fs::write(older.join("not-listed.mp4"), b"y").expect("a file nobody named");

        let report = move_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["moves.mp4"]),
        )
        .expect("a move");

        let size = size_of_file(&target.join("moves.mp4"));
        let source_gone = !older.join("moves.mp4").exists();
        let left_behind = older.join("not-listed.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(report.moved, names(&["moves.mp4"]));
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(report.missing.is_empty(), "{:?}", report.missing);
        assert_eq!(size, Some(10), "the bytes came with it");
        assert!(source_gone, "and the copy it came from is gone");
        assert!(
            left_behind,
            "a file nobody named must still be where it was"
        );
    }

    /// A file under a date subdirectory moves with its subdirectory.
    #[test]
    fn a_file_in_a_date_subdirectory_moves_with_its_subdirectory() {
        let root = scratch("move-subdir");
        let target = empty(&root, "now");
        let older = empty(&root, "before");
        std::fs::create_dir_all(older.join("20260930")).expect("the date directory");
        std::fs::write(older.join("20260930/三角洲行动-30.mp4"), b"0123456789")
            .expect("a downloaded file");

        let report = move_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["20260930/三角洲行动-30.mp4"]),
        )
        .expect("a move");

        let moved = std::fs::read(target.join("20260930/三角洲行动-30.mp4")).ok();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            report.moved,
            names(&["20260930/三角洲行动-30.mp4"]),
            "{:?}",
            report.failures
        );
        assert_eq!(
            moved.as_deref(),
            Some(&b"0123456789"[..]),
            "the file came with its subdirectory"
        );
    }

    /// The move on one volume is a **rename**, not a copy and a delete.
    ///
    /// Asserted by the inode: a same-volume `rename` leaves the file the same
    /// object, while a copy writes a new one. That is the difference the plan pays
    /// for by trying `rename` first — the ordinary move is atomic and needs no extra
    /// space — and nothing else in this module would notice if it were replaced by a
    /// copy.
    #[test]
    fn a_move_on_one_volume_renames_rather_than_copying() {
        use std::os::unix::fs::MetadataExt;

        let root = scratch("move-rename");
        let target = empty(&root, "now");
        let older = holding(&root, "before", "a.mp4", b"x");
        let before = std::fs::metadata(older.join("a.mp4"))
            .expect("the source")
            .ino();

        let report = move_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["a.mp4"]),
        )
        .expect("a move");

        let after = std::fs::metadata(target.join("a.mp4")).map(|meta| meta.ino());
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(report.moved, names(&["a.mp4"]));
        assert_eq!(after.ok(), Some(before), "a rename keeps the same file");
    }

    /// A file already in the target is kept, with that reason, and untouched.
    #[test]
    fn a_file_already_in_the_target_is_kept_not_moved() {
        let root = scratch("move-in-place");
        let target = holding(&root, "now", "a.mp4", b"body");

        let report = move_files(&[target.clone()], &target, &names(&["a.mp4"])).expect("a move");

        let body = std::fs::read_to_string(target.join("a.mp4")).ok();
        std::fs::remove_dir_all(&root).ok();

        assert!(report.moved.is_empty());
        assert_eq!(kept_reasons(&report), vec!["same_directory"]);
        assert_eq!(body.as_deref(), Some("body"));
    }

    /// The target's own entry is never overwritten.
    ///
    /// The file in the target here is the one that must survive: a move that wrote
    /// over it would destroy a file the person still has and report success.
    #[test]
    fn a_move_never_overwrites_what_is_already_in_the_target() {
        let root = scratch("move-no-clobber");
        let target = holding(&root, "now", "a.mp4", b"the-original");
        let older = holding(&root, "before", "a.mp4", b"the-older-copy");

        let report = move_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["a.mp4"]),
        )
        .expect("a move");

        let in_target = std::fs::read_to_string(target.join("a.mp4")).ok();
        let still_old = older.join("a.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(report.moved.is_empty());
        assert_eq!(kept_reasons(&report), vec!["same_directory"], "{report:?}");
        assert_eq!(in_target.as_deref(), Some("the-original"));
        assert!(
            still_old,
            "the file the move declined to place is still there"
        );
    }

    /// An entry in the target that is not a file keeps the name where it is.
    #[test]
    fn a_name_taken_in_the_target_by_something_else_is_kept() {
        let root = scratch("move-target-taken");
        let target = empty(&root, "now");
        std::fs::create_dir_all(target.join("a.mp4")).expect("a directory named like a download");
        let older = holding(&root, "before", "a.mp4", b"x");

        let report = move_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["a.mp4"]),
        )
        .expect("a move");

        let source_there = older.join("a.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(report.moved.is_empty());
        assert_eq!(kept_reasons(&report), vec!["target_exists"], "{report:?}");
        assert!(
            source_there,
            "and nothing was moved out of the older directory"
        );
    }

    /// A symlink is kept, with the reason that says which kind it is.
    #[test]
    fn a_symlink_is_kept_rather_than_moved() {
        use std::os::unix::fs::symlink;

        let root = scratch("move-symlink");
        let target = empty(&root, "now");
        let older = holding(&root, "before", "real.mp4", b"x");
        symlink(older.join("real.mp4"), older.join("linked.mp4")).expect("a link");

        let report = move_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["linked.mp4"]),
        )
        .expect("a move");

        let link_still_there = older.join("linked.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(report.moved.is_empty());
        assert_eq!(kept_reasons(&report), vec!["symlink"]);
        assert!(link_still_there, "and it is where it was");
    }

    /// A name that is nowhere is missing, and the run continues.
    ///
    /// One name that cannot be found must not stop the others: the positive control
    /// is the second name, which moves in the same call.
    #[test]
    fn a_name_that_is_nowhere_does_not_stop_the_other_names() {
        let root = scratch("move-missing");
        let target = empty(&root, "now");
        let older = holding(&root, "before", "moves.mp4", b"x");

        let report = move_files(
            &[target.clone(), older],
            &target,
            &names(&["gone.mp4", "moves.mp4"]),
        )
        .expect("a move");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(report.missing, names(&["gone.mp4"]));
        assert_eq!(report.moved, names(&["moves.mp4"]));
    }

    /// A directory nobody could read makes the name a **failure**, not a 「没找到」.
    ///
    /// The report has no second list for 「没能查」, so the honest entry is the one
    /// that says the action did not happen — a name reported as missing would say
    /// the file is not on this machine, which is not something this run knows.
    #[test]
    fn a_name_behind_an_unreadable_directory_is_a_failure_not_a_missing() {
        let root = scratch("move-unreadable");
        let target = empty(&root, "now");
        let directory = locked(&root, "locked");

        let report = move_files(
            &[target.clone(), directory.clone()],
            &target,
            &names(&["a.mp4"]),
        )
        .expect("a move");

        unlock(&directory);
        std::fs::remove_dir_all(&root).ok();

        assert!(report.missing.is_empty(), "{:?}", report.missing);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        assert_eq!(report.failures[0].name, "a.mp4");
        assert!(report.failures[0].reason.contains("无法读取保存位置"));
    }

    /// The failure names the file and never a path.
    ///
    /// The command family's whole shape: no path crosses it. A failure that quoted
    /// one would be the exception that undoes it. The move really is attempted — the
    /// source directory is one this process may read and not write, so `rename` fails
    /// on the directory rather than on a check invented for the test.
    #[test]
    fn a_failure_names_the_file_and_never_a_path() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("move-failure-name");
        let target = empty(&root, "now");
        let older = holding(&root, "before", "moves.mp4", b"x");
        std::fs::set_permissions(&older, std::fs::Permissions::from_mode(0o500))
            .expect("a directory that may be read and not written");

        let report = move_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["moves.mp4"]),
        )
        .expect("a move");

        let source_there = older.join("moves.mp4").exists();
        std::fs::set_permissions(&older, std::fs::Permissions::from_mode(0o700)).expect("unlock");
        std::fs::remove_dir_all(&root).ok();

        let failure = report
            .failures
            .first()
            .unwrap_or_else(|| panic!("the move was expected to fail: {report:?}"));
        assert_eq!(failure.name, "moves.mp4");
        assert!(
            !failure.reason.contains(&older.display().to_string()),
            "the reason must not carry the source's path: {}",
            failure.reason
        );
        assert!(source_there, "and a failed move left the file where it was");
    }

    /// **One bad name refuses the whole request**, before anything moves.
    ///
    /// Not a per-name failure: a caller that sent a path where a name belongs is
    /// broken, and a partial move is a machine in a state nobody asked for. The
    /// positive control is the good name in the same call — it must still be where
    /// it was.
    #[test]
    fn one_bad_name_stops_the_whole_move_before_it_starts() {
        let root = scratch("move-bad-name");
        let target = empty(&root, "now");
        let older = holding(&root, "before", "good.mp4", b"x");

        let report = move_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["good.mp4", "/etc/passwd"]),
        );

        let untouched = older.join("good.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(report.expect_err("refused").contains("不是一个文件名"));
        assert!(untouched, "the good name must not have been moved anyway");
    }

    // ---- the cross-volume half --------------------------------------------

    /// The fallback copies, puts the copy in place, and only then removes the
    /// source.
    ///
    /// Called directly because a second volume is not something a test can arrange;
    /// everything the branch does is here. The `.moving` file is gone afterwards —
    /// the suffix is a name that exists between two of these lines and never after
    /// the last one.
    #[test]
    fn a_cross_volume_move_copies_syncs_renames_and_then_removes_the_source() {
        let root = scratch("cross-volume");
        let target = empty(&root, "now");
        let older = holding(&root, "before", "a.mp4", b"0123456789");

        let moved = copy_in_place(
            &older.join("a.mp4"),
            &target.join("a.mp4"),
            &target,
            "a.mp4",
        );

        let copied = std::fs::read_to_string(target.join("a.mp4")).ok();
        let source_gone = !older.join("a.mp4").exists();
        let leftovers = listing(&target);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(moved, Ok(()));
        assert_eq!(copied.as_deref(), Some("0123456789"));
        assert!(source_gone, "the source goes last");
        assert_eq!(leftovers, ["a.mp4"], "and the temporary name does not stay");
    }

    /// A `.moving` file left by an earlier crash is refused, not reused — and not
    /// **deleted** either.
    ///
    /// Three claims, and the second is the one that was wrong first: `create_new`
    /// makes the leftover non-reusable, the file it names is not this move's, so the
    /// failure path must not clean it up, and the refusal names the file to delete —
    /// a person told only that the file exists has no way to act. The first draft's
    /// cleanup ran on every error of the copy, including the open's
    /// `AlreadyExists`, and removed a half-copy that belonged to nobody in this
    /// call.
    #[test]
    fn a_pending_moving_file_is_never_reused() {
        let root = scratch("cross-volume-pending");
        let target = empty(&root, "now");
        let older = holding(&root, "before", "a.mp4", b"the real one");
        let pending = target.join(format!("a.mp4{MOVING_SUFFIX}"));
        std::fs::write(&pending, b"somebody else's half-copy").expect("a leftover");

        let moved = copy_in_place(
            &older.join("a.mp4"),
            &target.join("a.mp4"),
            &target,
            "a.mp4",
        );

        let pending_body = std::fs::read_to_string(&pending).ok();
        let source_there = older.join("a.mp4").exists();
        let in_place = target.join("a.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        let reason = moved.expect_err("the leftover is not this move's to write over");
        assert_eq!(
            pending_body.as_deref(),
            Some("somebody else's half-copy"),
            "the leftover is left exactly as it was, not cleaned up"
        );
        assert!(
            !in_place,
            "and nothing was put in place under the real name"
        );
        assert!(
            source_there,
            "the source is not removed by a copy that failed"
        );
        assert!(
            reason.contains(&format!("a.mp4{MOVING_SUFFIX}")),
            "the refusal names the file to delete: {reason}"
        );
    }

    /// A copy that fails part-way removes its temporary file and leaves the source
    /// alone.
    ///
    /// The source is one this process may not read, so `File::open` fails after the
    /// temporary file already exists — the state the cleanup exists for.
    #[test]
    fn a_copy_that_fails_leaves_no_temporary_file() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("cross-volume-failure");
        let target = empty(&root, "now");
        let older = holding(&root, "before", "a.mp4", b"x");
        let source = older.join("a.mp4");
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o000))
            .expect("an unreadable source");

        let moved = copy_in_place(&source, &target.join("a.mp4"), &target, "a.mp4");

        let leftovers = listing(&target);
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).expect("unlock");
        std::fs::remove_dir_all(&root).ok();

        assert!(moved.is_err(), "{moved:?}");
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// A destination that appeared after the caller looked is refused.
    ///
    /// The last check before the filesystem is asked to act, and the one that keeps
    /// 「不覆盖」 from being a promise about an earlier moment: what is on disk now is
    /// what decides. The file planted here stands in for that appearance.
    #[test]
    fn a_destination_that_appeared_is_refused_rather_than_replaced() {
        let root = scratch("move-appeared");
        let target = holding(&root, "now", "a.mp4", b"appeared late");
        let older = holding(&root, "before", "a.mp4", b"the older copy");

        let moved = move_one(&older.join("a.mp4"), &target, "a.mp4");

        let in_target = std::fs::read_to_string(target.join("a.mp4")).ok();
        let source_there = older.join("a.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        let reason = moved.expect_err("must be refused");
        assert!(reason.contains("已有同名文件"), "{reason}");
        assert_eq!(in_target.as_deref(), Some("appeared late"));
        assert!(source_there);
    }

    /// `EXDEV` is the error the fallback is for, and it is not a literal.
    ///
    /// Weak on its own — it compares `rustix`'s number with itself — and it is here
    /// for the one edit that would break the branch silently: a hardcoded 18, which
    /// is a different error on another platform and never fires on this one.
    #[test]
    fn only_the_cross_device_error_is_cross_device() {
        let xdev = std::io::Error::from_raw_os_error(Errno::XDEV.raw_os_error());
        let other = std::io::Error::from_raw_os_error(Errno::NOENT.raw_os_error());

        assert!(is_cross_device(&xdev));
        assert!(!is_cross_device(&other));
        assert_eq!(
            std::io::Error::from(std::io::ErrorKind::NotFound).raw_os_error(),
            None,
            "and an error with no code is not one either"
        );
    }

    // ---- deleting ---------------------------------------------------------

    /// Only the named files go, and the positive control is the one beside them.
    #[test]
    fn a_delete_removes_the_named_files_and_leaves_the_others() {
        let root = scratch("delete-one");
        let current = holding(&root, "now", "doomed.mp4", b"x");
        std::fs::write(current.join("kept.mp4"), b"y").expect("a file nobody named");

        let report =
            delete_files(&[current.clone()], &current, &names(&["doomed.mp4"])).expect("a delete");

        let gone = !current.join("doomed.mp4").exists();
        let kept = current.join("kept.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(report.deleted, names(&["doomed.mp4"]));
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(gone);
        assert!(kept, "a file nobody named must still be where it was");
    }

    /// Every copy goes, not just the newest.
    ///
    /// A name in two directories is two files the person said they no longer want.
    /// Deleting one and reporting success would leave the duplicate for them to find
    /// later — and the *deleted* list names it once, because the person asked about a
    /// name and not about a copy.
    #[test]
    fn a_delete_removes_the_name_from_every_known_directory() {
        let root = scratch("delete-both");
        let current = holding(&root, "now", "a.mp4", b"new");
        let older = holding(&root, "before", "a.mp4", b"old");

        let report = delete_files(
            &[current.clone(), older.clone()],
            &current,
            &names(&["a.mp4"]),
        )
        .expect("a delete");

        let gone = !current.join("a.mp4").exists() && !older.join("a.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(report.deleted, names(&["a.mp4"]));
        assert!(gone, "both copies are gone");
    }

    /// A symlink is never deleted, and is reported with its own reason.
    #[test]
    fn a_delete_leaves_a_symlink_alone() {
        use std::os::unix::fs::symlink;

        let root = scratch("delete-symlink");
        let current = holding(&root, "now", "real.mp4", b"x");
        symlink(current.join("real.mp4"), current.join("linked.mp4")).expect("a link");

        let report =
            delete_files(&[current.clone()], &current, &names(&["linked.mp4"])).expect("a delete");

        let link_still_there = current.join("linked.mp4").exists();
        let target_still_there = current.join("real.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(report.deleted.is_empty());
        assert_eq!(kept_reasons(&report), vec!["symlink"]);
        assert!(link_still_there, "the link is not the download");
        assert!(
            target_still_there,
            "and nothing followed it to delete what it points at"
        );
    }

    /// A name that is nowhere is missing — and never a deletion that "worked".
    #[test]
    fn a_delete_of_a_name_that_is_nowhere_says_missing() {
        let root = scratch("delete-missing");
        let current = empty(&root, "now");

        let report =
            delete_files(&[current.clone()], &current, &names(&["gone.mp4"])).expect("a delete");

        std::fs::remove_dir_all(&root).ok();

        assert!(report.deleted.is_empty());
        assert_eq!(report.missing, names(&["gone.mp4"]));
    }

    /// Deleting is not refused by an unreadable **target** — unlike moving.
    ///
    /// The asymmetry is deliberate: a deletion does not write into the target, so a
    /// target nobody can list is one unreadable place among the others. The name is
    /// still a failure for the directory nobody could look in, and the copy in the
    /// readable directory still goes.
    #[test]
    fn a_target_that_cannot_be_read_does_not_refuse_a_delete() {
        let root = scratch("delete-target-locked");
        let target = locked(&root, "now");
        let older = holding(&root, "before", "a.mp4", b"x");

        let report = delete_files(
            &[target.clone(), older.clone()],
            &target,
            &names(&["a.mp4"]),
        )
        .expect("a delete is not refused by this");

        let older_gone = !older.join("a.mp4").exists();
        unlock(&target);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(report.deleted, names(&["a.mp4"]), "the readable copy went");
        assert!(older_gone);
    }

    /// A path-shaped name is refused before anything is deleted.
    #[test]
    fn a_path_shaped_name_refuses_the_delete_before_anything_is_removed() {
        let root = scratch("delete-name");
        let current = holding(&root, "now", "a.mp4", b"x");

        let report = delete_files(&[current.clone()], &current, &names(&["../now/a.mp4"]));

        let still_there = current.join("a.mp4").exists();
        std::fs::remove_dir_all(&root).ok();

        assert!(report.expect_err("refused").contains("不是一个文件名"));
        assert!(
            still_there,
            "and nothing was deleted on the way to the refusal"
        );
    }

    // ---- the vocabularies the page reads ----------------------------------

    /// The tokens the page renders, pinned.
    ///
    /// `local-settings-view.js`'s `KEPT_REASON_LABELS` has an entry per spelling
    /// below; a token that drifted would render as the raw string, which reads to an
    /// operator like a bug rather than like an explanation.
    #[test]
    fn the_reason_tokens_are_the_pages_vocabulary() {
        let reasons = [
            KeptReason::SameDirectory,
            KeptReason::TargetExists,
            KeptReason::NotAFile(NotAFile::Symlink),
            KeptReason::NotAFile(NotAFile::Other),
        ];
        let spellings: Vec<&str> = reasons.iter().map(|reason| reason.as_str()).collect();

        assert_eq!(
            spellings,
            [
                "same_directory",
                "target_exists",
                "symlink",
                "not_a_regular_file"
            ]
        );
        assert_eq!(spellings.len(), 4, "the labels the page knows");
    }
}
