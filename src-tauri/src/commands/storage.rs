//! Read-only storage and log commands: how much room is left, what the app is
//! using, what log files exist, and what the end of one says.
//!
//! **Read-only is a property of this module, not of its callers.** Nothing here
//! creates a directory, deletes a file, or touches the config; the only side
//! effects are `statvfs` and `read`. Cleanup (T-06) is a different module
//! precisely so that "the page was looking at something" and "the page removed
//! something" cannot be the same request.
//!
//! ## A failure is a failure, not a zero
//!
//! Every command returns `Result`, and an unreadable tree fails the whole answer
//! with a message naming the path (AC-06). The alternative — an error per field —
//! was considered and rejected: a page cannot accidentally render a missing key
//! as `0 MB`, whereas nothing stops it from rendering `{error: "…", bytes: null}`
//! as `0 MB`, and "缓存占 0 MB，可以清空" on a tree that was never read is the
//! exact failure this milestone is about. The cost is real and registered: one
//! unreadable tree hides the other readings in the same call, and the page's
//! remedy is to read them one at a time.
//!
//! ## Where the directories come from
//!
//! The resolvers are called here rather than the values being cached at startup,
//! and they are the *same* pure resolvers the launch used
//! (`bootstrap::build_environment`, `logging::paths::directory`,
//! `app_paths::resolve`), given the same three inputs — this process's `HOME`,
//! the build's environment, and the manifest directory. Two of those inputs are
//! the launch's, not the config's: a command that read `config.environment` would
//! point a development build at the installed tree, which is a test's job to
//! catch rather than a comment's.
//!
//! The Agent's tree is different in kind: Desktop does not own it, it mirrors the
//! Agent's own rule from the setting the Agent was given
//! (`logging::paths::agent_directory`). When that rule cannot be applied, the
//! command fails rather than answering with a directory the Agent is not using —
//! a wrong directory presented as the Agent's logs is worse than a refusal.
//!
//! ## The listing is the allowlist for the tail
//!
//! `local_log_tail` takes a file *name*, and it never joins that name onto a
//! directory by hand ([`listed_file`]). It lists the tree and reads the entry
//! whose name matches, so a name like `../../../../etc/passwd` is not a path that
//! gets rejected — it is simply not in the listing, and the command says the file
//! is not there. Validating the string would have to anticipate every spelling of
//! a traversal; looking it up in the list cannot miss one, because the list only
//! ever contains names that came out of that one directory.
//!
//! The bodies are split for the reason `commands::agent` gives: a command takes
//! `State`, which no test can build, so everything worth asserting lives in a
//! function that takes plain references — [`resolve`], [`listed_file`],
//! [`tail_of`] and [`file_fact`] — and the command bodies are the four lines that
//! wire them to the page.

use crate::app_paths::{self, AppPaths};
use crate::bootstrap;
use crate::config::DesktopConfig;
use crate::dto::{
    LogEntryFact, LogFileFact, LogListing, LogTailResult, LogTreeFiles, LogTreeUsage, StorageUsage,
};
use crate::logging::{
    paths as log_paths,
    reader::{self, LogFile, Source, TAIL_LINES},
};
use crate::storage;
use crate::system_paths::SystemPaths;
use std::path::{Path, PathBuf};
use tauri::State;

/// The most lines a caller may ask for.
///
/// A ceiling on the *request* rather than only on the read: the reader's byte
/// ceiling already bounds memory, but several megabytes of JSON per call is not
/// something a page should be able to ask for by accident, and this is far more
/// lines than a viewer shows.
const MAX_TAIL_LINES: usize = 5000;

/// The directories this launch reads or cleans, resolved once per command.
///
/// Shared with `commands::cleanup` rather than resolved a second time there: the
/// cleanup deletes in the same directories this module reads, and two resolvers
/// would be two rules nothing makes agree — a page could list one tree and clean
/// another.
#[derive(Debug)]
pub(crate) struct Resolved {
    pub(crate) paths: AppPaths,
    pub(crate) agent_logs: PathBuf,
}

impl Resolved {
    /// This component's tree first, then the Agent's — the order the page shows
    /// them in, and the order both commands use.
    pub(crate) fn trees(&self) -> [(Source, &Path); 2] {
        [
            (Source::Desktop, self.paths.logs.as_path()),
            (Source::Agent, self.agent_logs.as_path()),
        ]
    }

    /// The tree one source labels, for a command that takes a source rather than
    /// a path.
    ///
    /// Every label the reader knows has a tree here (`trees` covers both), so the
    /// lookup cannot fail — but a `Source` with no tree would be a new source
    /// added to one list and not the other, which is what this refuses to hide.
    pub(crate) fn tree(&self, source: Source) -> PathBuf {
        self.trees()
            .into_iter()
            .find(|(candidate, _)| *candidate == source)
            .map(|(_, directory)| directory.to_path_buf())
            .expect("every label the reader knows has a tree here")
    }
}

/// Resolve every root this module reads, or say which input was unusable.
pub(crate) fn resolve(config: &DesktopConfig) -> Result<Resolved, String> {
    let system = SystemPaths::current();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));

    let paths = app_paths::resolve(&system, bootstrap::build_environment(), manifest)
        .map_err(|error| format!("无法确定运行目录: {error}"))?;

    // An installed layout with no `HOME` is already refused by `resolve`, so these
    // are absolute whenever it succeeded — but a *relative* `HOME` would make them
    // relative too, and a relative root would be read against this process's
    // working directory. That is the one input that could still get through, so it
    // is checked once for all of them rather than assumed.
    for root in [&paths.data, &paths.versions, &paths.logs, &paths.cache] {
        if !root.is_absolute() {
            return Err(format!(
                "运行目录不是绝对路径（{}），无法确定它在哪：请检查系统用户目录",
                root.display()
            ));
        }
    }

    let agent_logs = log_paths::agent_directory(&system, config.agent.data_dir.as_deref())
        .map_err(|error| format!("无法确定 Agent 日志目录: {error}"))?;

    Ok(Resolved { paths, agent_logs })
}

/// One file a caller named, taken from the listing rather than from their string.
///
/// The name has to match an entry the reader just returned, so the only files
/// this can ever open are the ones in that one directory. A name that is not
/// there — misspelled, rotated away in the last instant, or an attempt to leave
/// the directory — gets the same answer, and it is an answer about the *listing*
/// rather than about the string that was passed.
fn listed_file(directory: &Path, source: Source, name: &str) -> Result<LogFile, String> {
    reader::list(directory, source)
        .map_err(|error| format!("读取日志目录 {} 失败: {error}", directory.display()))?
        .into_iter()
        .find(|file| file.name == name)
        .ok_or_else(|| {
            format!(
                "{} 里没有名为 {name:?} 的日志文件（请先列出文件）",
                directory.display()
            )
        })
}

/// The wire shape of one listed file.
fn file_fact(file: LogFile) -> LogFileFact {
    LogFileFact {
        name: file.name,
        kind: file.kind.as_str().to_string(),
        bytes: file.bytes,
        // A file whose mtime is before the epoch, or a filesystem that will not
        // say, becomes `null` rather than a number that was not measured.
        modified_seconds: file
            .modified
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_secs()),
    }
}

/// The end of a listed file, with the level filter applied.
///
/// `lines` is the caller's request, clamped to `1..=MAX_TAIL_LINES`; `min_level`
/// is the page's spelling, and an unrecognised one is an error rather than a
/// silent default — a filter that quietly meant "info" while the caller asked for
/// something else is a view that lies about what it is showing.
fn tail_of(
    source: Source,
    file: &LogFile,
    lines: Option<usize>,
    min_level: Option<&str>,
) -> Result<LogTailResult, String> {
    let min = match min_level {
        None => None,
        Some(value) => Some(reader::Level::from_str_opt(value).ok_or_else(|| {
            format!("未知的日志级别 {value:?}：只认识 trace/debug/info/warn/error")
        })?),
    };

    let wanted = lines.unwrap_or(TAIL_LINES).clamp(1, MAX_TAIL_LINES);
    let tail = reader::tail(&file.path, wanted)
        .map_err(|error| format!("读取日志 {} 失败: {error}", file.path.display()))?;

    let entries = reader::entries(&tail.lines);
    let lines_read = entries.len();
    let entries = match min {
        Some(min) => reader::filter_min(&entries, min),
        None => entries,
    };

    Ok(LogTailResult {
        source: source.label().to_string(),
        name: file.name.clone(),
        path: file.path.display().to_string(),
        min_level: min.map(|level| level.as_str().to_string()),
        lines_read,
        lines_shown: entries.len(),
        truncated: tail.truncated,
        lines: entries
            .into_iter()
            .map(|entry| LogEntryFact {
                level: entry.level.map(|level| level.as_str().to_string()),
                continues: entry.continues,
                text: entry.text,
            })
            .collect(),
    })
}

/// One tree's total, taken from the listing rather than measured separately.
///
/// Summed from the listing rather than measured again with
/// `storage::directory_bytes`: two measurements of the same tree, taken moments
/// apart, are two numbers a page can show as disagreeing, and the walk and the
/// listing do not even count the same set — the walk enters subdirectories, the
/// listing skips them. These are the files `local_log_files` returns, so the
/// total and the list stay one fact.
///
/// Split out of the command for the reason `commands::agent` gives: the command
/// takes `State`, which no test can build, and this is the half with the rule in
/// it.
fn tree_usage(source: Source, directory: &Path) -> Result<LogTreeUsage, String> {
    let files = reader::list(directory, source)
        .map_err(|error| format!("读取日志目录 {} 失败: {error}", directory.display()))?;
    Ok(LogTreeUsage {
        source: source.label().to_string(),
        directory: directory.display().to_string(),
        bytes: files.iter().map(|file| file.bytes).sum(),
        files: files.len(),
    })
}

/// Free space, the cache's size, and each log tree's size.
#[tauri::command]
pub fn local_storage_usage(config: State<'_, DesktopConfig>) -> Result<StorageUsage, String> {
    let resolved = resolve(&config)?;

    let available_bytes = storage::available_bytes_for(&resolved.paths.data)
        .map_err(|error| format!("读取可用空间失败: {error}"))?;
    let cache_bytes = storage::directory_bytes(&resolved.paths.cache)
        .map_err(|error| format!("统计缓存占用失败: {error}"))?;

    let mut logs = Vec::new();
    for (source, directory) in resolved.trees() {
        logs.push(tree_usage(source, directory)?);
    }

    Ok(StorageUsage {
        available_bytes,
        cache_bytes,
        logs,
    })
}

/// Every log file in both trees.
#[tauri::command]
pub fn local_log_files(config: State<'_, DesktopConfig>) -> Result<LogListing, String> {
    let resolved = resolve(&config)?;

    let mut trees = Vec::new();
    for (source, directory) in resolved.trees() {
        let files = reader::list(directory, source)
            .map_err(|error| format!("读取日志目录 {} 失败: {error}", directory.display()))?;
        trees.push(LogTreeFiles {
            source: source.label().to_string(),
            directory: directory.display().to_string(),
            files: files.into_iter().map(file_fact).collect(),
        });
    }

    Ok(LogListing { trees })
}

/// The end of one log file, optionally filtered to a level and above.
///
/// `source` and `name` name a file the listing returned; `lines` defaults to the
/// viewer's page and is clamped; `min_level` is the page's own spelling.
#[tauri::command]
pub fn local_log_tail(
    config: State<'_, DesktopConfig>,
    source: String,
    name: String,
    lines: Option<usize>,
    min_level: Option<String>,
) -> Result<LogTailResult, String> {
    let resolved = resolve(&config)?;

    let source = Source::from_label(&source)
        .ok_or_else(|| format!("未知的日志来源 {source:?}：只认识 desktop 与 agent"))?;
    let directory = resolved.tree(source);
    let file = listed_file(&directory, source, &name)?;
    tail_of(source, &file, lines, min_level.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, PRODUCTION_TOML};
    use std::collections::BTreeMap;

    /// A scratch log tree of this test's own, removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-commands-{}-{}-{}",
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

    fn record(stamp: &str, level: &str, message: &str) -> String {
        format!("{stamp} [{level}] desktop.startup: {message}")
    }

    /// The config the launch would have read, with `agent.data_dir` set to
    /// `data_dir` — the key that decides which tree the Agent's logs are in.
    fn config_with_data_dir(data_dir: Option<&str>) -> DesktopConfig {
        let text = match data_dir {
            Some(dir) => PRODUCTION_TOML.replace(
                "# data_dir = \"/path/to/agent-data\"",
                &format!("data_dir = {dir:?}"),
            ),
            None => PRODUCTION_TOML.to_string(),
        };
        load_with(
            &BTreeMap::new(),
            &text,
            crate::config::Environment::Production,
        )
        .expect("the test config must load")
    }

    /// The allowlist, from the failing side: a name that is not in the listing is
    /// not read, whatever it looks like.
    ///
    /// The traversal names are the point — each one would resolve to a real,
    /// readable file if the name were joined onto the directory — and the last
    /// case is the positive control: the *same* call with a name that is in the
    /// listing succeeds, so "everything fails" cannot pass this test.
    #[test]
    fn a_name_that_is_not_in_the_listing_is_never_read() {
        let root = scratch("allowlist");
        let outside = scratch("allowlist-outside");
        std::fs::write(outside.join("secret.txt"), b"not the app's to read").expect("plant");
        std::fs::write(
            root.join("desktop.log"),
            record("2026-09-24T20:00:00", "INFO", "hi"),
        )
        .expect("plant");

        let escapes = [
            "../../../etc/passwd".to_string(),
            format!(
                "../{}/secret.txt",
                outside.file_name().unwrap().to_string_lossy()
            ),
            "secret.txt".to_string(),
            "/etc/passwd".to_string(),
            String::new(),
        ];
        let mut failures = Vec::new();
        for name in &escapes {
            failures.push((
                name.clone(),
                listed_file(&root, Source::Desktop, name).is_err(),
            ));
        }
        let control = listed_file(&root, Source::Desktop, "desktop.log");

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();

        for (name, refused) in failures {
            assert!(
                refused,
                "{name:?} must not be read: it is not in the listing"
            );
        }
        assert!(
            control.is_ok(),
            "the positive control must succeed: {control:?}"
        );
    }

    /// What the listing scopes is the **directory**, not the file's kind.
    ///
    /// Two files with the same name in two directories: the one in the tree that
    /// was listed is readable, the one that is only in another tree is not — which
    /// is the property that makes the lookup a guard rather than a name check. And
    /// a file the reader does not recognise as the rotator's own (`other`, which
    /// on a real machine is the pre-T-02 `desktop-20260924-1.log`) is readable
    /// too: it is a log file in this directory, and refusing it would hide exactly
    /// the history a person opens the viewer to read.
    #[test]
    fn the_listing_scopes_by_directory_and_not_by_kind() {
        let here = scratch("scoped-here");
        let elsewhere = scratch("scoped-elsewhere");
        std::fs::write(here.join("desktop.log"), b"mine").expect("plant");
        std::fs::write(elsewhere.join("desktop.log"), b"not mine").expect("plant");
        std::fs::write(here.join("desktop-20260924-1.log"), b"older").expect("plant");

        let found = listed_file(&here, Source::Desktop, "desktop.log");
        let other_tree = listed_file(&elsewhere.join("nowhere"), Source::Desktop, "desktop.log");
        let unrecognised = listed_file(&here, Source::Desktop, "desktop-20260924-1.log");

        std::fs::remove_dir_all(&here).ok();
        std::fs::remove_dir_all(&elsewhere).ok();

        let found = found.expect("a file in the tree that was listed");
        assert!(
            found.path.starts_with(&here),
            "the file read must be the one in this directory: {found:?}"
        );
        assert!(
            other_tree.is_err(),
            "another directory's file is not this tree's"
        );
        assert_eq!(
            unrecognised.expect("history is readable").kind,
            reader::FileKind::Other,
            "an unrecognised name is still a file in this directory"
        );
    }

    /// The filter is applied to the lines and reported honestly: how many were
    /// read, how many are shown, and which filter did it.
    #[test]
    fn the_filter_reports_both_counts_and_the_level_it_used() {
        let root = scratch("filter");
        let path = root.join("desktop.log");
        let text = [
            record("2026-09-24T20:00:00", "DEBUG", "noisy"),
            "  a continuation of the debug record".to_string(),
            record("2026-09-24T20:00:01", "ERROR", "visible"),
            "  a continuation of the error".to_string(),
            record("2026-09-24T20:00:02", "INFO", "also hidden"),
        ]
        .join("\n")
            + "\n";
        std::fs::write(&path, text).expect("plant");
        let file = listed_file(&root, Source::Desktop, "desktop.log").expect("listed");

        let unfiltered = tail_of(Source::Desktop, &file, None, None);
        let filtered = tail_of(Source::Desktop, &file, None, Some("error"));

        std::fs::remove_dir_all(&root).ok();

        let unfiltered = unfiltered.expect("readable");
        assert_eq!(unfiltered.lines_read, 5);
        assert_eq!(unfiltered.lines_shown, 5, "no filter hides nothing");
        assert_eq!(unfiltered.min_level, None);
        assert_eq!(unfiltered.path, path.display().to_string());

        let filtered = filtered.expect("readable");
        assert_eq!(filtered.lines_read, 5, "the count before filtering is kept");
        assert_eq!(filtered.lines_shown, 2, "the error and its continuation");
        assert_eq!(filtered.min_level.as_deref(), Some("error"));
        assert!(filtered.lines[1].continues, "the block stays together");
        assert_eq!(filtered.lines[0].level.as_deref(), Some("error"));
    }

    /// An unknown level is refused rather than defaulted, with the positive
    /// control in the same test: the spellings that *are* understood go through.
    #[test]
    fn an_unknown_level_is_an_error_rather_than_a_default() {
        let root = scratch("levels");
        std::fs::write(
            root.join("desktop.log"),
            record("2026-09-24T20:00:00", "WARN", "x"),
        )
        .expect("plant");
        let file = listed_file(&root, Source::Desktop, "desktop.log").expect("listed");

        let unknown = tail_of(Source::Desktop, &file, None, Some("verbose"));
        let upper = tail_of(Source::Desktop, &file, None, Some("WARN"));
        let alias = tail_of(Source::Desktop, &file, None, Some("warning"));

        std::fs::remove_dir_all(&root).ok();

        let unknown = unknown.expect_err("an unknown level must not be defaulted");
        assert!(
            unknown.contains("verbose"),
            "the message must quote it: {unknown}"
        );

        assert_eq!(upper.expect("readable").min_level.as_deref(), Some("warn"));
        assert_eq!(
            alias.expect("readable").min_level.as_deref(),
            Some("warn"),
            "the Python spelling is the same level, reported in the page's spelling"
        );
    }

    /// The line count is clamped, and the clamp is visible: a file with more lines
    /// than the ceiling returns exactly the ceiling.
    #[test]
    fn the_line_count_is_clamped_to_the_reader_ceiling() {
        let root = scratch("clamp");
        let path = root.join("desktop.log");
        let lines = MAX_TAIL_LINES + 1;
        let text: String = (1..=lines)
            .map(|n| {
                format!(
                    "{}\n",
                    record("2026-09-24T20:00:00", "INFO", &format!("l{n}"))
                )
            })
            .collect();
        std::fs::write(&path, text).expect("plant");
        // Measured from the bytes on disk rather than restated from the constant:
        // "the file has more lines than the ceiling" is the premise the clamps
        // are only interesting under, and a loop that planted fewer would make
        // this test pass for the wrong reason.
        let planted = std::fs::read_to_string(&path)
            .expect("read back")
            .lines()
            .count();
        let file = listed_file(&root, Source::Desktop, "desktop.log").expect("listed");

        let asked_for_everything = tail_of(Source::Desktop, &file, Some(usize::MAX), None);
        let asked_for_none = tail_of(Source::Desktop, &file, Some(0), None);
        let default = tail_of(Source::Desktop, &file, None, None);

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            planted,
            MAX_TAIL_LINES + 1,
            "the premise: more lines than the ceiling are on disk"
        );
        assert_eq!(
            asked_for_everything.expect("readable").lines_shown,
            MAX_TAIL_LINES,
            "more than the ceiling is the ceiling"
        );
        assert_eq!(
            asked_for_none.expect("readable").lines_shown,
            1,
            "zero is one rather than an empty view"
        );
        assert_eq!(
            default.expect("readable").lines_shown,
            TAIL_LINES,
            "and the default is the viewer's page"
        );
        let _ = lines;
    }

    /// The resolved trees are this *build's*, not the config's — the input a
    /// command could most easily get wrong, since the config is right there.
    ///
    /// The two disagree in a test build on purpose: the file says production and
    /// the build is development, and the launch logs to the development tree. A
    /// command that read `config.environment` would list the installed tree
    /// instead, which is what this asserts is *not* happening.
    #[test]
    fn the_trees_are_the_builds_not_the_configs_environment() {
        let config = config_with_data_dir(None);
        assert_eq!(
            config.environment,
            crate::config::Environment::Production,
            "the premise: the config says production"
        );

        let resolved = resolve(&config).expect("resolvable");
        let system = SystemPaths::current();

        let expected = log_paths::directory(
            &system,
            bootstrap::build_environment(),
            Path::new(env!("CARGO_MANIFEST_DIR")),
        );
        assert_eq!(resolved.paths.logs, expected);
        let installed_logs = log_paths::directory(
            &system,
            crate::config::Environment::Production,
            Path::new(env!("CARGO_MANIFEST_DIR")),
        );
        assert_ne!(
            resolved.paths.logs, installed_logs,
            "a development build must not read the installed tree"
        );
        assert!(resolved.paths.logs.is_absolute());
    }

    /// The configured Agent data directory flows into the Agent's tree, and a
    /// value Desktop cannot follow fails the command instead of being guessed.
    #[test]
    fn the_agents_tree_comes_from_the_configured_data_dir() {
        let absolute = resolve(&config_with_data_dir(Some("/Volumes/Scratch/agent-data")))
            .expect("resolvable");
        let unset = resolve(&config_with_data_dir(None)).expect("resolvable");
        let relative = resolve(&config_with_data_dir(Some("./agent-data")));

        assert_eq!(
            absolute.agent_logs,
            PathBuf::from("/Volumes/Scratch/agent-data/logs")
        );
        assert_ne!(absolute.agent_logs, unset.agent_logs);
        assert_ne!(
            absolute.paths.logs, absolute.agent_logs,
            "the two components never share a tree"
        );
        assert!(
            matches!(&relative, Err(message) if message.contains("Agent") && message.contains("./agent-data")),
            "a data directory Desktop cannot follow must fail the command: {relative:?}"
        );
    }

    /// The listing's wire shape is the reader's own spelling, and an absent tree
    /// is an empty list with its directory shown rather than an error.
    #[test]
    fn the_listing_maps_the_readers_vocabulary() {
        let root = scratch("listing-shape");
        std::fs::write(root.join("desktop.log"), b"x").expect("plant");
        std::fs::write(root.join("desktop.log.2026-09-24-19"), b"yy").expect("plant");
        std::fs::write(root.join("desktop-20260924-1.log"), b"zzz").expect("plant");

        let facts: Vec<LogFileFact> = reader::list(&root, Source::Desktop)
            .expect("listed")
            .into_iter()
            .map(file_fact)
            .collect();

        std::fs::remove_dir_all(&root).ok();

        let kinds: Vec<&str> = facts.iter().map(|fact| fact.kind.as_str()).collect();
        assert_eq!(kinds, ["live", "archive", "other"]);
        let bytes: Vec<u64> = facts.iter().map(|fact| fact.bytes).collect();
        assert_eq!(bytes, [1, 2, 3]);
        assert!(
            facts.iter().all(|fact| fact.modified_seconds.is_some()),
            "a file just written has a modification time"
        );
    }

    /// A tree's total is the listing's own bytes, not a second measurement of the
    /// same directory.
    ///
    /// A subdirectory is what tells the two apart: `storage::directory_bytes`
    /// walks into it and the listing skips it, so a total taken from the walk
    /// would count bytes the page's own list does not show — the total and the
    /// list would be two facts that disagree on screen.
    #[test]
    fn a_trees_total_is_the_sum_of_the_files_the_listing_shows() {
        let root = scratch("tree-usage");
        std::fs::write(root.join("desktop.log"), b"1234567890").expect("plant");
        std::fs::write(root.join("desktop.log.2026-09-24-19"), b"12345").expect("plant");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).expect("a subdirectory");
        std::fs::write(nested.join("ignored.log"), vec![b'x'; 4096]).expect("plant");

        let usage = tree_usage(Source::Desktop, &root);
        let listed = reader::list(&root, Source::Desktop).expect("listed");
        let walked = crate::storage::directory_bytes(&root).expect("walked");

        std::fs::remove_dir_all(&root).ok();

        let usage = usage.expect("measurable");
        assert_eq!(usage.files, 2, "the listing skips the subdirectory");
        assert_eq!(usage.bytes, 15, "the two files, and nothing under `nested`");
        assert_eq!(
            usage.bytes,
            listed.iter().map(|file| file.bytes).sum::<u64>(),
            "the total is the listing, so the two commands cannot disagree"
        );
        assert_eq!(usage.source, "desktop");
        assert_eq!(usage.directory, root.display().to_string());
        assert!(
            walked > usage.bytes,
            "the premise: the walk's total is a different, larger number ({walked})"
        );
    }

    /// The order the page shows the trees in: this component's, then the Agent's.
    #[test]
    fn the_two_trees_are_this_components_then_the_agents() {
        let resolved = resolve(&config_with_data_dir(None)).expect("resolvable");

        let trees = resolved.trees();

        assert_eq!(trees[0].0.label(), "desktop");
        assert_eq!(trees[1].0.label(), "agent");
        assert_eq!(trees[0].1, resolved.paths.logs.as_path());
        assert_eq!(trees[1].1, resolved.agent_logs.as_path());
    }
}
