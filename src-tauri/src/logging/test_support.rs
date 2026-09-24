//! Test-only plumbing for the modules that have to read what the sink wrote.
//!
//! Three modules now assert on the file the real subscriber produces — the
//! backend itself (T-13), the sidecar drain (T-16) and the Agent commands
//! (T-16). Each needs the same four things: a scratch directory of its own, an
//! options value pointing at it, a clock that does not move so a stamp can be
//! asserted to the second, and one definition of "what is in the file". Four
//! copies of `written` would be four answers to "did the record arrive", which
//! is the question these tests exist to answer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::TimeZone;
use tracing::subscriber::Subscriber;

use crate::config::Environment;
use crate::logging::backend::{assemble, Clock, Options};
use crate::logging::rolling;
use crate::logging::targets::Levels;

/// A clock that does not move, so a stamp can be asserted to the second.
pub(crate) struct Fixed(pub(crate) SystemTime);

impl Clock for Fixed {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// The local time 2026-09-24 10:11:12, so an expected line can be written out in
/// full.
///
/// Built from local fields rather than as a fixed UTC second, because a stamp is
/// now local: a `Fixed` holding `UNIX_EPOCH + n` would render differently on
/// every machine and the expected line would have to carry this machine's offset
/// (CHG-058 D-09). The instant is the same fact either way — what changes is that
/// the *assertion* is now portable.
pub(crate) fn clock() -> Arc<dyn Clock + Send + Sync> {
    let when = chrono::Local
        .with_ymd_and_hms(2026, 9, 24, 10, 11, 12)
        .single()
        .expect("an unambiguous local time");
    Arc::new(Fixed(when.into()))
}

/// A path of this test's own, removed when the guard drops.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub(crate) fn scratch(label: &str) -> Scratch {
    let path = std::env::temp_dir().join(format!(
        "wt-media-backend-{}-{}-{}",
        label,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::remove_dir_all(&path).ok();
    Scratch(path)
}

/// A subscriber's options at the given levels, no secrets, one file sink.
pub(crate) fn options_at(levels: Levels, directory: Option<&Path>) -> Options {
    Options {
        levels,
        secrets: Vec::new(),
        directory: directory.map(Path::to_path_buf),
        limits: rolling::Limits::SHIPPED,
        clock: clock(),
    }
}

/// The development subscriber's levels, no secrets, one file sink.
pub(crate) fn options(directory: Option<&Path>) -> Options {
    options_at(Levels::shipped(Environment::Development), directory)
}

/// Everything the file layer has written, in order. The writer flushes per
/// record, so the file is readable while the subscriber is still alive. A
/// directory that was never created reads as no records, which is what a
/// launch that logged nothing produces.
///
/// **What "in order" means now that the file name is stable.** The live file is
/// `desktop.log` and the archives are `desktop.log.<YYYY-MM-DD-HH>`, so the
/// live file is the one `file-rotate` is currently appending to and the
/// archives are the hours that ended — oldest first, and the live file last.
/// The old filter (`extension() == "log"`) happened to work, but for the wrong
/// reason: it was written when every file in the family was a `.log` and the
/// union of them was what a test wanted. It would now silently *exclude* the
/// archives, and a test that ran across a real hour boundary would lose the
/// records written before it and fail somewhere unrelated. Named explicitly
/// instead, so the archive case is at least handled the same way it was.
pub(crate) fn written(directory: &Path) -> String {
    let mut text = String::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return text;
    };
    let prefix = format!("{}.", rolling::LOG_FILE_NAME);
    let mut archives: Vec<PathBuf> = Vec::new();
    let mut live: Option<PathBuf> = None;
    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        if name == rolling::LOG_FILE_NAME {
            live = Some(path);
        } else if name.starts_with(&prefix) {
            archives.push(path);
        }
    }
    archives.sort();
    for path in archives.iter().chain(live.iter()) {
        text.push_str(&std::fs::read_to_string(path).expect("the log file"));
    }
    text
}

/// A scratch directory and a subscriber writing into it at the given levels.
///
/// The directory has to outlive the reads, so it comes back with the
/// subscriber rather than being created inside the closure.
pub(crate) fn capture_at(levels: Levels, label: &str) -> (Scratch, impl Subscriber + Send + Sync) {
    let directory = scratch(label);
    let subscriber = assemble(options_at(levels, Some(&directory.0)));
    (directory, subscriber)
}

/// The same, at the development levels most tests want.
pub(crate) fn capture(label: &str) -> (Scratch, impl Subscriber + Send + Sync) {
    capture_at(Levels::shipped(Environment::Development), label)
}
