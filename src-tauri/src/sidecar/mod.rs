//! Starting and stopping the Local Agent sidecar.
//!
//! Both spawn paths live here so the command layer never holds a `CommandChild`
//! and never has to know which path produced it. The bundled sidecar is the only
//! path a release build can take; the Python fallback sits behind a debug-build
//! opt-in and is unreachable from a release bundle, so customers never need a
//! system Python install.
//!
//! The label each path returns is preserved verbatim from the command bodies
//! this was extracted from (`sidecar_started` / `started` / `already_running` /
//! `not_running`), because the Vue layer matches on them.

use crate::config::DesktopConfig;
use crate::state::SidecarLog;
use crate::token::RuntimeToken;
use tauri::{AppHandle, Runtime};
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

pub mod drain;
pub mod readiness;

/// The fallback's whole command line.
///
/// A constant, and pinned by a test, because the temptation here is to add
/// `--token <value>` — which would put the per-launch secret in `ps` for every
/// other user on the machine. The token travels in the environment
/// (`sidecar_main.py`'s header, and D-04 in this change's record).
const FALLBACK_ARGS: [&str; 2] = ["-m", "wt_media_agent.local_api.server"];

/// The variables Desktop hands the Agent.
///
/// The names are the **Agent's**, declared in its `runtime/config.py` field
/// table; this function is the Desktop half of that contract. Nothing checks the
/// two halves against each other at build time — a name that drifts on either
/// side is not a compile error, it is a sidecar that binds a different port than
/// the one the client calls, or one that never demands the token the client
/// sends.
pub fn environment(config: &DesktopConfig, token: &RuntimeToken) -> Vec<(String, String)> {
    let mut vars = vec![
        ("WT_MEDIA_LOCAL_API_HOST".to_string(), config.agent.host.clone()),
        ("WT_MEDIA_LOCAL_API_PORT".to_string(), config.agent.port.to_string()),
        (
            "WT_MEDIA_AGENT_RUNTIME_TOKEN".to_string(),
            token.expose().to_string(),
        ),
    ];
    // Omitted rather than sent empty — see the test below for why the difference
    // matters to the Agent.
    if let Some(data_dir) = &config.agent.data_dir {
        vars.push(("WT_MEDIA_AGENT_DATA_DIR".to_string(), data_dir.clone()));
    }
    vars
}

/// Start the Local Agent, returning the path label and the child handle.
///
/// A failed bundled-sidecar spawn is not reported: with the fallback enabled it
/// falls through to the Python path, and without it the caller gets the
/// "reinstall" message — which is the message that helps a user either way. The
/// spawn error itself is deliberately not surfaced, as before.
///
/// Both paths hand their event receiver to `drain`, so whichever one ran, its
/// output is readable from `log` afterwards — and both are given the same
/// environment (`environment`), applied through the single `spawn` helper below.
/// A variable that reached only one path would be a development launch that 401s
/// while production works, or the reverse, and neither shows up until a real
/// launch.
///
/// Takes the config and the token rather than a prepared list of pairs, so that
/// "where do these variables come from" cannot be answered differently at a call
/// site. The environment is *added* to the child's inherited one, not substituted
/// for it: the bundled sidecar is started by PyInstaller's bootstrap, which needs
/// `PATH` and `$HOME` to be intact.
///
/// Generic over the runtime, and only because of that: `Wry` is what ships, and
/// naming it here would make the one function that *spawns* the Agent reachable
/// only from a real app. `MockRuntime` (`tauri`'s `test` feature, a test-only
/// dependency) drives this same code, so a test can hold a real `CommandChild` —
/// which is what T-02's defect needs and what no fake could stand in for. No call
/// site changes: every one of them passes a `Wry` handle and infers it.
pub fn start<R: Runtime>(
    app: &AppHandle<R>,
    allow_python_fallback: bool,
    log: &SidecarLog,
    config: &DesktopConfig,
    token: &RuntimeToken,
) -> Result<(&'static str, CommandChild), String> {
    const SIDECAR: &str = "sidecar_started";
    const FALLBACK: &str = "started";

    let vars = environment(config, token);
    let with_vars = |command: tauri_plugin_shell::process::Command| {
        command.envs(vars.iter().map(|(key, value)| (key.as_str(), value.as_str())))
    };

    match app
        .shell()
        .sidecar("wt-media-agent")
        .map(|cmd| with_vars(cmd).spawn())
    {
        Ok(Ok((events, child))) => {
            drain::follow(events, log.clone(), SIDECAR);
            Ok((SIDECAR, child))
        }
        _ if allow_python_fallback => {
            let shell = app.shell();
            let (events, child) = with_vars(shell.command("python3").args(FALLBACK_ARGS))
                .spawn()
                .map_err(|e| format!("agent launch failed: {}", e))?;
            drain::follow(events, log.clone(), FALLBACK);
            Ok((FALLBACK, child))
        }
        _ => Err(
            "未找到或无法启动随应用提供的 Local Agent。请重新安装完整的 WT Media 安装包。".into(),
        ),
    }
}

/// Kill a running sidecar. Takes the handle by value because
/// `CommandChild::kill(self)` consumes it — the caller has already `take`n it
/// out of the managed state, so there is nothing left to hold.
pub fn stop(child: CommandChild) -> Result<String, String> {
    child
        .kill()
        .map(|_| "stopped".into())
        .map_err(|e| format!("agent stop failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use std::collections::BTreeMap;

    fn config_with(host: &str, port: u16, data_dir: Option<&str>) -> DesktopConfig {
        let text = PRODUCTION_TOML
            .replace("host = \"127.0.0.1\"", &format!("host = {:?}", host))
            .replace("port = 8765", &format!("port = {}", port))
            .replace(
                "# data_dir = \"/path/to/agent-data\"",
                &match data_dir {
                    Some(dir) => format!("data_dir = {:?}", dir),
                    None => String::new(),
                },
            );
        load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config")
    }

    fn as_map(vars: &[(String, String)]) -> BTreeMap<String, String> {
        vars.iter().cloned().collect()
    }

    /// The Agent is told where to listen and what to demand.
    ///
    /// Values, not just key names: a function that returned the right four keys
    /// with the wrong values would look identical from the outside and produce a
    /// sidecar that listens somewhere nobody is calling.
    #[test]
    fn the_agent_is_told_where_to_listen_and_what_to_demand() {
        let config = config_with("127.0.0.1", 18765, None);
        let token = RuntimeToken::generate();

        let vars = as_map(&environment(&config, &token));

        assert_eq!(vars.get("WT_MEDIA_LOCAL_API_HOST").map(String::as_str), Some("127.0.0.1"));
        assert_eq!(vars.get("WT_MEDIA_LOCAL_API_PORT").map(String::as_str), Some("18765"));
        assert_eq!(
            vars.get("WT_MEDIA_AGENT_RUNTIME_TOKEN").map(String::as_str),
            Some(token.expose())
        );
        assert_eq!(vars.len(), 3, "nothing else is sent: {vars:?}");
    }

    /// An unset data directory is **omitted**, not sent empty.
    ///
    /// The Agent's own rule is that an unset `WT_MEDIA_AGENT_DATA_DIR` means
    /// "resolve your own default" (`runtime/paths.py`); an empty value would be
    /// a path that does not exist. Both directions, because a function that
    /// always omitted it would pass a one-sided test.
    #[test]
    fn an_unset_data_dir_is_omitted_and_a_set_one_is_passed_through() {
        let token = RuntimeToken::generate();

        let unset = config_with("127.0.0.1", 8765, None);
        assert!(!as_map(&environment(&unset, &token)).contains_key("WT_MEDIA_AGENT_DATA_DIR"));

        let set = config_with("127.0.0.1", 8765, Some("/Users/example/agent-data"));
        assert_eq!(
            as_map(&environment(&set, &token))
                .get("WT_MEDIA_AGENT_DATA_DIR")
                .map(String::as_str),
            Some("/Users/example/agent-data")
        );
    }

    /// The token handed to the Agent is the **same one the requests carry**.
    ///
    /// This is the coupling the whole commit exists for: `_check_auth` on the
    /// Agent side demands `Bearer <its token>` on every call once it has one, so
    /// an Agent told one secret and challenged with another is a sidecar that
    /// answers 401 to everything. Two clients with two different tokens, so the
    /// assertion cannot hold by both sides reading the same constant.
    #[test]
    fn the_token_the_agent_is_told_is_the_one_the_requests_carry() {
        let config = config_with("127.0.0.1", 8765, None);
        let first = RuntimeToken::generate();
        let second = RuntimeToken::generate();

        let from_first = as_map(&environment(&config, crate::http::LocalAgentClient::new(&config, first.clone()).token()));
        let from_second = as_map(&environment(&config, crate::http::LocalAgentClient::new(&config, second.clone()).token()));

        assert_eq!(
            from_first.get("WT_MEDIA_AGENT_RUNTIME_TOKEN").map(String::as_str),
            Some(first.expose())
        );
        assert_eq!(
            from_second.get("WT_MEDIA_AGENT_RUNTIME_TOKEN").map(String::as_str),
            Some(second.expose())
        );
        assert_ne!(first.expose(), second.expose(), "the two cases must differ to mean anything");
    }

    /// D-04: the secret travels in the environment and nowhere else.
    ///
    /// `ps` shows every user on the machine the full command line of a process
    /// that is not theirs, so the argv is pinned: adding `--token <value>` here
    /// has to be a deliberate edit to this line.
    ///
    /// Compared as slices rather than as arrays of a fixed length. The direct
    /// form (`assert_eq!(FALLBACK_ARGS, [..])`) makes appending an argument a
    /// *type* error instead of a test failure — which sounds like a stronger
    /// guard and is really only an accidental one: it would send the next person
    /// to fix the test's array length, and the assertion that actually carries
    /// this rule is the one below, which does not care how many arguments there
    /// are.
    #[test]
    fn the_fallback_command_line_is_only_the_module_name() {
        assert_eq!(
            FALLBACK_ARGS.to_vec(),
            vec!["-m", "wt_media_agent.local_api.server"]
        );
        assert!(
            !FALLBACK_ARGS.iter().any(|arg| arg.to_lowercase().contains("token")),
            "the token must never be an argument: {FALLBACK_ARGS:?}"
        );
    }
}
