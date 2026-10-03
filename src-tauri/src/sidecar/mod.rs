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
#[cfg(unix)]
use rustix::io::Errno;
#[cfg(unix)]
use rustix::process::{kill_process, test_kill_process, Pid, Signal};
use tauri::{AppHandle, Runtime};
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

pub mod drain;
pub mod integrity;
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
        (
            "WT_MEDIA_LOCAL_API_HOST".to_string(),
            config.agent.host.clone(),
        ),
        (
            "WT_MEDIA_LOCAL_API_PORT".to_string(),
            config.agent.port.to_string(),
        ),
        (
            "WT_MEDIA_AGENT_RUNTIME_TOKEN".to_string(),
            token.expose().to_string(),
        ),
        // The sidecar's task loops are gated on this and default to off, so
        // without it the app ships an Agent that serves status and claims
        // nothing. It reaches only the bundled sidecar in practice: the Python
        // fallback assembles `local_api.server`, which starts no loops at all
        // and stays a status surface whichever way this is set.
        ("WT_MEDIA_AGENT_RUN_RUNNER".to_string(), "true".to_string()),
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
/// The bundled path is checked before it is taken: `integrity` compares the
/// sidecar against the `sidecar-manifest.json` the package carries, and a
/// disagreement **ends the start** rather than falling through — see `Attempt`.
/// A sidecar with no record at all is only refused when this launch came out of a
/// bundle, which is why a developer's tree still starts (the table in
/// `integrity`'s header is the whole rule).
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
    const FALLBACK: &str = "started";

    let vars = environment(config, token);
    let with_vars = |command: tauri_plugin_shell::process::Command| {
        command.envs(
            vars.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
    };

    let attempt = match integrity::Location::of(app) {
        Ok(location) => spawn_verified(app, &location, &vars, log),
        // Nowhere to look for a sidecar at all. The same arm as a spawn that
        // failed, so a debug build's Python opt-in still covers it — this is not
        // a refusal, it is the absence of a package to refuse.
        Err(_) => Attempt::Unavailable,
    };

    if let Attempt::Started(label, child) = attempt {
        return Ok((label, child));
    }
    if may_fall_back(&attempt, allow_python_fallback) {
        let shell = app.shell();
        let (events, child) = with_vars(shell.command("python3").args(FALLBACK_ARGS))
            .spawn()
            .map_err(|e| format!("agent launch failed: {}", e))?;
        drain::follow(events, log.clone(), FALLBACK);
        return Ok((FALLBACK, child));
    }
    Err(match attempt {
        Attempt::Refused(reason) => reason,
        _ => "未找到或无法启动随应用提供的 Local Agent。请重新安装完整的起飞安装包。".into(),
    })
}

/// Whether a bundled attempt that did not start may be answered by the Python
/// path.
///
/// Pure and separate on purpose. This is the one place where a **refusal** — "the
/// sidecar is not what the package records" — could be quietly converted into
/// "start something else instead", which is the single response that hides a
/// mismatched package: the app would come up, and every later call would be
/// answered by an Agent nobody checked. So `Refused` is terminal whatever
/// `allow_python_fallback` says, and the fallback is for
/// [`Attempt::Unavailable`] alone.
///
/// A function rather than a `match` arm because in a release build the two arms
/// are indistinguishable by construction (`development.python_fallback` is
/// rejected in production, `config.rs`), which is exactly the kind of decision
/// that is otherwise only ever read.
fn may_fall_back(attempt: &Attempt, allow_python_fallback: bool) -> bool {
    allow_python_fallback && matches!(attempt, Attempt::Unavailable)
}

/// What taking the bundled path came to.
///
/// Three, not two, because "there is no sidecar here" and "this sidecar is not
/// the one the package records" call for opposite answers from [`start`]: the
/// first is what a developer's tree looks like and the fallback exists for it,
/// the second is never something to work around.
pub(crate) enum Attempt {
    /// Running, under this label, with the handle to it.
    Started(&'static str, CommandChild),
    /// No usable bundled sidecar: absent, unspawnable, or its path unresolvable.
    Unavailable,
    /// The sidecar and the package's record of it disagree. The reason is the
    /// caller's to report.
    Refused(String),
}

/// Check the sidecar at `location`, then start **that file**.
///
/// The path comes in as an argument and is used for both halves, which is the
/// whole reason this is a separate function from [`start`]: the verified bytes and
/// the spawned bytes are the same bytes by construction, because there is only one
/// resolution. `start` fills the argument in with [`integrity::Location::of`]; the
/// tests fill it in with a location of their own, and then check what actually
/// happens to a process rather than what the code appears to do.
///
/// Spawning the resolved path instead of the plugin's `sidecar(name)` is the same
/// spawn — `Shell::sidecar` *is* `command(relative_command_path(name))`
/// (`tauri-plugin-shell-2.3.5/src/lib.rs:67`, `:181`) — with the path kept in hand.
pub(crate) fn spawn_verified<R: Runtime>(
    app: &AppHandle<R>,
    location: &integrity::Location,
    vars: &[(String, String)],
    log: &SidecarLog,
) -> Attempt {
    const SIDECAR: &str = "sidecar_started";

    let checked = match location.check() {
        Ok(checked) => checked,
        Err(reason) => return Attempt::Refused(reason),
    };
    integrity::note(&checked, location);

    let command = app.shell().command(location.sidecar()).envs(
        vars.iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    match command.spawn() {
        Ok((events, child)) => {
            drain::follow(events, log.clone(), SIDECAR);
            Attempt::Started(SIDECAR, child)
        }
        Err(_) => Attempt::Unavailable,
    }
}

/// Turn the pid a handle reports into something `kill(2)` may be pointed at.
///
/// `Pid::from_raw(0)` is `None`, and that is not a formality: `kill(0, sig)`
/// means "every process in my process group", so a zero reaching the kernel is
/// not a pid that cannot be signalled — it is a request to signal everything
/// this app is attached to. Refused here rather than passed on.
#[cfg(unix)]
fn as_pid(pid: u32) -> Result<Pid, String> {
    i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| format!("agent pid {pid} 不是一个可以发信号的进程号"))
}

/// Ask the sidecar to stop: `SIGTERM`, and nothing more.
///
/// The counterpart of [`force`], and the first half of the Unix exit protocol
/// (CHG-059 T-03). `SIGTERM` is what the Agent answers — `local_api.server.serve`
/// installs a handler for it, stops accepting and waits for the requests already
/// in flight — so once this returns `Ok`, the process has been asked to leave by
/// its own exit path rather than killed outright.
///
/// **This returning `Ok` is not "it stopped."** `kill(2)` succeeds once the
/// signal is queued; whether the process does anything about it is a different
/// question, and a process that ignores `SIGTERM` answers this call just as
/// successfully as one that obeys it. The answer is [`alive`], and the deadline
/// belongs to the caller.
///
/// An `Err` here is usually not a failure of the stop: it is `ESRCH`, the
/// process already being gone — which is an outcome the caller should read as
/// "nothing to wait for", not as "the ask did not work".
#[cfg(unix)]
pub fn ask(pid: u32) -> Result<(), String> {
    kill_process(as_pid(pid)?, Signal::TERM).map_err(|e| format!("agent stop failed: {}", e))
}

/// Windows has no pid-only equivalent of SIGTERM. Return an `Err` rather than
/// pretending to have asked gracefully; the caller treats this as non-fatal and
/// still uses the held `CommandChild` for the deadline force stop.
#[cfg(windows)]
pub fn ask(pid: u32) -> Result<(), String> {
    Err(format!(
        "agent pid {pid} has no graceful Windows stop; the force path will run"
    ))
}

/// Is there still a process at `pid`?
///
/// On Unix this is `kill(pid, 0)`. On Windows it opens the process for limited
/// query access and checks that the exit code is still `STILL_ACTIVE`. Used
/// between the ask and the deadline, so the reading is "it obeyed" rather than
/// "enough time passed".
///
/// **Known limit, and it is the caller's to bound**: a pid is only unique among
/// the processes alive at one moment. If this one exits and the kernel hands its
/// number to a new process inside the grace window, this says `true` about a
/// stranger, and the deadline ends in [`force`] killing something Desktop never
/// started. Not defended against here — the window is seconds and the id space
/// is large — but it is why the caller's deadline is short and fixed.
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    let Ok(pid) = as_pid(pid) else {
        return false;
    };
    match test_kill_process(pid) {
        Ok(()) => true,
        Err(Errno::PERM) => true,
        _ => false,
    }
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return false;
    }

    // SAFETY: the handle comes from and is consumed by Win32. `exit_code` is a
    // plain out-parameter, and every successful open is closed on this path.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }

        let mut exit_code = 0;
        let readable = GetExitCodeProcess(process, &mut exit_code);
        CloseHandle(process);
        readable != 0 && exit_code == STILL_ACTIVE as u32
    }
}

/// Kill a running sidecar, without asking. Takes the handle by value because
/// `CommandChild::kill(self)` consumes it — the caller has already `take`n it
/// out of the managed state, so there is nothing left to hold.
///
/// `SIGKILL`, so the Agent runs no exit path at all. That is deliberate at the
/// two places this is called from: the deadline of the exit protocol, where the
/// ask has already been given its full window and ignored, and `discard`, where
/// the handle is known to be a leftover and there is nothing left to ask.
pub fn force(child: CommandChild) -> Result<String, String> {
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
    #[cfg(unix)]
    use std::io::{BufRead, BufReader};
    #[cfg(unix)]
    use std::process::{Child, Command, Stdio};
    #[cfg(unix)]
    use std::time::{Duration, Instant};

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

        assert_eq!(
            vars.get("WT_MEDIA_LOCAL_API_HOST").map(String::as_str),
            Some("127.0.0.1")
        );
        assert_eq!(
            vars.get("WT_MEDIA_LOCAL_API_PORT").map(String::as_str),
            Some("18765")
        );
        assert_eq!(
            vars.get("WT_MEDIA_AGENT_RUNTIME_TOKEN").map(String::as_str),
            Some(token.expose())
        );
        assert_eq!(vars.len(), 4, "nothing else is sent: {vars:?}");
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

    /// The Agent is told to run its task loops, not only to serve status.
    ///
    /// `run_runner` gates `start_task_loops` (`bootstrap/sidecar.py`) and the
    /// config the package ships keeps it false on purpose — "an Agent started by
    /// hand or by a health check must not begin claiming tasks". The other half
    /// of that rule is this variable: without it the bundled sidecar answers
    /// status and reports in, and every download queued against this machine
    /// sits `pending` forever next to a live executor. The environment is the
    /// only per-launch channel for it; the packaged config is not rewritten.
    #[test]
    fn the_agent_is_told_to_run_its_task_loops() {
        let config = config_with("127.0.0.1", 8765, None);
        let token = RuntimeToken::generate();

        assert_eq!(
            as_map(&environment(&config, &token))
                .get("WT_MEDIA_AGENT_RUN_RUNNER")
                .map(String::as_str),
            Some("true")
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

        let from_first = as_map(&environment(
            &config,
            crate::http::LocalAgentClient::new(&config, first.clone()).token(),
        ));
        let from_second = as_map(&environment(
            &config,
            crate::http::LocalAgentClient::new(&config, second.clone()).token(),
        ));

        assert_eq!(
            from_first
                .get("WT_MEDIA_AGENT_RUNTIME_TOKEN")
                .map(String::as_str),
            Some(first.expose())
        );
        assert_eq!(
            from_second
                .get("WT_MEDIA_AGENT_RUNTIME_TOKEN")
                .map(String::as_str),
            Some(second.expose())
        );
        assert_ne!(
            first.expose(),
            second.expose(),
            "the two cases must differ to mean anything"
        );
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
    /// A real process whose `SIGTERM` trap is **already installed**.
    ///
    /// `std::process` rather than the shell plugin, because `ask` and `alive`
    /// take a pid — that is the whole reason they are not tied to
    /// `CommandChild` — so this needs nothing from Tauri, and the control below
    /// is a statement about a real process rather than about a mock.
    ///
    /// Waiting for the ready line is load-bearing, and it was measured the hard
    /// way: the first version of these tests asked immediately after the spawn,
    /// and the child died of signal 15 with the trap never having run. A signal
    /// arriving before the trap tests the default disposition, not the ask —
    /// `sleep 0.2` in the loop then bounds how long answering may take, since a
    /// shell defers a trap until the foreground child returns.
    #[cfg(unix)]
    fn with_trap(trap: &str) -> Child {
        let mut child = Command::new("sh")
            .args([
                "-c",
                &format!("trap {trap} TERM; echo trap-set; while true; do sleep 0.2; done"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("a shell");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("a stdout pipe"))
            .read_line(&mut line)
            .expect("the ready line");
        assert!(
            line.contains("trap-set"),
            "the trap must be up first: {line:?}"
        );
        child
    }

    /// The ask is answered: a real process, a real signal, and it leaves.
    ///
    /// The control is at the top — `alive` says `true` before the ask — so the
    /// `false` afterwards is a change of state rather than something that held
    /// all along.
    #[test]
    #[cfg(unix)]
    fn an_ask_reaches_a_real_process_and_it_leaves() {
        let mut child = with_trap("'exit 0'");
        let pid = child.id();
        assert!(alive(pid), "the control: there is a process to ask");

        assert!(ask(pid).is_ok(), "the ask must reach it");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut status = None;
        while Instant::now() < deadline {
            // `try_wait` is the reap, and it is not incidental: an unreaped
            // process is still a process, so `kill(pid, 0)` answers `true` for a
            // zombie and `alive` would keep saying yes about a corpse. In
            // production the shell plugin's reader thread does this — it is how
            // `Terminated` exists at all — so this models the real sequence.
            match child.try_wait().expect("the exit status") {
                Some(exit) => {
                    status = Some(exit);
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }

        let status = status.expect("it must have left the ask");
        assert_eq!(
            status.code(),
            Some(0),
            "an exit code rather than a signal: the trap ran, so it answered"
        );
        assert!(!alive(pid), "and with it reaped there is nothing there");
    }

    /// A process that ignores the ask is **still there** afterwards.
    ///
    /// This is the reading the grace window exists for, and it is the control for
    /// the test above: without it, "`alive` went false after an ask" would hold
    /// just as well for an `alive` that always answers `false`.
    ///
    /// `trap '' TERM` sets the disposition to `SIG_IGN`, which `sleep` inherits
    /// across `exec`, so nothing in the tree reacts and only `SIGKILL` ends it —
    /// which is how this test cleans up.
    #[test]
    #[cfg(unix)]
    fn a_process_that_ignores_the_ask_is_still_alive() {
        let mut child = with_trap("''");
        let pid = child.id();

        assert!(
            ask(pid).is_ok(),
            "the signal is delivered — that is all it says"
        );

        std::thread::sleep(Duration::from_millis(300));
        assert!(alive(pid), "it ignores SIGTERM, so the ask changed nothing");
        assert!(
            child.try_wait().expect("the exit status").is_none(),
            "and it is not merely unreaped: it has not exited"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    /// A pid that is not a process id is refused, not signalled.
    ///
    /// `kill(0, sig)` means "every process in my process group", so a zero
    /// reaching the kernel is not a failed ask — it is an ask aimed at everything
    /// this app is attached to. The type cannot express it (`Pid::from_raw`
    /// returns `None` for zero) and these two functions are where that shows.
    #[test]
    #[cfg(unix)]
    fn a_pid_of_zero_is_refused_rather_than_signalled() {
        let refused = ask(0).expect_err("zero must never be signalled");
        assert!(
            refused.contains('0'),
            "the message must name the pid, and nothing else: {refused}"
        );
        assert!(!alive(0), "there is nothing at it to report as alive");
    }

    #[test]
    fn the_fallback_command_line_is_only_the_module_name() {
        assert_eq!(
            FALLBACK_ARGS.to_vec(),
            vec!["-m", "wt_media_agent.local_api.server"]
        );
        assert!(
            !FALLBACK_ARGS
                .iter()
                .any(|arg| arg.to_lowercase().contains("token")),
            "the token must never be an argument: {FALLBACK_ARGS:?}"
        );
    }

    /// A refusal is never answered by starting something else.
    ///
    /// The three arms are the whole truth table, and the first one is the one
    /// that matters: with the opt-in **on** — a debug build, which is where a
    /// refusal is otherwise indistinguishable from "the sidecar would not start"
    /// — a mismatched package still ends the start. The other two pin the
    /// fallback's real job so the first cannot be satisfied by removing it.
    #[test]
    fn a_refusal_is_never_answered_with_the_python_path() {
        assert!(
            !may_fall_back(&Attempt::Refused("校验失败".into()), true),
            "the opt-in must not reach a refusal: that is how a mismatched \
             package becomes a running app"
        );
        assert!(may_fall_back(&Attempt::Unavailable, true));
        assert!(
            !may_fall_back(&Attempt::Unavailable, false),
            "and the opt-in is still an opt-in"
        );
    }
}
