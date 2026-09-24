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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::subscriber::Subscriber;

use crate::config::Environment;
use crate::logging::backend::{assemble, Options};
use crate::logging::rolling;
use crate::logging::targets::Levels;

/// A clock that does not move, so a stamp can be asserted to the second.
pub(crate) struct Fixed(pub(crate) SystemTime);

impl rolling::Clock for Fixed {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// 2026-09-24T10:11:12Z, so an expected line can be written out in full.
pub(crate) fn clock() -> Arc<dyn rolling::Clock + Send + Sync> {
    let date = rolling::Date::from_ymd(2026, 9, 24).expect("a real date");
    let seconds = date.days() as u64 * 86_400 + 10 * 3_600 + 11 * 60 + 12;
    Arc::new(Fixed(UNIX_EPOCH + Duration::from_secs(seconds)))
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
pub(crate) fn written(directory: &Path) -> String {
    let mut text = String::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return text;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|kind| kind == "log"))
        .collect();
    paths.sort();
    for path in paths {
        text.push_str(&std::fs::read_to_string(&path).expect("the log file"));
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
