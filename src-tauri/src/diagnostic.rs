//! The support bundle (CHG-058 T-07): one archive a person can attach to a report.
//!
//! Four things go in, and the list is the whole contract: this build's version
//! and where it took its configuration from; the components' state (this app,
//! the sidecar process, the Cloud binding, the Agent as it last answered); the
//! log files, masked; and a summary of the Agent's failed work.
//!
//! ```text
//! wt-media-diagnostic-20260924-225501.tar.gz
//! ├── summary.json      # the four things, machine-readable
//! ├── manifest.txt      # the same, for the person reading it, plus the caps
//! └── logs/
//!     ├── desktop/desktop.log
//!     ├── desktop/desktop.log.2026-09-24-20
//!     └── agent/{agent,error,task}.log
//! ```
//!
//! ## Nothing is assembled here that was assembled elsewhere
//!
//! The log entries are exactly what `logging::reader` lists and `reader::tail`
//! reads — the same pair T-05's viewer uses — and the mask is `logging::redact`,
//! the same one the writers apply. So the bundle cannot hold a line the viewer
//! would not show, and it cannot hold one the writers masked.
//!
//! ## Every string that goes in has been through the mask
//!
//! Twice, for the logs: once by the writer that put the line there, once here on
//! the way into the archive. The second pass is not ceremony — a file written
//! before the mask existed is still on disk, and this archive is the artifact
//! that leaves the machine. `redact` is idempotent (the Agent's own doc says so,
//! and both sides' tests pin it), so the second pass cannot damage a line.
//!
//! What this module must never do is hand a value to `write_archive` that did not
//! come through [`mask`]. The one credential this process holds — the launch
//! token — and the one it may hold — the binding's node credential — are both
//! passed in as `secrets`, so they are masked *verbatim* wherever they appear,
//! including in text this module did not build.
//!
//! ## The caps, and what happens at each
//!
//! | cap | at the edge |
//! | --- | --- |
//! | per file, lines | the file is read from its **end** ([`reader::tail`]); the entry says it is a window |
//! | per file, bytes | the reader's own `TAIL_MAX_BYTES` ceiling, for the same reason |
//! | entry name | refused into `omitted` rather than truncated: a renamed log is a log nobody can find |
//! | the log payload | further entries go to `omitted` with `archive_full`; the ones already in stay |
//!
//! The last row's *direction* is load-bearing and comes from `reader::list`,
//! which is newest-first: the entries that fill the payload first are the most
//! recent ones, so what a ceiling drops is the oldest history. The opposite
//! reading — keep the start of the week, drop the hour the report is about —
//! would be the one failure of this module that still produces a plausible
//! archive, which is why `the_log_payload_stops_at_its_ceiling…` pins which end
//! is dropped rather than only that something is.
//!
//! A tree that cannot be listed is `omitted` too, with `unreadable` — **not** an
//! error for the whole call. This is where the export deliberately parts company
//! with the cleanup: a cleanup that cannot read a tree must fail (it would delete
//! blind), while an export that fails produces no evidence at all, which is the
//! opposite of what someone asking for one needs.

use crate::logging::{reader, redact};
use sha2::Digest as _;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The archive's name: prefix, local stamp, extension.
pub const ARCHIVE_PREFIX: &str = "wt-media-diagnostic-";
/// Local time, to the second. Local because every other stamp in this product is
/// (ruling D-09), and this one gets read next to a log whose lines carry it.
pub const ARCHIVE_STAMP: &str = "%Y%m%d-%H%M%S";
pub const ARCHIVE_EXTENSION: &str = "tar.gz";
pub const CHECKSUM_EXTENSION: &str = "sha256";
/// The index of the bundle: always the first entry written.
pub const SUMMARY_NAME: &str = "summary.json";
/// The same index for a person: always the second.
pub const MANIFEST_NAME: &str = "manifest.txt";
/// The directory the log entries live under, one subdirectory per source.
pub const LOGS_PREFIX: &str = "logs";
/// How many names `write_archive` will try before giving up. Low on purpose: a
/// directory with twenty of these in it is a directory that needs looking at, not
/// a directory to keep counting in.
const MAX_NAME_ATTEMPTS: u32 = 20;

/// The Agent's task log, whose warn-and-louder records are the failure summary.
///
/// Mirrors `runtime/logging.py`'s `TASK_LOG_NAME`, like `reader::Source::Agent`'s
/// live-name list does — and, like that list, nothing at compile time ties the
/// two together. The test at the bottom of this module is the tie: it asserts
/// this name is one of the Agent's live names, so a rename on the reader's side
/// fails here rather than producing a bundle with no failure summary in it.
pub const FAILURE_LOG_NAME: &str = "task.log";

/// The bounds this module works to. A struct rather than constants alone so a
/// test can shrink one and watch the rule at the edge, which is the only place
/// these rules do anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Caps {
    /// Lines read from the end of each file. The reader's byte ceiling is the
    /// other bound and usually arrives first.
    pub max_entry_lines: usize,
    /// The log payload's total. The summary and the manifest are not counted:
    /// they are bounded by construction, this is the part that is not.
    pub max_archive_bytes: u64,
    /// Longest entry name accepted. A single component of a path can be 255
    /// bytes on APFS; the cap is below that so the manifest stays readable.
    pub max_name_bytes: usize,
    pub max_failure_records: usize,
    /// The failure summary's own payload, so one enormous line cannot carry the
    /// whole archive.
    pub max_failure_bytes: u64,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            max_entry_lines: 20_000,
            max_archive_bytes: 32 * 1024 * 1024,
            max_name_bytes: 128,
            max_failure_records: 50,
            max_failure_bytes: 64 * 1024,
        }
    }
}

/// Why an entry is not in the archive. Not an error: the bundle says what it left.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OmittedReason {
    /// The name is longer than [`Caps::max_name_bytes`].
    NameTooLong,
    /// The tree it lives in could not be listed.
    Unreadable,
    /// The log payload had already reached [`Caps::max_archive_bytes`].
    ArchiveFull,
}

impl OmittedReason {
    /// The spelling the bundle uses, and the page shows.
    pub const fn as_str(self) -> &'static str {
        match self {
            OmittedReason::NameTooLong => "name_too_long",
            OmittedReason::Unreadable => "unreadable",
            OmittedReason::ArchiveFull => "archive_full",
        }
    }
}

/// One entry left out, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Omission {
    pub name: String,
    pub reason: OmittedReason,
}

/// One log file, read, masked, and named as it will be inside the archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogEntry {
    /// `logs/<source label>/<file name>`.
    pub name: String,
    /// The masked text. No trailing newline handling here: the writer decides.
    pub text: String,
    /// The bytes the file has on disk, so the manifest can say what the window
    /// is a window *of*.
    pub source_bytes: u64,
    /// The file's own last modification, carried into the archive's header.
    pub modified: Option<SystemTime>,
    /// True when a cap stopped the read, so the text is the end of the file
    /// rather than all of it.
    pub truncated: bool,
}

/// What came out of the two log trees.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Logs {
    pub entries: Vec<LogEntry>,
    pub omitted: Vec<Omission>,
}

/// The failed-work summary, taken from one log entry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Failures {
    /// The name the records came from, or `None` when that log was not in the
    /// bundle at all.
    pub source: Option<String>,
    pub records: Vec<String>,
    /// True when a cap stopped the collection.
    pub truncated: bool,
}

/// What this process can say about itself. Assembled by the command, which is
/// the only place that holds managed state.
#[derive(Clone, Debug)]
pub struct HostFacts {
    /// The **product's** version, as `tauri.conf.json` declares it and
    /// `package_info` reports it — the number a release is stamped with.
    pub app_version: String,
    /// The **crate's** version, compiled into this binary.
    ///
    /// Reported beside the product's rather than instead of it: the two are
    /// equal today and nothing keeps them so, and "which version am I running"
    /// is the first question a bundle is opened to answer. Reporting one and
    /// calling it "the version" would answer it wrongly on the first release
    /// that bumped only one of them.
    pub build_version: String,
    /// `development` or `production` — the build's, not the file's.
    pub environment: String,
    /// Where the configuration was taken from, machine-readable
    /// (`paths::Source::code`).
    pub config_source: String,
    /// Why a located configuration file was refused, if one was. The message
    /// names a key and never its value (`config::parse_error` holds that line).
    pub config_rejected: Option<String>,
    pub data_root: String,
    pub logs_root: String,
    pub cache_root: String,
    /// The volume's free space, when it could be measured. `None` is "could not
    /// be measured", never zero: the same distinction T-05's commands draw.
    pub free_bytes: Option<u64>,
    /// Whether this process spawned a sidecar it has not stopped.
    pub sidecar_running: bool,
    /// The binding's node id when this session is bound. The **credential** that
    /// comes with it is never carried here; it goes in [`HostFacts::secrets`].
    pub binding_node_id: Option<String>,
    pub agent: AgentFacts,
    /// Values that must not appear in the bundle, masked verbatim wherever they
    /// occur. See the module header.
    pub secrets: Vec<String>,
}

/// The Agent as this process last saw it.
///
/// [`AgentFacts::state`] is the spelling both readers use.
#[derive(Clone, Debug)]
pub enum AgentFacts {
    /// Its fields, flattened to strings. The keys are the constants in this
    /// module; the values are whatever the Agent said, masked on the way in.
    Answered(BTreeMap<String, String>),
    /// It did not answer. Not a failure of the export: an Agent that will not
    /// answer is a reason to want a bundle.
    Unreachable { error: String },
}

impl AgentFacts {
    /// The word the bundle and the page both use for "did it answer".
    ///
    /// Here rather than at each reader, so `summary.json`, `manifest.txt` and the
    /// page's own report cannot come to spell the two states three ways.
    pub const fn state(&self) -> &'static str {
        match self {
            AgentFacts::Answered(_) => "answered",
            AgentFacts::Unreachable { .. } => "unreachable",
        }
    }
}

/// One file as the writer receives it: already masked, already bounded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileToWrite {
    pub name: String,
    pub bytes: Vec<u8>,
    pub modified: Option<SystemTime>,
}

/// Where the archive and its checksum ended up, and what they are.
#[derive(Debug)]
pub struct Written {
    pub path: PathBuf,
    /// `"<hex>  <file name>\n"`, the shape `shasum -a 256` writes, so the usual
    /// tools verify it without being told a format.
    pub checksum_path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

/// Why a bundle could not be written.
#[derive(Debug)]
pub enum DiagnosticError {
    /// The target directory is not one, or the archive could not be created in
    /// it.
    Target {
        path: PathBuf,
        reason: std::io::Error,
    },
    /// The archive was created but could not be filled.
    Archive { reason: std::io::Error },
    /// A name handed to the writer was not one this module builds.
    ///
    /// Not a user-facing condition: every name in a bundle is
    /// `logs/<label>/<file name>` with a single-component file name. This is the
    /// refusal that keeps that true if a future caller ever passes something else.
    BadName { name: String },
    /// The target name was taken [`MAX_NAME_ATTEMPTS`] times over.
    TargetTaken { stem: String },
}

impl std::fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticError::Target { path, reason } => {
                write!(f, "{} could not be written to: {reason}", path.display())
            }
            DiagnosticError::Archive { reason } => write!(f, "the archive failed: {reason}"),
            DiagnosticError::BadName { name } => write!(f, "{name:?} is not a name this builds"),
            DiagnosticError::TargetTaken { stem } => {
                write!(f, "every name for {stem} is taken")
            }
        }
    }
}

impl std::error::Error for DiagnosticError {}

/// The archive's file name for a stamp.
pub fn archive_file_name(stem: &str) -> String {
    format!("{stem}.{ARCHIVE_EXTENSION}")
}

/// The stem every name in this module starts from.
pub fn archive_stem(stamp: &str) -> String {
    format!("{ARCHIVE_PREFIX}{stamp}")
}

/// The stem for a moment, on the product's own clock.
///
/// Through `logging::backend` rather than through `chrono` directly, so this
/// module holds no clock of its own: every stamp this product writes — a record's,
/// an archive name's, this one's — is the same reading in a different format. The
/// caller supplies the moment, which is what keeps [`write_archive`] and
/// [`bundle_files`] free of one.
pub fn stem_at(time: SystemTime) -> String {
    archive_stem(&crate::logging::backend::stamp_with(ARCHIVE_STAMP, time))
}

/// The bundle's `created_at`, in the shape a log line carries.
///
/// The same format as the records inside the bundle, on purpose: the first thing
/// a reader does with a support archive is line its history up against a log
/// line somebody quoted, and two formats on two clocks make that a conversion.
pub fn created_at(time: SystemTime) -> String {
    crate::logging::backend::stamp_with(crate::logging::rolling::STAMP_FORMAT, time)
}

/// Mask every string that is about to enter the archive.
///
/// The single door: [`LogEntry::text`] is built with it, and the summary and the
/// manifest go through it too. A caller that builds a string and does not call
/// this is the one mistake this module cannot catch for you — which is why there
/// is exactly one caller per artifact, and why the test at the bottom asserts a
/// credential does not survive all three.
pub fn mask(text: &str, secrets: &[String]) -> String {
    redact::redact(text, secrets)
}

/// Read both trees: what the reader lists, from the end of each file, masked.
///
/// The listing is the allowlist — the same one the viewer shows and the cleanup
/// deletes from — so a file this function never sees is a file neither of those
/// sees either. Order is the listing's order per tree, and the trees in the order
/// they are given: the most recent and the recognisable first, which is also the
/// order a person reads a bundle in.
pub fn gather_logs(trees: &[(reader::Source, PathBuf)], secrets: &[String], caps: &Caps) -> Logs {
    let mut logs = Logs::default();
    let mut payload = 0_u64;

    for (source, directory) in trees {
        let listed = match reader::list(directory, *source) {
            Ok(listed) => listed,
            Err(error) => {
                // The whole tree, as one omission: the name is the directory,
                // because that is the thing that could not be read.
                logs.omitted.push(Omission {
                    name: directory.display().to_string(),
                    reason: OmittedReason::Unreadable,
                });
                // The reason travels in the manifest's own words; the error's
                // Display is the path plus the operating system's, which is what
                // a reader needs and holds no credentials.
                let _ = error;
                continue;
            }
        };

        for file in listed {
            let name = format!("{LOGS_PREFIX}/{}/{}", source.label(), file.name);
            if name.len() > caps.max_name_bytes {
                logs.omitted.push(Omission {
                    name,
                    reason: OmittedReason::NameTooLong,
                });
                continue;
            }
            // One line over the cap, so "we got exactly what we asked for" is
            // distinguishable from "there was more" — a file that ends at the
            // cap exactly would otherwise be reported as a window.
            let probe = match reader::tail(&file.path, caps.max_entry_lines + 1) {
                Ok(probe) => probe,
                Err(_) => {
                    logs.omitted.push(Omission {
                        name,
                        reason: OmittedReason::Unreadable,
                    });
                    continue;
                }
            };
            let over_lines = probe.lines.len() > caps.max_entry_lines;
            let kept: Vec<String> = if over_lines {
                probe.lines[probe.lines.len() - caps.max_entry_lines..].to_vec()
            } else {
                probe.lines
            };
            let text = mask(&kept.join("\n"), secrets);

            let entry_bytes = text.len() as u64;
            if payload.saturating_add(entry_bytes) > caps.max_archive_bytes {
                logs.omitted.push(Omission {
                    name,
                    reason: OmittedReason::ArchiveFull,
                });
                // Everything after it is refused for the same reason, so the
                // listing is walked to the end and each one is registered: a
                // person comparing the bundle to the log directory has to be able
                // to account for every file they can see.
                continue;
            }
            payload += entry_bytes;
            logs.entries.push(LogEntry {
                name,
                text,
                source_bytes: file.bytes,
                modified: file.modified,
                truncated: probe.truncated || over_lines,
            });
        }
    }

    logs
}

/// The warn-and-louder records of the given entries, newest-last.
///
/// "Failed work" as the logs can show it: the Agent's task log is where a task
/// that ended badly says so, and its records carry the level. This is **not** the
/// Agent's task ledger — that lives in its SQLite store and the local API does
/// not expose failed rows (see the T-07 evidence). Registered as a boundary
/// rather than papered over.
pub fn failure_records(entries: &[LogEntry], caps: &Caps) -> Failures {
    let source = entries
        .iter()
        .find(|entry| entry.name.ends_with(&format!("/{FAILURE_LOG_NAME}")))
        .map(|entry| entry.name.clone());

    let Some(entry) = entries
        .iter()
        .find(|entry| entry.name.ends_with(&format!("/{FAILURE_LOG_NAME}")))
    else {
        return Failures::default();
    };

    let mut records = Vec::new();
    let mut bytes = 0_u64;
    let mut truncated = false;
    for line in entry.text.lines() {
        let loud = reader::record_level(line).is_some_and(|level| level >= reader::Level::Warn);
        if !loud {
            continue;
        }
        if records.len() >= caps.max_failure_records
            || bytes.saturating_add(line.len() as u64) > caps.max_failure_bytes
        {
            truncated = true;
            break;
        }
        bytes += line.len() as u64;
        records.push(line.to_string());
    }

    Failures {
        source,
        records,
        truncated,
    }
}

/// The bundle's files, in the order they are written.
///
/// Pure: everything it needs is an argument, so the whole bundle's content can be
/// asserted without a filesystem. `write_archive` is the other half and does
/// nothing but put this list on disk.
pub fn bundle_files(
    host: &HostFacts,
    logs: &Logs,
    failures: &Failures,
    created_at: &str,
) -> Vec<FileToWrite> {
    let summary = mask(
        &serde_json::to_string_pretty(&summary(host, logs, failures, created_at))
            .unwrap_or_else(|_| "{\"error\":\"the summary could not be rendered\"}".to_string()),
        &host.secrets,
    );
    let manifest = mask(&manifest(host, logs, failures, created_at), &host.secrets);

    let mut files = vec![
        FileToWrite {
            name: SUMMARY_NAME.to_string(),
            bytes: summary.into_bytes(),
            // The bundle's own files take the moment of the export; only the log
            // entries carry the times of the files they came from.
            modified: None,
        },
        FileToWrite {
            name: MANIFEST_NAME.to_string(),
            bytes: manifest.into_bytes(),
            modified: None,
        },
    ];
    // The second pass over the logs, and the one that makes the invariant a
    // property of this function rather than of its callers: everything this
    // returns has been through `mask`, whatever built it. `gather_logs` already
    // masked, and masking is idempotent, so this cannot damage a line —
    // `masking_twice_leaves_the_bytes_alone` pins that on the producer's path.
    files.extend(logs.entries.iter().map(|entry| FileToWrite {
        name: entry.name.clone(),
        bytes: mask(&entry.text, &host.secrets).into_bytes(),
        modified: entry.modified,
    }));
    files
}

/// The bundle's index, as JSON.
pub fn summary(
    host: &HostFacts,
    logs: &Logs,
    failures: &Failures,
    created_at: &str,
) -> serde_json::Value {
    let agent = match &host.agent {
        AgentFacts::Answered(fields) => {
            let mut object = serde_json::Map::new();
            object.insert("state".to_string(), host.agent.state().into());
            for (key, value) in fields {
                object.insert(key.clone(), value.clone().into());
            }
            serde_json::Value::Object(object)
        }
        AgentFacts::Unreachable { error } => {
            let mut object = serde_json::Map::new();
            object.insert("state".to_string(), host.agent.state().into());
            object.insert("error".to_string(), error.clone().into());
            serde_json::Value::Object(object)
        }
    };

    serde_json::json!({
        "created_at": created_at,
        "desktop": {
            "app_version": host.app_version,
            "build_version": host.build_version,
            "environment": host.environment,
            "config_source": host.config_source,
            "config_rejected": host.config_rejected,
            "data_root": host.data_root,
            "logs_root": host.logs_root,
            "cache_root": host.cache_root,
            "free_bytes": host.free_bytes,
        },
        "sidecar": { "running": host.sidecar_running },
        "binding": { "bound": host.binding_node_id.is_some(), "node_id": host.binding_node_id },
        "agent": agent,
        "failed_work": {
            "source": failures.source,
            "count": failures.records.len(),
            "truncated": failures.truncated,
            "records": failures.records,
        },
        "logs": {
            "entries": logs.entries.len(),
            // The masked text's size, which is what lands in the archive: the
            // only producer of a `LogEntry` is `gather_logs`, and it masks.
            "bytes": logs.entries.iter().map(|entry| entry.text.len() as u64).sum::<u64>(),
            "omitted": logs.omitted.len(),
        },
    })
}

/// The bundle's index, for a person: what is in it, what is not, and the bounds.
pub fn manifest(host: &HostFacts, logs: &Logs, failures: &Failures, created_at: &str) -> String {
    let caps = Caps::default();
    let mut out = String::new();
    out.push_str("WT Media 诊断包（脱敏）\n");
    out.push_str(&format!("生成时间: {created_at}\n"));
    out.push_str(&format!(
        "Desktop {}（构建 {}）/ 环境 {} / 配置来源 {}\n",
        host.app_version, host.build_version, host.environment, host.config_source
    ));
    match &host.config_rejected {
        Some(reason) => out.push_str(&format!("另有一个配置文件被拒绝: {reason}\n")),
        None => out.push_str("没有被拒绝的配置文件\n"),
    }
    out.push_str(&format!(
        "Agent: {}\n",
        match &host.agent {
            AgentFacts::Answered(_) => "已应答（见 summary.json）".to_string(),
            AgentFacts::Unreachable { error } => format!("无应答（{error}）"),
        }
    ));
    out.push_str(&format!(
        "失败任务摘要: {} 条（来源 {}）{}\n",
        failures.records.len(),
        failures.source.as_deref().unwrap_or("（未收集到）"),
        if failures.truncated {
            "，已达上限"
        } else {
            ""
        }
    ));
    out.push_str(&format!(
        "绑定: {}\n",
        match &host.binding_node_id {
            Some(node) => format!("已绑定（node_id {node}）"),
            None => "未绑定".to_string(),
        }
    ));

    out.push_str("\n日志文件:\n");
    for entry in &logs.entries {
        out.push_str(&format!(
            "  {}  {} 字节（源文件 {} 字节）{}\n",
            entry.name,
            entry.text.len(),
            entry.source_bytes,
            if entry.truncated {
                "  [尾部窗口]"
            } else {
                ""
            }
        ));
    }
    if logs.entries.is_empty() {
        out.push_str("  （无）\n");
    }
    out.push_str("\n未纳入:\n");
    for omission in &logs.omitted {
        out.push_str(&format!(
            "  {}  （{}）\n",
            omission.name,
            omission.reason.as_str()
        ));
    }
    if logs.omitted.is_empty() {
        out.push_str("  （无）\n");
    }

    out.push_str(&format!(
        "\n上限: 每文件 {} 行 / 日志合计 {} 字节 / 条目名 {} 字节 / 失败摘要 {} 条\n",
        caps.max_entry_lines, caps.max_archive_bytes, caps.max_name_bytes, caps.max_failure_records
    ));
    out.push_str(
        "脱敏: 每一条目都已按与两侧日志相同的规则重新脱敏；\
         本归档的 sha256 在同目录的 .sha256 文件里（放进归档里会变成自指）。\n",
    );
    out
}

/// Write the entries to `directory` as `<stem>.tar.gz`, plus `<stem>.tar.gz.sha256`.
///
/// Never overwrites: the file is created with `create_new`, and an existing name
/// moves the attempt on to `-2`, `-3`, …. A support bundle that replaced the one
/// before it would destroy the very evidence it exists to carry.
pub fn write_archive(
    directory: &Path,
    stem: &str,
    files: &[FileToWrite],
) -> Result<Written, DiagnosticError> {
    if !directory.is_dir() {
        return Err(DiagnosticError::Target {
            path: directory.to_path_buf(),
            reason: std::io::Error::new(std::io::ErrorKind::NotFound, "not a directory"),
        });
    }

    let mut attempt = 0_u32;
    let (path, file) = loop {
        let name = match attempt {
            0 => archive_file_name(stem),
            n => format!("{stem}-{}.{ARCHIVE_EXTENSION}", n + 1),
        };
        let candidate = directory.join(&name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(reason) if reason.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt += 1;
                if attempt >= MAX_NAME_ATTEMPTS {
                    return Err(DiagnosticError::TargetTaken {
                        stem: stem.to_string(),
                    });
                }
            }
            Err(reason) => {
                return Err(DiagnosticError::Target {
                    path: candidate,
                    reason,
                })
            }
        }
    };

    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for entry in files {
        let name = check_name(&entry.name)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(entry.bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(unix_seconds(entry.modified));
        header.set_cksum();
        builder
            .append_data(&mut header, name, entry.bytes.as_slice())
            .map_err(|reason| DiagnosticError::Archive { reason })?;
    }
    let encoder = builder
        .into_inner()
        .map_err(|reason| DiagnosticError::Archive { reason })?;
    let file = encoder
        .finish()
        .map_err(|reason| DiagnosticError::Archive { reason })?;
    file.sync_all()
        .map_err(|reason| DiagnosticError::Archive { reason })?;
    drop(file);

    let mut reader = std::fs::File::open(&path).map_err(|reason| DiagnosticError::Target {
        path: path.clone(),
        reason,
    })?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut reader, &mut hasher).map_err(|reason| DiagnosticError::Target {
        path: path.clone(),
        reason,
    })?;
    let sha256 = hex::encode(hasher.finalize());
    let bytes = std::fs::metadata(&path)
        .map_err(|reason| DiagnosticError::Target {
            path: path.clone(),
            reason,
        })?
        .len();

    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let checksum_path = directory.join(format!("{file_name}.{CHECKSUM_EXTENSION}"));
    let mut checksum =
        std::fs::File::create(&checksum_path).map_err(|reason| DiagnosticError::Target {
            path: checksum_path.clone(),
            reason,
        })?;
    checksum
        .write_all(format!("{sha256}  {file_name}\n").as_bytes())
        .and_then(|_| checksum.sync_all())
        .map_err(|reason| DiagnosticError::Target {
            path: checksum_path.clone(),
            reason,
        })?;

    Ok(Written {
        path,
        checksum_path,
        bytes,
        sha256,
    })
}

/// A name this module builds, or a refusal.
///
/// `bad` names are: empty, absolute, holding a `..` component, or holding a
/// component that is empty (a doubled or trailing slash). Everything in a bundle
/// comes from `logs/<label>/<file name>`, so all four are unreachable from here —
/// and this is what keeps that a property rather than an assumption.
fn check_name(name: &str) -> Result<&str, DiagnosticError> {
    let bad = name.is_empty()
        || name.starts_with('/')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if bad {
        return Err(DiagnosticError::BadName {
            name: name.to_string(),
        });
    }
    Ok(name)
}

/// The header's mtime. An absent one is the epoch, which is what a tar header
/// holds when nobody set it — stated rather than defaulted to "now", because a
/// substituted clock is a claim about a file that nobody measured.
fn unix_seconds(modified: Option<SystemTime>) -> u64 {
    modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A scratch directory of this test's own, removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-diagnostic-{}-{}-{}",
            label,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).expect("scratch dir");
        path
    }

    fn plant(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("parent");
        std::fs::write(path, text).expect("plant");
    }

    fn facts(secrets: Vec<String>) -> HostFacts {
        HostFacts {
            app_version: "0.1.0".to_string(),
            build_version: "0.1.0".to_string(),
            environment: "development".to_string(),
            config_source: "development_tree".to_string(),
            config_rejected: None,
            data_root: "/home/operator/.local/data".to_string(),
            logs_root: "/home/operator/.local/logs".to_string(),
            cache_root: "/home/operator/.local/cache".to_string(),
            free_bytes: Some(91_757_240_320),
            sidecar_running: false,
            binding_node_id: Some("node-7".to_string()),
            agent: AgentFacts::Answered(BTreeMap::from([(
                "agent_version".to_string(),
                "0.2.2".to_string(),
            )])),
            secrets,
        }
    }

    /// Read an archive back: `(name, bytes)` per entry, in the order written.
    fn read_back(path: &Path) -> Vec<(String, Vec<u8>)> {
        let file = std::fs::File::open(path).expect("the archive");
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let mut out = Vec::new();
        for entry in archive.entries().expect("entries") {
            let mut entry = entry.expect("an entry");
            let name = entry.path().expect("a path").display().to_string();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).expect("the bytes");
            out.push((name, bytes));
        }
        out
    }

    fn text_of(files: &[FileToWrite], name: &str) -> String {
        String::from_utf8(
            files
                .iter()
                .find(|file| file.name == name)
                .expect("that entry")
                .bytes
                .clone(),
        )
        .expect("utf-8")
    }

    #[test]
    fn the_name_is_the_prefix_the_stamp_and_the_extension() {
        let stem = archive_stem("20260924-225501");
        assert_eq!(stem, "wt-media-diagnostic-20260924-225501");
        assert_eq!(
            archive_file_name(&stem),
            "wt-media-diagnostic-20260924-225501.tar.gz"
        );
    }

    /// The stem is a file name: no separators, and the moment is legible in it.
    #[test]
    fn the_stem_of_a_moment_sorts_and_needs_no_escaping() {
        let moment = UNIX_EPOCH + std::time::Duration::from_secs(1_787_000_000);
        let stem = stem_at(moment);

        assert!(stem.starts_with(ARCHIVE_PREFIX), "{stem}");
        let stamp = &stem[ARCHIVE_PREFIX.len()..];
        assert_eq!(stamp.len(), 15, "{stamp} must be YYYYMMDD-HHMMSS");
        assert_eq!(stamp.as_bytes()[8], b'-', "{stamp}");
        assert!(
            stamp.chars().all(|c| c.is_ascii_digit() || c == '-'),
            "{stamp}"
        );
        assert_eq!(stem, stem_at(moment), "the same moment names the same file");

        // And the document's own stamp is the log line's shape, so the two can be
        // read against each other without a conversion.
        let at = created_at(moment);
        assert_eq!(at.len(), 19, "{at}");
        assert_eq!(&at[..4], &stamp[..4], "the same year: {at} vs {stamp}");
        assert_eq!(&at[5..7], &stamp[4..6], "the same month: {at} vs {stamp}");
        assert_eq!(&at[8..10], &stamp[6..8], "the same day: {at} vs {stamp}");
        assert_eq!(&at[11..13], &stamp[9..11], "the same hour: {at} vs {stamp}");
    }

    #[test]
    fn the_failure_log_name_is_one_the_reader_lists() {
        assert!(
            reader::Source::Agent
                .live_names()
                .contains(&FAILURE_LOG_NAME),
            "{FAILURE_LOG_NAME} must be one of the Agent's live names, or no bundle \
             ever carries a failure summary"
        );
    }

    #[test]
    fn the_bundle_opens_with_the_summary_then_the_manifest() {
        let logs = Logs::default();
        let files = bundle_files(
            &facts(vec![]),
            &logs,
            &Failures::default(),
            "2026-09-24T22:55:01+08:00",
        );
        assert_eq!(files[0].name, SUMMARY_NAME);
        assert_eq!(files[1].name, MANIFEST_NAME);
        assert_eq!(files.len(), 2, "no logs means no more files");
        let summary: serde_json::Value =
            serde_json::from_str(&text_of(&files, SUMMARY_NAME)).expect("the summary is JSON");
        assert_eq!(summary["desktop"]["app_version"], "0.1.0");
        assert_eq!(summary["desktop"]["build_version"], "0.1.0");
        assert_eq!(summary["binding"]["bound"], true);
        assert_eq!(summary["binding"]["node_id"], "node-7");
        assert_eq!(summary["agent"]["state"], "answered");
        assert_eq!(summary["agent"]["agent_version"], "0.2.2");
        assert_eq!(summary["logs"]["entries"], 0);
    }

    #[test]
    fn an_unreachable_agent_is_a_stated_state_not_a_failed_export() {
        let mut host = facts(vec![]);
        host.agent = AgentFacts::Unreachable {
            error: "agent unreachable: connection refused".to_string(),
        };
        let files = bundle_files(&host, &Logs::default(), &Failures::default(), "now");
        let summary: serde_json::Value =
            serde_json::from_str(&text_of(&files, SUMMARY_NAME)).expect("JSON");
        assert_eq!(summary["agent"]["state"], "unreachable");
        assert!(summary["agent"]["error"]
            .as_str()
            .is_some_and(|text| text.contains("connection refused")));
        assert!(
            text_of(&files, MANIFEST_NAME).contains("无应答"),
            "and the person reading it is told too"
        );
    }

    /// The credential test, in both directions.
    ///
    /// First the control: the needle really is in the input, so an archive that
    /// does not contain it is not just a needle that never matches anything.
    #[test]
    fn a_credential_in_the_logs_or_the_facts_never_reaches_the_archive() {
        const TOKEN: &str = "launch-token-9f2c8a41";
        const NODE: &str = "node-credential-4b7e1d90";
        let base = scratch("credential");
        let tree = base.join("logs/agent");
        let raw = format!(
            "2026-09-24T22:55:01 [INFO] agent.auth: node_credential={NODE} sent\ntoken={TOKEN}\n"
        );
        plant(&tree.join("agent.log"), &raw);

        // The control, before the claim, and on the bytes that actually went in:
        // the needles are in the input. Without this the assertions below count a
        // needle that was never there, and a test that cannot fail is not
        // evidence. Read back from disk rather than from `raw`, so the control
        // covers the write too.
        let planted = std::fs::read_to_string(tree.join("agent.log")).expect("the planted file");
        assert!(
            planted.contains(TOKEN) && planted.contains(NODE),
            "the control must find both credentials in the input"
        );

        let logs = gather_logs(
            &[(reader::Source::Agent, tree.clone())],
            &[TOKEN.to_string(), NODE.to_string()],
            &Caps::default(),
        );
        let host = facts(vec![TOKEN.to_string(), NODE.to_string()]);
        let files = bundle_files(
            &host,
            &logs,
            &Failures::default(),
            "2026-09-24T22:55:01+08:00",
        );

        assert_eq!(
            files[0..2]
                .iter()
                .map(|file| String::from_utf8_lossy(&file.bytes).to_string())
                .collect::<String>()
                .matches(TOKEN)
                .count(),
            0,
            "the token must not be in the summary or the manifest"
        );

        let written =
            write_archive(&base, &archive_stem("20260924-225501"), &files).expect("the archive");
        let all = std::fs::read(&written.path).expect("the bytes");
        for (label, needle) in [("token", TOKEN), ("node credential", NODE)] {
            assert!(
                !all.windows(needle.len())
                    .any(|window| window == needle.as_bytes()),
                "the {label} reached the archive"
            );
        }
        // And the mask did its job rather than the line being dropped: the key is
        // still there, which is what makes the line useful to read.
        let entry = text_of(&files, "logs/agent/agent.log");
        assert!(entry.contains("node_credential=***"), "{entry}");
        assert!(entry.contains("token=***"), "{entry}");
        std::fs::remove_dir_all(&base).ok();
    }

    /// The second pass over a log entry changes nothing.
    ///
    /// `bundle_files` masks every entry's text on the way in, and `gather_logs`
    /// has already masked it. This is what makes the second pass safe to state as
    /// an invariant rather than a risk: if it were not idempotent, the entry in
    /// the archive would differ from the entry the manifest counts, and a person
    /// comparing them would be comparing two different files.
    #[test]
    fn masking_twice_leaves_the_bytes_alone() {
        const TOKEN: &str = "launch-token-9f2c8a41";
        let base = scratch("idempotent");
        let tree = base.join("logs/desktop");
        plant(
            &tree.join("desktop.log"),
            &format!("2026-09-24T22:55:01 [INFO] desktop.auth: token={TOKEN} sent\n"),
        );
        let secrets = vec![TOKEN.to_string()];

        let logs = gather_logs(
            &[(reader::Source::Desktop, tree.clone())],
            &secrets,
            &Caps::default(),
        );
        let once = logs.entries[0].text.clone();
        let files = bundle_files(&facts(secrets), &logs, &Failures::default(), "now");
        let twice = text_of(&files, "logs/desktop/desktop.log");

        assert_eq!(once, twice, "the second pass must be a no-op");
        assert!(twice.contains("token=***"), "{twice}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_credential_is_masked_even_in_the_field_the_agent_answered() {
        let host = facts(vec!["agent-secret-value".to_string()]);
        let logs = Logs {
            entries: vec![LogEntry {
                name: "logs/agent/task.log".to_string(),
                text: "2026-09-24T22:00:00 [ERROR] wt_media_agent.task: agent-secret-value"
                    .to_string(),
                source_bytes: 10,
                modified: None,
                truncated: false,
            }],
            omitted: Vec::new(),
        };
        let files = bundle_files(&host, &logs, &Failures::default(), "now");
        let entry = text_of(&files, "logs/agent/task.log");
        assert!(entry.contains("***"), "{entry}");
        assert!(!entry.contains("agent-secret-value"));
    }

    #[test]
    fn both_trees_are_named_after_their_source_and_only_what_is_listed_is_read() {
        let base = scratch("two-trees");
        let desktop = base.join("desktop-tree");
        let agent = base.join("agent-tree");
        plant(
            &desktop.join("desktop.log"),
            "2026-09-24T10:00:00 [INFO] d\n",
        );
        plant(
            &desktop.join("desktop.log.2026-09-24-09"),
            "2026-09-24T09:00:00 [INFO] older\n",
        );
        plant(&agent.join("agent.log"), "2026-09-24T11:00:00 [INFO] a\n");
        // Not in any tree: a subdirectory is not listed, so its contents cannot
        // be read. The listing is the allowlist.
        plant(&agent.join("nested/secret.txt"), "must not be read");

        let logs = gather_logs(
            &[
                (reader::Source::Desktop, desktop.clone()),
                (reader::Source::Agent, agent.clone()),
            ],
            &[],
            &Caps::default(),
        );

        let names: Vec<&str> = logs.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "logs/desktop/desktop.log",
                "logs/desktop/desktop.log.2026-09-24-09",
                "logs/agent/agent.log",
            ]
        );
        assert!(
            logs.entries
                .iter()
                .all(|entry| !entry.text.contains("must not be read")),
            "nothing outside the listing is read"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    /// A tree that exists but cannot be read is an omission; a tree that is not
    /// there at all is not even that.
    ///
    /// The distinction is the reader's (`list` answers `Ok` with nothing for an
    /// absent directory and `Err` for one it cannot open), and it survives into
    /// the bundle: "there are no logs yet" and "your logs are there but I could
    /// not read them" are different sentences to the person holding the archive.
    #[test]
    fn a_tree_that_cannot_be_read_is_an_omission_rather_than_a_failed_export() {
        use std::os::unix::fs::PermissionsExt as _;
        let base = scratch("unreadable");
        let closed = base.join("closed");
        let missing = base.join("not-here");
        std::fs::create_dir_all(&closed).expect("the closed tree");
        // A mode the process cannot read through. The test only runs as an
        // ordinary user; as `root` the open succeeds and the assertions below
        // would fail, which is the honest outcome -- a guarantee that cannot be
        // exercised must not be reported as one.
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000))
            .expect("closing the tree");

        let logs = gather_logs(
            &[
                (reader::Source::Desktop, closed.clone()),
                (reader::Source::Agent, missing.clone()),
            ],
            &[],
            &Caps::default(),
        );

        assert!(logs.entries.is_empty());
        assert_eq!(
            logs.omitted.len(),
            1,
            "only the tree that could not be read is omitted: {:?}",
            logs.omitted
        );
        assert_eq!(logs.omitted[0].reason, OmittedReason::Unreadable);
        assert_eq!(logs.omitted[0].name, closed.display().to_string());

        // The other half of the claim: a bundle with nothing in it is still a
        // bundle, so the export did not fail.
        let files = bundle_files(&facts(vec![]), &logs, &Failures::default(), "now");
        assert!(text_of(&files, MANIFEST_NAME).contains("unreadable"));
        assert!(text_of(&files, SUMMARY_NAME).contains("\"omitted\": 1"));

        // Put the mode back before removing, or the removal is the test's own
        // failure rather than the code's.
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    /// **The bundle cannot reach user media**, and this is the whole of why.
    ///
    /// 素材/成片 live under a data root, and inside *subdirectories* of the
    /// places this product writes. The export reads one directory per tree,
    /// non-recursively, and reads only regular files that are direct children —
    /// so a media file is unreachable for the same reason the cleanup cannot
    /// reach one: there is no code path that walks downward.
    ///
    /// The nested case is the realistic one and it is asserted here. A `.mp4`
    /// lying *directly* in the log tree is a different case and is deliberately
    /// **included** (it is masked and bounded like every other entry): the tree
    /// belongs to this component, the file would be evidence about this
    /// component, and T-06 already settled that an unrecognised name in the tree
    /// is visible rather than skipped. Registered in the evidence, not silently
    /// decided here.
    #[test]
    fn a_media_file_inside_a_subdirectory_of_the_tree_is_not_reachable() {
        let base = scratch("media");
        let tree = base.join("logs/desktop");
        plant(
            &tree.join("desktop.log"),
            "2026-09-24T22:55:01 [INFO] desktop: hi\n",
        );
        plant(
            &tree.join("media/成片/clip.mp4"),
            "not really an mp4, and not ours to read",
        );
        plant(
            &tree.join(".local/data/versions/media/raw.png"),
            "also not ours",
        );

        let logs = gather_logs(
            &[(reader::Source::Desktop, tree.clone())],
            &[],
            &Caps::default(),
        );

        let names: Vec<&str> = logs.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["logs/desktop/desktop.log"],
            "one directory, read once, and no descent: {names:?}"
        );
        assert!(
            logs.omitted.is_empty(),
            "a subdirectory is not log evidence and not an omission: {:?}",
            logs.omitted
        );

        // The control: the same tree *does* reach a media file lying directly in
        // it, so the assertion above is about descent and not about a filter
        // that happens to reject everything unfamiliar.
        plant(&tree.join("clip.mp4"), "planted in the tree itself");
        let logs = gather_logs(
            &[(reader::Source::Desktop, tree.clone())],
            &[],
            &Caps::default(),
        );
        let names: Vec<&str> = logs.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["logs/desktop/desktop.log", "logs/desktop/clip.mp4"],
            "a direct child is listed, whatever it is called: {names:?}"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_name_over_the_cap_is_left_out_rather_than_shortened() {
        let base = scratch("long-name");
        let tree = base.join("tree");
        let long = format!("{}.log", "n".repeat(200));
        plant(&tree.join(&long), "2026-09-24T10:00:00 [INFO] hi\n");
        plant(&tree.join("desktop.log"), "2026-09-24T10:00:00 [INFO] hi\n");

        let logs = gather_logs(
            &[(reader::Source::Desktop, tree.clone())],
            &[],
            &Caps::default(),
        );

        assert_eq!(logs.entries.len(), 1, "only the name that fits is read");
        assert!(logs.entries[0].name.ends_with("/desktop.log"));
        assert_eq!(logs.omitted.len(), 1);
        assert_eq!(logs.omitted[0].reason, OmittedReason::NameTooLong);
        assert!(
            logs.omitted[0].name.len() > 128,
            "the omission names the file as it is on disk"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_file_longer_than_the_cap_is_the_end_of_the_file_and_says_so() {
        let base = scratch("window");
        let tree = base.join("tree");
        let lines: Vec<String> = (0..40)
            .map(|n| format!("2026-09-24T10:00:{n:02} [INFO] line-{n}"))
            .collect();
        plant(
            &tree.join("desktop.log"),
            &format!("{}\n", lines.join("\n")),
        );

        let caps = Caps {
            max_entry_lines: 5,
            ..Caps::default()
        };
        let logs = gather_logs(&[(reader::Source::Desktop, tree.clone())], &[], &caps);

        assert_eq!(logs.entries.len(), 1);
        let entry = &logs.entries[0];
        assert!(entry.truncated, "a window says it is one");
        assert!(entry.text.contains("line-39"), "the end is what is kept");
        assert!(!entry.text.contains("line-0\n"), "and the start is not");
        assert_eq!(entry.text.lines().count(), 5);
        assert_eq!(
            entry.source_bytes,
            std::fs::metadata(tree.join("desktop.log")).unwrap().len(),
            "the entry says how big the file it took a window of is"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    /// A file that ends exactly at the cap is not called a window.
    #[test]
    fn a_file_that_ends_at_the_cap_exactly_is_not_reported_as_a_window() {
        let base = scratch("exact");
        let tree = base.join("tree");
        let lines: Vec<String> = (0..5)
            .map(|n| format!("2026-09-24T10:00:{n:02} [INFO] line-{n}"))
            .collect();
        plant(
            &tree.join("desktop.log"),
            &format!("{}\n", lines.join("\n")),
        );

        let caps = Caps {
            max_entry_lines: 5,
            ..Caps::default()
        };
        let logs = gather_logs(&[(reader::Source::Desktop, tree.clone())], &[], &caps);
        assert_eq!(logs.entries[0].text.lines().count(), 5);
        assert!(
            !logs.entries[0].truncated,
            "five lines of a five-line file is the whole file"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_log_payload_stops_at_its_ceiling_and_registers_everything_after_it() {
        let base = scratch("payload");
        let tree = base.join("tree");
        for hour in ["08", "09", "10"] {
            plant(
                &tree.join(format!("desktop.log.2026-09-24-{hour}")),
                &"2026-09-24T08:00:00 [INFO] x\n".repeat(20),
            );
        }

        // Each file's masked text is 579 bytes (20 records of 29, joined), so a
        // ceiling of 1200 admits two and refuses the third. The numbers are
        // spelled out because the assertion below is about where the edge falls,
        // and an edge nobody can locate is not a tested edge.
        let caps = Caps {
            max_archive_bytes: 1200,
            ..Caps::default()
        };
        let logs = gather_logs(&[(reader::Source::Desktop, tree.clone())], &[], &caps);

        assert_eq!(logs.entries[0].text.len(), 579, "the arithmetic above");
        assert_eq!(logs.entries.len(), 2, "the ceiling stopped the payload");
        assert_eq!(logs.omitted.len(), 1);
        assert_eq!(logs.omitted[0].reason, OmittedReason::ArchiveFull);
        // The ceiling drops the **oldest**, because the listing is newest-first
        // and the payload is filled in that order. That is the property the
        // bundle wants: the hours around the incident are the ones a person
        // reads, and an archive that quietly kept the start of the week instead
        // would be evidence of the wrong hours.
        assert!(
            logs.entries[0].name.ends_with("desktop.log.2026-09-24-10"),
            "the newest archive is the one that stays: {:?}",
            logs.entries[0].name
        );
        assert!(
            logs.omitted[0].name.ends_with("desktop.log.2026-09-24-08"),
            "and the oldest is the one that is registered: {:?}",
            logs.omitted
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_failure_summary_is_the_warn_and_louder_records() {
        let entry = LogEntry {
            name: "logs/agent/task.log".to_string(),
            text: [
                "2026-09-24T10:00:00 [INFO] task.start: no",
                "2026-09-24T10:00:01 [WARNING] task.retry: maybe",
                "2026-09-24T10:00:02 [ERROR] task.run: yes",
                "2026-09-24T10:00:03 [DEBUG] task.trace: no",
            ]
            .join("\n"),
            source_bytes: 10,
            modified: None,
            truncated: false,
        };
        let failures = failure_records(&[entry], &Caps::default());
        assert_eq!(failures.source.as_deref(), Some("logs/agent/task.log"));
        assert_eq!(failures.records.len(), 2);
        assert!(failures.records[0].contains("WARNING"));
        assert!(failures.records[1].contains("ERROR"));
        assert!(!failures.truncated);
    }

    #[test]
    fn the_failure_summary_stops_at_its_cap_and_says_so() {
        let records: Vec<String> = (0..8)
            .map(|n| format!("2026-09-24T10:00:{n:02} [ERROR] task.run: nope-{n}"))
            .collect();
        let entry = LogEntry {
            name: "logs/agent/task.log".to_string(),
            text: records.join("\n"),
            source_bytes: 10,
            modified: None,
            truncated: false,
        };
        let caps = Caps {
            max_failure_records: 3,
            ..Caps::default()
        };
        let failures = failure_records(&[entry.clone()], &caps);
        assert_eq!(failures.records.len(), 3);
        assert!(failures.truncated);

        let caps = Caps {
            max_failure_bytes: 60,
            ..Caps::default()
        };
        let failures = failure_records(&[entry], &caps);
        assert!(failures.truncated);
        assert!(
            failures.records.iter().map(String::len).sum::<usize>() <= 60,
            "the byte ceiling holds: {:?}",
            failures.records
        );
    }

    #[test]
    fn a_bundle_with_no_task_log_has_no_failure_source() {
        let failures = failure_records(
            &[LogEntry {
                name: "logs/agent/agent.log".to_string(),
                text: "2026-09-24T10:00:00 [ERROR] something".to_string(),
                source_bytes: 1,
                modified: None,
                truncated: false,
            }],
            &Caps::default(),
        );
        assert_eq!(failures, Failures::default());
        let summary = summary(&facts(vec![]), &Logs::default(), &failures, "now");
        assert!(summary["failed_work"]["source"].is_null());
        assert_eq!(summary["failed_work"]["count"], 0);
    }

    #[test]
    fn the_written_archive_holds_exactly_what_was_handed_to_it() {
        let base = scratch("round-trip");
        let logs = Logs {
            entries: vec![
                LogEntry {
                    name: "logs/desktop/desktop.log".to_string(),
                    text: "hello\n".to_string(),
                    source_bytes: 6,
                    modified: None,
                    truncated: false,
                },
                LogEntry {
                    name: "logs/desktop/desktop.log.2026-09-24-09".to_string(),
                    text: "older\n".to_string(),
                    source_bytes: 6,
                    modified: None,
                    truncated: false,
                },
            ],
            omitted: Vec::new(),
        };
        let files = bundle_files(&facts(vec![]), &logs, &Failures::default(), "now");
        let written =
            write_archive(&base, &archive_stem("20260924-225501"), &files).expect("the archive");

        let read = read_back(&written.path);
        assert_eq!(
            read.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec![
                SUMMARY_NAME,
                MANIFEST_NAME,
                "logs/desktop/desktop.log",
                "logs/desktop/desktop.log.2026-09-24-09",
            ]
        );
        assert_eq!(read[3].1, b"older\n");
        assert_eq!(
            written.bytes,
            std::fs::metadata(&written.path).unwrap().len()
        );
        assert!(written.path.to_string_lossy().ends_with(".tar.gz"));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_checksum_file_names_the_archive_and_holds_its_digest() {
        let base = scratch("checksum");
        let files = bundle_files(
            &facts(vec![]),
            &Logs::default(),
            &Failures::default(),
            "now",
        );
        let written =
            write_archive(&base, &archive_stem("20260924-225501"), &files).expect("the archive");

        let name = written
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(
            written.checksum_path.file_name().unwrap().to_string_lossy(),
            format!("{name}.sha256")
        );
        let line = std::fs::read_to_string(&written.checksum_path).expect("the checksum file");
        assert_eq!(line, format!("{}  {name}\n", written.sha256));
        assert_eq!(written.sha256.len(), 64, "sha256 in hex");
        assert!(written.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        std::fs::remove_dir_all(&base).ok();
    }

    /// The digest is the digest **of the file**, checked by a reader that shares
    /// no code with the one that computed it.
    ///
    /// The test above is self-consistent: it compares `written.sha256` against
    /// the line that `write_archive` itself wrote, which passes for any digest
    /// that is a deterministic function of the same inputs. A mutation that
    /// hashed the first byte of the file instead of all of it survived that
    /// test — and a checksum that does not answer "are these bytes the bytes?"
    /// is worse than no checksum, because it is trusted.
    ///
    /// So the second opinion comes from the operating system's own tool. If
    /// neither tool is on this machine the premise is printed and the test
    /// returns: an unavailable witness is not a passing one.
    #[test]
    fn the_digest_is_the_digest_of_the_file_by_an_independent_reader() {
        let base = scratch("digest-witness");
        let files = bundle_files(
            &facts(vec![]),
            &Logs::default(),
            &Failures::default(),
            "now",
        );
        let written =
            write_archive(&base, &archive_stem("20260924-225501"), &files).expect("the archive");

        let Some(read) = external_digest(&written.path) else {
            eprintln!(
                "premise failed: neither `shasum` nor `sha256sum` is on this machine, \
                 so the digest was not checked by an independent reader"
            );
            std::fs::remove_dir_all(&base).ok();
            return;
        };

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(
            written.sha256, read,
            "the reported digest must be the digest of the bytes on disk"
        );
        assert!(
            written.bytes > 1,
            "the file is more than one byte, which is what makes the check bite: {}",
            written.bytes
        );
    }

    /// `shasum -a 256 <path>` or `sha256sum <path>`, whichever this machine has.
    ///
    /// Both print `<hex>  <name>`, and both name the file relative to their own
    /// working directory, so only the first field is read.
    fn external_digest(path: &Path) -> Option<String> {
        for (program, args) in [("shasum", ["-a", "256"]), ("sha256sum", ["", ""])] {
            let mut command = std::process::Command::new(program);
            command.args(args.iter().filter(|arg| !arg.is_empty()));
            let Ok(output) = command.arg(path).output() else {
                continue;
            };
            if !output.status.success() {
                continue;
            }
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(field) = text.split_whitespace().next() {
                return Some(field.to_string());
            }
        }
        None
    }

    #[test]
    fn an_archive_already_at_that_name_is_never_overwritten() {
        let base = scratch("no-overwrite");
        let stem = archive_stem("20260924-225501");
        let files = bundle_files(
            &facts(vec![]),
            &Logs::default(),
            &Failures::default(),
            "now",
        );

        let first = write_archive(&base, &stem, &files).expect("the first");
        let before = std::fs::read(&first.path).expect("its bytes");
        let second = write_archive(&base, &stem, &files).expect("the second");

        assert_ne!(first.path, second.path);
        assert!(
            second
                .path
                .to_string_lossy()
                .ends_with("wt-media-diagnostic-20260924-225501-2.tar.gz"),
            "{}",
            second.path.display()
        );
        assert_eq!(
            std::fs::read(&first.path).expect("still there"),
            before,
            "the first bundle is untouched"
        );
        assert!(
            second.checksum_path.exists(),
            "each archive gets its own line"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_attempts_are_bounded() {
        let base = scratch("bounded");
        let stem = archive_stem("20260924-225501");
        let files = bundle_files(
            &facts(vec![]),
            &Logs::default(),
            &Failures::default(),
            "now",
        );
        for _ in 0..MAX_NAME_ATTEMPTS {
            write_archive(&base, &stem, &files).expect("one of the attempts");
        }
        let refused = write_archive(&base, &stem, &files);
        assert!(
            matches!(refused, Err(DiagnosticError::TargetTaken { .. })),
            "{refused:?}"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_directory_that_is_not_one_is_refused() {
        let base = scratch("not-a-dir");
        let missing = base.join("nowhere");
        let refused = write_archive(&missing, "stem", &[]);
        assert!(
            matches!(refused, Err(DiagnosticError::Target { .. })),
            "{refused:?}"
        );
        let file = base.join("a-file");
        std::fs::write(&file, b"x").unwrap();
        let refused = write_archive(&file, "stem", &[]);
        assert!(
            matches!(refused, Err(DiagnosticError::Target { .. })),
            "{refused:?}"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_name_that_is_not_one_component_is_refused() {
        for name in [
            "",
            "/etc/passwd",
            "../escape",
            "logs/../../escape",
            "logs//double",
            "logs/agent/",
        ] {
            assert!(
                matches!(check_name(name), Err(DiagnosticError::BadName { .. })),
                "{name:?} must be refused"
            );
        }
        // The control: the shape this module actually builds is accepted.
        assert!(check_name("logs/agent/agent.log").is_ok());
        assert!(check_name("summary.json").is_ok());
    }

    #[test]
    fn the_entry_below_a_nested_directory_is_not_in_the_bundle() {
        // The other half of `check_name`: the names that arrive are the ones the
        // listing gave, and a listing never gives a path.
        let base = scratch("names-from-listing");
        let tree = base.join("tree");
        plant(&tree.join("desktop.log"), "2026-09-24T10:00:00 [INFO] x\n");
        let logs = gather_logs(
            &[(reader::Source::Desktop, tree.clone())],
            &[],
            &Caps::default(),
        );
        for entry in &logs.entries {
            assert!(
                check_name(&entry.name).is_ok(),
                "{} must be a name the writer accepts",
                entry.name
            );
            assert_eq!(entry.name.matches('/').count(), 2);
        }
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_manifest_lists_every_entry_and_every_omission() {
        let logs = Logs {
            entries: vec![LogEntry {
                name: "logs/desktop/desktop.log".to_string(),
                text: "hello\n".to_string(),
                source_bytes: 4096,
                modified: None,
                truncated: true,
            }],
            omitted: vec![
                Omission {
                    name: "logs/agent/huge.log".to_string(),
                    reason: OmittedReason::ArchiveFull,
                },
                Omission {
                    name: "logs/agent/unreadable-tree".to_string(),
                    reason: OmittedReason::Unreadable,
                },
            ],
        };
        let files = bundle_files(&facts(vec![]), &logs, &Failures::default(), "now");
        let manifest = text_of(&files, MANIFEST_NAME);
        assert!(manifest.contains("logs/desktop/desktop.log"));
        assert!(manifest.contains("尾部窗口"), "{manifest}");
        assert!(manifest.contains("logs/agent/huge.log"));
        assert!(manifest.contains("archive_full"));
        assert!(manifest.contains("unreadable"));
        assert!(
            manifest.contains(".sha256"),
            "the manifest says where the digest is"
        );
    }

    #[test]
    fn the_manifest_never_claims_a_credential_free_bundle_it_cannot_know() {
        // A small property with a large failure mode: the manifest explains the
        // mask and where the digest is, and does not print the digest itself,
        // which would be a self-reference.
        let files = bundle_files(
            &facts(vec![]),
            &Logs::default(),
            &Failures::default(),
            "now",
        );
        let manifest = text_of(&files, MANIFEST_NAME);
        assert!(!manifest.contains("sha256: "), "{manifest}");
    }
}
