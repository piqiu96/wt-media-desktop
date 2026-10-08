//! 导出脱敏诊断包: one command, one archive.
//!
//! The rules are in [`crate::diagnostic`] — what goes in, what the caps are, and
//! what happens at each edge. This module is the wiring: which roots to read,
//! which facts this process can answer, and where the archive lands. It is split
//! the way `commands::cleanup` and `commands::storage` are split, for the reason
//! both of those give: a command takes `State`, which no test can build, so
//! everything worth asserting lives in a function that takes plain references.
//!
//! ## What this command can see, and what it deliberately cannot
//!
//! It reads four pieces of managed state: the config (for the Agent's data
//! directory), the host facts injected at launch, the Agent client (for its
//! status), the process handle and the binding. Of those, exactly one holds a
//! credential — [`RuntimeBinding::node_credential`] — and it is readable **only**
//! to be masked: [`secrets`] takes it and the bundle's facts never do. The launch
//! token is in the same position from the other side: it is in
//! [`DiagnosticHost::secrets`], which is masked, and nowhere else in this module.
//!
//! ## The Agent's status is a fact, not a precondition
//!
//! An Agent that will not answer is one of the two reasons somebody is exporting
//! a bundle. So its status is recorded as [`AgentFacts::Unreachable`] with the
//! reason, and the export continues — the same choice the reader makes for an
//! absent log tree, and the opposite of the one the cleanup makes for an
//! unreadable one (there, nothing is known and the operation must not proceed;
//! here, "the Agent did not answer" *is* the finding).

use crate::app_paths::AppPaths;
use crate::bootstrap;
use crate::commands::storage::resolve;
use crate::config::DesktopConfig;
// The rules module is named `crate::diagnostic` rather than imported as
// `diagnostic`, and the reason is not style: `#[tauri::command]` expands to an
// item carrying `#[diagnostic::on_unimplemented]`, which is a **tool attribute**
// resolved in this scope. A `use crate::diagnostic::{self, …}` here binds that
// name to this crate's module and the macro's own expansion stops compiling
// ("cannot find `on_unimplemented` in `diagnostic`"). Full paths, no `self`.
use crate::diagnostic::{AgentFacts, Caps, DiagnosticError, Failures, HostFacts, Logs, Written};
use crate::dto::{DiagnosticReport, LocalAgentStatus, LocalAgentStatusResponse};
use crate::http::LocalAgentClient;
use crate::logging::reader::Source;
use crate::state::{AgentProcess, RuntimeBinding, RuntimeBindingState};
use crate::storage;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tauri::State;

/// The facts about this launch that only `main` can answer.
///
/// Managed as state because the command has no other way to reach them: the
/// config file's source and a rejected file's reason are decided once, during
/// startup, and re-deriving them here would be a second resolution that could
/// disagree with the one the launch actually used — the same argument
/// `commands::storage` makes for calling the launch's resolvers rather than
/// caching their values.
pub struct DiagnosticHost {
    /// `paths::Source::code()` — which candidate supplied the configuration.
    pub config_source: String,
    /// Why a located configuration file was refused, if one was.
    pub config_rejected: Option<String>,
    /// Values the bundle must not contain. The launch token, and anything else
    /// the launch knows is a secret; the binding's credential is added per call
    /// by [`secrets`], because it arrives after startup.
    pub secrets: Vec<String>,
}

impl DiagnosticHost {
    pub fn new(
        source: crate::paths::Source,
        rejected: Option<String>,
        secrets: Vec<String>,
    ) -> Self {
        Self {
            config_source: source.code().to_string(),
            config_rejected: rejected,
            secrets,
        }
    }
}

/// Every value the bundle must not contain, given the binding as it is now.
///
/// The credential is added here and not stored: it belongs to a session that
/// starts and ends inside this process (D-10), so a copy of it in a `Vec` that
/// outlives the session would be a credential held for no reason. Empty values
/// are dropped — a mask told to hide `""` would mask every position in every
/// string, which is the one way this list can do damage instead of preventing it.
fn secrets(injected: &[String], binding: Option<&RuntimeBinding>) -> Vec<String> {
    let mut all: Vec<String> = injected.iter().filter(|s| !s.is_empty()).cloned().collect();
    if let Some(binding) = binding {
        if !binding.node_credential.is_empty() {
            all.push(binding.node_credential.clone());
        }
    }
    all
}

/// Where an export goes: the folder a person attaches files from, when it is
/// there, and this component's own data root when it is not.
///
/// `~/Downloads` because the artifact exists to be handed to somebody, and the
/// place people look for a file they are about to attach is the one place every
/// desktop asks them to look. It is *not* a security boundary either way — the
/// bundle is masked by construction, which is what makes the destination a
/// convenience rather than a decision about confidentiality — but it is a
/// decision, so it is stated here and registered in the evidence rather than
/// left as a literal in the middle of a command.
///
/// The fallback is the data root and not the desktop or the current directory: a
/// `HOME` with no `Downloads` is a machine with an unusual layout, not a reason
/// to write somewhere this process cannot vouch for.
fn output_directory(paths: &AppPaths, home: Option<&Path>) -> PathBuf {
    home.map(|home| home.join("Downloads"))
        .filter(|downloads| downloads.is_dir())
        .unwrap_or_else(|| paths.data.clone())
}

/// The Agent's status as the bundle carries it: named fields, all strings.
///
/// Absent means "the Agent did not say", and it is spelled by the key being
/// *missing* rather than by an empty value — the same rule `commands::agent`
/// follows for a record's `operation_id`. An empty string would be a claim the
/// Agent made; a missing key is the fact that it made none.
///
/// `bit_profile_ids` is **counted, not listed**. The ids name the user's browser
/// profiles and a machine may hold hundreds; a bundle's reader needs to know
/// whether the Agent is holding any, not which. Registered as a deliberate
/// omission in the evidence, because "we left something out" is the kind of
/// thing a reader should find written down rather than infer from a screenshot.
fn agent_fields(status: &LocalAgentStatus) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    fields.insert("agent_id".to_string(), status.agent_id.clone());
    fields.insert("status".to_string(), status.status.clone());
    fields.insert(
        "bit_profile_count".to_string(),
        status.bit_profile_ids.len().to_string(),
    );
    fields.insert(
        "pending_result_count".to_string(),
        status.pending_result_count.to_string(),
    );
    let mut optional = |key: &str, value: Option<&String>| {
        if let Some(value) = value {
            fields.insert(key.to_string(), value.clone());
        }
    };
    optional("node_id", status.node_id.as_ref());
    optional("agent_version", status.agent_version.as_ref());
    optional("python_version", status.python_version.as_ref());
    optional("bitbrowser_status", status.bitbrowser_status.as_ref());
    optional("main_user_id", status.main_user_id.as_ref());
    optional("operating_system", status.operating_system.as_ref());
    optional("cpu_architecture", status.cpu_architecture.as_ref());
    optional("workdir_status", status.workdir_status.as_ref());
    optional("current_task_id", status.current_task_id.as_ref());
    optional("current_task_status", status.current_task_status.as_ref());
    if let Some(progress) = status.current_task_progress {
        fields.insert("current_task_progress".to_string(), progress.to_string());
    }
    fields
}

/// Ask the Agent who it is. Its silence is recorded, never fatal.
async fn agent_facts(client: &LocalAgentClient) -> AgentFacts {
    let response = match client.get("/api/v1/status").send().await {
        Ok(response) => response,
        Err(error) => {
            return AgentFacts::Unreachable {
                error: format!("agent unreachable: {error}"),
            }
        }
    };
    match response.json::<LocalAgentStatusResponse>().await {
        Ok(body) => AgentFacts::Answered(agent_fields(&LocalAgentStatus::from(body.data))),
        Err(error) => AgentFacts::Unreachable {
            error: format!("invalid status response: {error}"),
        },
    }
}

/// Everything this process can say about itself, assembled in one place.
///
/// `app_version` and `build_version` are two values and are both reported: the
/// first is the product's, from `tauri.conf.json` through `package_info` — the
/// number a `.dmg` is stamped with and the one a person quotes in a report — and
/// the second is the crate's, compiled into this binary. Today they are both
/// `0.1.0`; nothing makes them stay equal, and a bundle that reported only the
/// crate's would be wrong the first time the product number moved without a
/// release rebuild. Which of the five versions this project has is which is a
/// question CHG-D owns; this reports the two this process can see rather than
/// picking one and calling it "the version".
fn host_facts(
    host: &DiagnosticHost,
    app_version: String,
    resolved: &crate::commands::storage::Resolved,
    sidecar_running: bool,
    binding: Option<&RuntimeBinding>,
    agent: AgentFacts,
    secrets: Vec<String>,
) -> HostFacts {
    HostFacts {
        app_version,
        build_version: env!("CARGO_PKG_VERSION").to_string(),
        environment: bootstrap::build_environment().code().to_string(),
        config_source: host.config_source.clone(),
        config_rejected: host.config_rejected.clone(),
        data_root: resolved.paths.data.display().to_string(),
        logs_root: resolved.paths.logs.display().to_string(),
        cache_root: resolved.paths.cache.display().to_string(),
        // `None` is "could not be measured", never zero: the same distinction
        // `commands::storage` draws, and here the reading sits beside the roots
        // so a reader can tell a full disk from a failed `statvfs`.
        free_bytes: storage::available_bytes_for(&resolved.paths.data).ok(),
        sidecar_running,
        binding_node_id: binding.map(|binding| binding.node_id.clone()),
        agent,
        secrets,
    }
}

/// The read of the two trees, in the order the page shows them.
fn logs_of(resolved: &crate::commands::storage::Resolved, secrets: &[String], caps: &Caps) -> Logs {
    let trees: Vec<(Source, PathBuf)> = resolved
        .trees()
        .into_iter()
        .map(|(source, path)| (source, path.to_path_buf()))
        .collect();
    crate::diagnostic::gather_logs(&trees, secrets, caps)
}

/// One export, from the roots to the written file.
///
/// Split from the command so the whole path can be exercised in a test — with a
/// real directory tree and a real archive, since that is what the assertions are
/// about.
#[allow(clippy::too_many_arguments)]
fn export(
    resolved: &crate::commands::storage::Resolved,
    host: &DiagnosticHost,
    app_version: String,
    sidecar_running: bool,
    binding: Option<&RuntimeBinding>,
    agent: AgentFacts,
    home: Option<&Path>,
    now: SystemTime,
) -> Result<(Written, Logs, Failures, String), DiagnosticError> {
    let secrets = secrets(&host.secrets, binding);
    let facts = host_facts(
        host,
        app_version,
        resolved,
        sidecar_running,
        binding,
        agent,
        secrets.clone(),
    );

    let caps = Caps::default();
    let logs = logs_of(resolved, &secrets, &caps);
    let failures = crate::diagnostic::failure_records(&logs.entries, &caps);
    let created_at = crate::diagnostic::created_at(now);
    let files = crate::diagnostic::bundle_files(&facts, &logs, &failures, &created_at);

    let directory = output_directory(&resolved.paths, home);
    let written =
        crate::diagnostic::write_archive(&directory, &crate::diagnostic::stem_at(now), &files)?;
    Ok((written, logs, failures, created_at))
}

#[tauri::command]
pub async fn local_diagnostic_export(
    app: tauri::AppHandle,
    config: State<'_, DesktopConfig>,
    host: State<'_, DiagnosticHost>,
    client: State<'_, LocalAgentClient>,
    process: State<'_, AgentProcess>,
    binding: State<'_, RuntimeBindingState>,
) -> Result<DiagnosticReport, String> {
    let resolved = resolve(&config)?;

    let bound = binding
        .0
        .lock()
        .map_err(|_| "runtime binding lock poisoned".to_string())?
        .clone();
    let sidecar_running = process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned".to_string())?
        .is_some();

    let agent = agent_facts(&client).await;
    let agent_state = agent.state().to_string();

    let home = std::env::var_os("HOME").map(PathBuf::from);
    // The product's version, from the same `package_info` the launch resolved its
    // config against — not a second reading of `tauri.conf.json` at command time.
    let app_version = app.package_info().version.to_string();
    let (written, logs, failures, created_at) = export(
        &resolved,
        &host,
        app_version,
        sidecar_running,
        bound.as_ref(),
        agent,
        home.as_deref(),
        SystemTime::now(),
    )
    .map_err(|error| format!("导出诊断包失败: {error}"))?;

    Ok(crate::dto::report(
        &written,
        &created_at,
        &logs,
        &failures,
        &agent_state,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_paths::AppPaths;

    /// A scratch directory of this test's own, removed by the caller.
    pub(super) fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-diagnostic-cmd-{}-{}-{}",
            label,
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).expect("scratch dir");
        path
    }

    fn plant(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, text).expect("plant");
    }

    /// Four roots of this test's own, in the shape `AppPaths` has.
    fn roots(base: &Path) -> AppPaths {
        AppPaths {
            data: base.join("data"),
            versions: base.join("data/versions"),
            logs: base.join("logs/desktop-tree"),
            cache: base.join("cache"),
        }
    }

    fn resolved(base: &Path) -> crate::commands::storage::Resolved {
        crate::commands::storage::Resolved {
            paths: roots(base),
            agent_logs: base.join("logs/agent-tree"),
        }
    }

    pub(super) fn host(secrets: Vec<String>) -> DiagnosticHost {
        DiagnosticHost::new(crate::paths::Source::DevelopmentTree, None, secrets)
    }

    fn a_status(profiles: Vec<&str>) -> LocalAgentStatus {
        LocalAgentStatus {
            node_id: Some("node-7".to_string()),
            agent_id: "agent-abc".to_string(),
            status: "running".to_string(),
            bitbrowser_status: Some("ready".to_string()),
            main_user_id: Some("user-42".to_string()),
            operating_system: Some("macOS".to_string()),
            cpu_architecture: Some("arm64".to_string()),
            agent_version: Some("0.2.2".to_string()),
            python_version: Some("3.11.9".to_string()),
            ffmpeg: None,
            workdir_status: None,
            disk: None,
            bit_profile_ids: profiles.into_iter().map(str::to_string).collect(),
            current_task_id: None,
            current_task_progress: None,
            current_task_status: Some("idle".to_string()),
            pending_result_count: 2,
        }
    }

    /// Read an archive back: `(name, bytes)` per entry, in the order written.
    pub(super) fn read_back(path: &Path) -> Vec<(String, Vec<u8>)> {
        use std::io::Read as _;
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

    /// The whole export, against a real tree, with the credential pair.
    ///
    /// Two claims in one run, because they are the two halves of one property:
    /// the **binding's node id is reported** (it is how somebody correlates this
    /// bundle with a Cloud record) and the **binding's credential is not** (it is
    /// the thing that would let whoever reads the archive act as this machine).
    /// `node_credential` is written into the log file first and read back from
    /// disk, so the control is on the bytes that actually went in: the mask is
    /// being asked to hide something that is genuinely present.
    #[test]
    fn the_export_reports_the_node_id_and_never_the_credential() {
        const TOKEN: &str = "launch-token-9f2c8a41";
        const NODE: &str = "node-credential-4b7e1d90";
        let base = scratch("export");
        let resolved = resolved(&base);
        // The destination this export will pick, created here so the test knows
        // where to look rather than reading it back out of the code under test:
        // `~/Downloads` when it is there (`output_directory`'s own test covers
        // the fallback), so the assertion below can name the directory.
        let downloads = base.join("Downloads");
        std::fs::create_dir_all(&downloads).expect("the destination");
        let line = format!(
            "2026-09-24T22:55:01 [INFO] agent.supervisor: node_credential={NODE} token={TOKEN}\n"
        );
        plant(&resolved.paths.logs.join("desktop.log"), &line);
        plant(
            &resolved.agent_logs.join("task.log"),
            "2026-09-24T22:56:00 [ERROR] wt_media_agent.task: task 7 failed\n",
        );

        let binding = RuntimeBinding {
            node_id: "node-7".to_string(),
            node_credential: NODE.to_string(),
        };
        let moment = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_787_000_000);

        let planted = std::fs::read_to_string(resolved.paths.logs.join("desktop.log"))
            .expect("the planted file");
        assert!(
            planted.contains(NODE) && planted.contains(TOKEN),
            "the control must find both values in the file on disk"
        );

        let (written, logs, failures, created_at) = export(
            &resolved,
            &host(vec![TOKEN.to_string()]),
            "0.1.0".to_string(),
            true,
            Some(&binding),
            AgentFacts::Answered(agent_fields(&a_status(vec!["p1", "p2", "p3"]))),
            Some(&base),
            moment,
        )
        .expect("the export");

        let entries = read_back(&written.path);
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "summary.json",
                "manifest.txt",
                "logs/desktop/desktop.log",
                "logs/agent/task.log",
            ]
        );

        let summary = String::from_utf8(entries[0].1.clone()).expect("utf-8");
        let document: serde_json::Value =
            serde_json::from_str(&summary).expect("the summary is JSON");
        assert_eq!(document["binding"]["node_id"], "node-7");
        assert_eq!(document["binding"]["bound"], true);
        assert_eq!(document["sidecar"]["running"], true);
        assert_eq!(document["desktop"]["app_version"], "0.1.0");
        assert_eq!(document["desktop"]["build_version"], "0.1.0");
        assert_eq!(document["desktop"]["config_source"], "development_tree");
        assert_eq!(document["agent"]["state"], "answered");
        assert_eq!(document["failed_work"]["count"], 1);
        assert_eq!(document["logs"]["entries"], 2);

        // The whole archive, byte-wise, not a field of it: the credential must
        // not be anywhere in the file that leaves this machine.
        let all = std::fs::read(&written.path).expect("the bytes");
        for (label, needle) in [("launch token", TOKEN), ("node credential", NODE)] {
            assert!(
                !all.windows(needle.len())
                    .any(|window| window == needle.as_bytes()),
                "the {label} reached the archive"
            );
        }
        // And the mask did its work rather than the whole line being dropped:
        // the keys are still there, which is what makes the line worth reading.
        let log = String::from_utf8(entries[2].1.clone()).expect("utf-8");
        assert!(log.contains("node_credential=***"), "{log}");
        assert!(log.contains("token=***"), "{log}");

        // The export landed in the folder `output_directory` picked, and it
        // describes the file that actually exists rather than the name it asked
        // for.
        assert_eq!(
            written.path.parent().expect("a parent"),
            downloads.as_path()
        );
        assert!(written
            .path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .starts_with("wt-media-diagnostic-"));
        assert_eq!(logs.entries.len(), 2);
        assert_eq!(logs.omitted.len(), 0);
        assert_eq!(failures.records.len(), 1);
        assert_eq!(failures.source.as_deref(), Some("logs/agent/task.log"));
        assert_eq!(created_at.len(), 19);
        assert!(written.checksum_path.exists());
        std::fs::remove_dir_all(&base).ok();
    }

    /// Where the bundle goes: the folder a person attaches from, when it is there.
    #[test]
    fn the_export_goes_to_downloads_when_there_is_one_and_the_data_root_when_there_is_not() {
        let base = scratch("destination");
        let paths = roots(&base);
        std::fs::create_dir_all(&paths.data).expect("the data root");

        let with_downloads = base.join("home-with");
        std::fs::create_dir_all(with_downloads.join("Downloads")).expect("Downloads");
        assert_eq!(
            output_directory(&paths, Some(&with_downloads)),
            with_downloads.join("Downloads")
        );

        // The control for the branch above: a home that has the directory and
        // still got the data root would mean the predicate is not what decides.
        let without = base.join("home-without");
        std::fs::create_dir_all(&without).expect("a home");
        assert_eq!(output_directory(&paths, Some(&without)), paths.data);

        // A `Downloads` that is a file is not a directory to write into.
        let file_home = base.join("home-file");
        std::fs::create_dir_all(&file_home).expect("a home");
        std::fs::write(file_home.join("Downloads"), b"not a directory").expect("a file");
        assert_eq!(output_directory(&paths, Some(&file_home)), paths.data);

        // And no home at all is the fallback, not a panic.
        assert_eq!(output_directory(&paths, None), paths.data);
        std::fs::remove_dir_all(&base).ok();
    }

    /// An empty secret is dropped: a mask told to hide `""` would strike out
    /// every position of every string, which turns the bundle into confetti.
    #[test]
    fn an_empty_secret_is_dropped_and_a_real_one_is_kept() {
        let kept = secrets(&["real-secret-value".to_string()], None);
        assert_eq!(kept, vec!["real-secret-value".to_string()]);

        let dropped = secrets(&[String::new(), "live".to_string()], None);
        assert_eq!(dropped, vec!["live".to_string()]);
        assert!(
            !dropped.iter().any(String::is_empty),
            "an empty mask is not a mask"
        );

        // The binding's credential joins the list, and an empty one does not.
        let bound = secrets(
            &["launch-token-x".to_string()],
            Some(&RuntimeBinding {
                node_id: "node-7".to_string(),
                node_credential: "node-credential-y".to_string(),
            }),
        );
        assert_eq!(bound.len(), 2);
        assert!(bound.contains(&"node-credential-y".to_string()));
        let empty_credential = secrets(
            &[],
            Some(&RuntimeBinding {
                node_id: "node-7".to_string(),
                node_credential: String::new(),
            }),
        );
        assert!(empty_credential.is_empty(), "{empty_credential:?}");
    }

    /// The Agent's fields: what it said is named, what it did not say is absent,
    /// and the profile list is a count.
    #[test]
    fn the_agent_fields_are_what_it_answered_and_never_the_profile_list() {
        let fields = agent_fields(&a_status(vec!["profile-secret-1", "profile-secret-2"]));

        assert_eq!(fields["status"], "running");
        assert_eq!(fields["agent_version"], "0.2.2");
        assert_eq!(fields["pending_result_count"], "2");
        assert_eq!(
            fields["bit_profile_count"], "2",
            "the count is what a reader needs"
        );
        assert!(
            !fields.contains_key("current_task_id"),
            "the Agent did not give one, so there is no key: {fields:?}"
        );
        assert!(!fields.contains_key("current_task_progress"), "{fields:?}");
        assert!(
            !fields.contains_key("bit_profile_ids"),
            "the ids are counted, not carried: {fields:?}"
        );
        // The control, and the stronger claim: the ids are not anywhere in the
        // document, not merely absent under their own key.
        let rendered = format!("{fields:?}");
        for id in ["profile-secret-1", "profile-secret-2"] {
            assert!(!rendered.contains(id), "{id} reached the bundle");
        }

        // The other direction: a field that *is* set appears.
        assert_eq!(fields["node_id"], "node-7");
        assert_eq!(fields["main_user_id"], "user-42");
    }
}

/// A one-off probe against this machine, run by hand and not part of the suite.
///
/// ```text
/// cargo test --offline commands::diagnostic::tests::probe_real_machine -- --nocapture
/// ```
///
/// It reads the two real log trees through the same resolvers a launch uses,
/// builds a real archive into a **scratch** directory (never `~/Downloads`: a
/// probe that littered the operator's machine would be doing the thing it is
/// checking), reads the archive back, and enumerates every entry. It is
/// `#[ignore]`, so `bash scripts/test.sh` never runs it.
#[cfg(test)]
mod probe {
    use super::tests::{host, read_back};
    use super::*;
    use crate::app_paths;
    use crate::config::Environment;

    #[test]
    #[ignore = "a probe, run by hand: cargo test --offline probe_real_machine -- --ignored --nocapture"]
    fn probe_real_machine() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let system = crate::system_paths::SystemPaths::from_parts(
            crate::system_paths::Platform::Darwin,
            home.clone(),
            None,
            manifest.to_path_buf(),
        );
        let paths = app_paths::resolve(&system, Environment::Development, manifest)
            .expect("the development layout");
        let agent_logs =
            crate::logging::paths::agent_directory(&system, None).expect("the Agent's tree");
        let base = std::env::temp_dir().join(format!("wt-media-t07-probe-{}", std::process::id()));
        // A scratch home with a `Downloads`, so the probe writes where the
        // export would on a normal machine and not into this crate's tree.
        std::fs::create_dir_all(base.join("Downloads")).expect("the scratch destination");

        let resolved = crate::commands::storage::Resolved {
            paths: paths.clone(),
            agent_logs: agent_logs.clone(),
        };
        let moment = SystemTime::now();
        let binding = Some(RuntimeBinding {
            node_id: "probe-node".to_string(),
            node_credential: "probe-node-credential-value".to_string(),
        });
        let result = export(
            &resolved,
            &host(vec!["probe-launch-token-value".to_string()]),
            "probe".to_string(),
            false,
            binding.as_ref(),
            AgentFacts::Unreachable {
                error: "the probe does not ask".to_string(),
            },
            Some(&base),
            moment,
        );
        let (written, logs, failures, created_at) = match result {
            Ok(ok) => ok,
            Err(error) => {
                println!("PROBE export failed: {error}");
                return;
            }
        };

        println!("PROBE created_at={created_at}");
        println!("PROBE desktop tree={}", paths.logs.display());
        println!("PROBE agent   tree={}", agent_logs.display());
        println!(
            "PROBE archive={} bytes={} sha256={}",
            written.path.display(),
            written.bytes,
            written.sha256
        );
        println!(
            "PROBE entries={} omitted={}",
            logs.entries.len(),
            logs.omitted.len()
        );
        for entry in &logs.entries {
            println!(
                "PROBE   {} text={} source_bytes={} truncated={}",
                entry.name,
                entry.text.len(),
                entry.source_bytes,
                entry.truncated
            );
        }
        for omission in &logs.omitted {
            println!(
                "PROBE   OMITTED {} ({})",
                omission.name,
                omission.reason.as_str()
            );
        }
        println!(
            "PROBE failures source={:?} records={} truncated={}",
            failures.source,
            failures.records.len(),
            failures.truncated
        );

        // The credential scan, on the bytes of the artifact itself. The
        // denominator is reported with the count, and the pattern is first proved
        // to match something: a scan that found nothing because it matches
        // nothing is not a result.
        let bytes = std::fs::read(&written.path).expect("the archive bytes");
        // All five markers, so "the pattern emits nothing" cannot be true by
        // construction: a control that only exercises two of the five proves
        // nothing about the other three.
        let control = "token=a password=b client_secret=c authorization: d cookies: e";
        let pattern = |text: &str| {
            let lower = text.to_ascii_lowercase();
            [
                "token=",
                "password=",
                "client_secret",
                "authorization:",
                "cookies:",
            ]
            .iter()
            .filter(|needle| lower.contains(**needle))
            .count()
        };
        println!(
            "PROBE scan control: the pattern matches {}/5 markers in a synthetic string",
            pattern(control)
        );
        let text = String::from_utf8_lossy(&bytes);
        println!(
            "PROBE scan of the archive: {} marker hits over {} bytes, {} entries",
            pattern(&text),
            bytes.len(),
            logs.entries.len()
        );
        for needle in ["probe-launch-token-value", "probe-node-credential-value"] {
            let found = bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes());
            println!("PROBE   {needle}: present={found}");
        }

        let entries = read_back(&written.path);
        println!("PROBE read-back {} entries:", entries.len());
        for (name, body) in &entries {
            println!("PROBE   {name} {} bytes", body.len());
        }
        println!(
            "PROBE checksum file: {}",
            std::fs::read_to_string(&written.checksum_path)
                .unwrap_or_default()
                .trim()
        );
        for (name, body) in &entries {
            if name == "summary.json" || name == "manifest.txt" {
                println!("PROBE ---- {name} ----");
                println!("{}", String::from_utf8_lossy(body));
            }
        }
        std::fs::remove_dir_all(&base).ok();
    }
}
