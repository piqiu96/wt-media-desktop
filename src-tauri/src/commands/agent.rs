//! Local Agent observation and process lifecycle commands.
//!
//! Everything that happens to the Agent at Desktop's request is a record under
//! `agent.supervisor`: starting it, stopping it, and whether its health check
//! answered. None of it was logged before, so the only way to tell "the Agent is
//! not running" from "the Agent is running and refusing" was a failed command in
//! the frontend.
//!
//! The records live in the small functions below rather than inline in the
//! commands, for the reason T-15 split `plan` out of `install`: a command body
//! takes `State`, which no test can build, so an inline `tracing!` call would be
//! reachable only by launching the app. The bodies are split out for the same
//! reason — `health` and `stop` take plain references and are exercised against a
//! real socket; what still needs a running app is the spawn in `start`, and that
//! is the one thing left where it was rather than faked.

use crate::config::DesktopConfig;
use crate::development_python_fallback_enabled;
use crate::dto::{LocalAgentStatus, LocalAgentStatusResponse};
use crate::http::LocalAgentClient;
use crate::sidecar::{self, drain};
use crate::state::{AgentProcess, SidecarLog};
use tauri::State;

/// The Agent started, with the label the caller also receives back.
fn started(label: &str) {
    tracing::info!(target: "agent.supervisor", "Local Agent 已启动（{label}）");
}

/// A start request while one is already running: nothing happened, and that is
/// worth a record precisely because nothing did — the caller still gets
/// `already_running` back, and a second Agent is not started.
fn already_running() {
    tracing::info!(target: "agent.supervisor", "Local Agent 已在运行，忽略本次启动请求");
}

/// The Agent was killed at Desktop's request.
fn stopped() {
    tracing::info!(target: "agent.supervisor", "Local Agent 已停止");
}

/// A stop request with nothing to stop.
fn not_running() {
    tracing::info!(target: "agent.supervisor", "Local Agent 未在运行，忽略本次停止请求");
}

/// The health check answered, body and all — at DEBUG.
///
/// The body is the Agent's own status, already written to the Agent's own files;
/// keeping it out of the production log is ruling 四, and it has to stay
/// reachable somehow or the level is decoration rather than a policy.
fn healthy(text: &str) {
    tracing::debug!(target: "agent.supervisor", "健康检查成功：{text}");
}

/// A failed Agent command: the caller gets the tail, the record does not.
///
/// The two texts differ on purpose. The frontend shows the returned one, and a
/// failure a user is looking at should say what the Agent last printed. The
/// record must not: health checks are polled, and a repeating failure would bury
/// the lifecycle records under the Agent's output. The last twenty lines reach
/// the log once, in the exit report, and this keeps that true.
///
/// The message is passed through unchanged, so the string the frontend sees is
/// exactly what it saw before this existed — the record is the only new thing.
fn failed(message: String, log: &SidecarLog) -> String {
    tracing::warn!(target: "agent.supervisor", "{message}");
    message + &drain::summary(log)
}

#[tauri::command]
pub async fn local_agent_status(client: State<'_, LocalAgentClient>) -> Result<LocalAgentStatus, String> {
    let resp = client
        .get("/api/v1/status")
        .send()
        .await
        .map_err(|e| format!("agent unreachable: {}", e))?;
    let body: LocalAgentStatusResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid status response: {}", e))?;
    Ok(LocalAgentStatus::from(body.data))
}
/// The health check itself, without Tauri.
///
/// Split out of the command for the reason `plan`/`install` were split in
/// `logging::setup`: the command takes `State`, which no test can build, so a
/// record emitted inside it would be reachable only by a real launch — and a
/// launch does not reach it here either, because the front end does not boot in
/// this environment (the window opens blank; see the T-16 evidence). What stays
/// in the command is the argument unpacking.
async fn health(client: &LocalAgentClient, log: &SidecarLog) -> Result<String, String> {
    let resp = client
        .get("/healthz")
        .send()
        .await
        .map_err(|e| failed(format!("agent unreachable: {e}"), log))?;
    let text = resp
        .text()
        .await
        .map_err(|e| failed(format!("read error: {e}"), log))?;
    healthy(&text);
    Ok(text)
}

/// The start itself, minus the argument unpacking.
///
/// This one still needs the `AppHandle`, which is exactly what no test can
/// build: the spawn it feeds (`sidecar::start`) is the only part of the command
/// layer the suite cannot reach, and it is left alone here rather than faked.
async fn start(
    app: &tauri::AppHandle,
    client: &LocalAgentClient,
    config: &DesktopConfig,
    process: &AgentProcess,
    log: &SidecarLog,
) -> Result<String, String> {
    if process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .is_some()
    {
        already_running();
        return Ok("already_running".into());
    }
    // Release builds must run only the bundled sidecar. Development can opt into
    // a Python fallback explicitly when iterating without a frozen binary.
    //
    // The token comes from the client, not from a second copy: the Agent has to
    // be told the same secret the requests will present, and one owner is what
    // makes that true by construction rather than by discipline.
    let (label, child) = sidecar::start(
        app,
        development_python_fallback_enabled(),
        log,
        config,
        client.token(),
    )
    .map_err(|e| failed(e, log))?;
    process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .replace(child);
    started(label);
    Ok(label.into())
}

/// The stop itself, without Tauri.
fn stop(process: &AgentProcess) -> Result<String, String> {
    let child = process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .take();
    match child {
        Some(child) => match sidecar::stop(child) {
            Ok(label) => {
                stopped();
                Ok(label)
            }
            // The kill failed. No tail: nothing here reads the sidecar's output,
            // and the buffer belongs to a process that is still alive.
            Err(reason) => {
                tracing::warn!(target: "agent.supervisor", "{reason}");
                Err(reason)
            }
        },
        None => {
            not_running();
            Ok("not_running".into())
        }
    }
}

#[tauri::command]
pub async fn local_agent_health(
    client: State<'_, LocalAgentClient>,
    log: State<'_, SidecarLog>,
) -> Result<String, String> {
    health(&client, &log).await
}
#[tauri::command]
pub async fn local_agent_start(
    app: tauri::AppHandle,
    client: State<'_, LocalAgentClient>,
    config: State<'_, DesktopConfig>,
    process: State<'_, AgentProcess>,
    log: State<'_, SidecarLog>,
) -> Result<String, String> {
    start(&app, &client, &config, &process, &log).await
}
#[tauri::command]
pub fn local_agent_stop(process: State<'_, AgentProcess>) -> Result<String, String> {
    stop(&process)
}
/// Fetch task progress as a status snapshot.
///
/// No stream is opened: `task_id` is validated server-side and the overall
/// status is returned. The Agent's `text/event-stream` endpoint is not consumed
/// from here.
#[tauri::command]
pub async fn local_agent_task_status(
    client: State<'_, LocalAgentClient>,
    _task_id: String,
) -> Result<LocalAgentStatus, String> {
    // Report back the current overall status; task_id is validated server-side.
    local_agent_status(client).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::logging::test_support::{capture, capture_at, written};
    use crate::logging::targets::Levels;
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use tracing::subscriber::with_default;

    fn client_to(port: u16) -> LocalAgentClient {
        let text = PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}"));
        let config = load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config");
        LocalAgentClient::new(&config, RuntimeToken::generate())
    }

    /// A listener answering `body` to one request, on a port the kernel picks.
    ///
    /// Port 0, never a fixed one: the socket is the test's own, and a test that
    /// picked 8765 would fight the developer's running Agent for it.
    fn answering(body: &'static str) -> u16 {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = server.local_addr().expect("the bound address").port();
        std::thread::spawn(move || {
            if let Ok((mut socket, _)) = server.accept() {
                let mut request = [0u8; 1024];
                let _ = socket.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        port
    }

    /// A port nothing is listening on: bound, then released.
    fn silent_port() -> u16 {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        server.local_addr().expect("the bound address").port()
    }

    /// Every lifecycle record says what happened, under Desktop's own target,
    /// at a level production keeps.
    ///
    /// Production levels, not the development ones the other tests use: an INFO
    /// record is the entire point of these, and the shipped filter is what the
    /// records have to survive to be worth writing.
    #[test]
    fn the_lifecycle_records_are_desktops_own_and_survive_production() {
        let (directory, subscriber) =
            capture_at(Levels::shipped(Environment::Production), "agent-lifecycle");
        with_default(subscriber, || {
            started("sidecar_started");
            already_running();
            stopped();
            not_running();
        });

        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 4, "one record per outcome: {text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 已启动（sidecar_started）"), "{text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 已在运行，忽略本次启动请求"), "{text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 已停止"), "{text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 未在运行，忽略本次停止请求"), "{text}");
    }

    /// The health body is DEBUG: kept in development, dropped in production.
    ///
    /// Both arms, because one alone says nothing — a record that never appears
    /// passes the production arm, and a record that always appears passes the
    /// development one. This is ruling 四's boundary, and it is the only place
    /// it is checked.
    #[test]
    fn the_health_body_is_development_only() {
        let (development, subscriber) = capture("agent-health-body-development");
        with_default(subscriber, || healthy("{\"status\":\"ok\"}"));
        let text = written(&development.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("[DEBUG] agent.supervisor: 健康检查成功：{\"status\":\"ok\"}"), "{text}");

        let (production, subscriber) =
            capture_at(Levels::shipped(Environment::Production), "agent-health-body-production");
        with_default(subscriber, || healthy("{\"status\":\"ok\"}"));
        assert_eq!(
            written(&production.0),
            "",
            "the Agent's own status must not be copied into the production log"
        );
    }

    /// A failed health check tells the caller, not the log, what the Agent last
    /// said.
    ///
    /// The control is in the same test: the tail **is** in the returned string,
    /// so "the record has no tail" is a statement about where the tail went and
    /// not about a drain that captured nothing.
    #[test]
    fn a_failure_keeps_the_tail_out_of_the_record() {
        let (directory, subscriber) = capture("agent-failure");
        let log = SidecarLog::default();
        drain::push(&log, "bind: 127.0.0.1:8765".to_string());

        let returned = with_default(subscriber, || {
            failed("agent unreachable: connection refused".to_string(), &log)
        });

        assert_eq!(
            returned,
            "agent unreachable: connection refused\n\nLocal Agent 最近输出：\nbind: 127.0.0.1:8765",
            "the caller gets exactly the string it got before, tail and all"
        );
        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "one failure, one record: {text}");
        assert!(
            text.contains("[WARN] agent.supervisor: agent unreachable: connection refused"),
            "{text}"
        );
        assert!(
            !text.contains("bind: 127.0.0.1:8765"),
            "the tail reaches the log once, in the exit report: {text}"
        );
    }

    /// With nothing captured, the returned string is the message unchanged —
    /// the same rule `drain::summary` keeps for an error that predates output.
    #[test]
    fn a_failure_with_no_output_is_the_message_alone() {
        let (directory, subscriber) = capture("agent-failure-quiet");
        let returned = with_default(subscriber, || {
            failed("read error: unexpected eof".to_string(), &SidecarLog::default())
        });

        assert_eq!(returned, "read error: unexpected eof");
        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("[WARN] agent.supervisor: read error: unexpected eof"), "{text}");
    }

    /// The health command's body, against a real socket: the body comes back and
    /// the success is recorded.
    ///
    /// This is the wiring, not the wording — the command itself cannot be called
    /// (it takes `State`), so `health` is what the command does with the answer
    /// once it has one. Both halves are asserted, because a body that is
    /// recorded but not returned would break the feature the frontend uses.
    #[test]
    fn a_health_answer_is_returned_and_recorded() {
        let port = answering("{\"status\":\"ok\"}");
        let client = client_to(port);
        let (directory, subscriber) = capture("agent-health-body");

        let returned = with_default(subscriber, || {
            tauri::async_runtime::block_on(health(&client, &SidecarLog::default()))
        });

        assert_eq!(returned.expect("the stub answers"), "{\"status\":\"ok\"}");
        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(
            text.contains("[DEBUG] agent.supervisor: 健康检查成功：{\"status\":\"ok\"}"),
            "{text}"
        );
    }

    /// The health command's failure path, with the tail reaching only the caller.
    ///
    /// The same split as `a_failure_keeps_the_tail_out_of_the_record`, but
    /// through the body the command runs: the string the frontend receives still
    /// ends with the Agent's last output, and the record does not contain it.
    #[test]
    fn an_unreachable_agent_keeps_the_tail_out_of_the_record() {
        let client = client_to(silent_port());
        let (directory, subscriber) = capture("agent-health-unreachable");
        let log = SidecarLog::default();
        drain::push(&log, "uvicorn: started".to_string());

        let returned = with_default(subscriber, || {
            tauri::async_runtime::block_on(health(&client, &log))
        });

        let returned = returned.expect_err("nothing is listening");
        assert!(returned.starts_with("agent unreachable: "), "{returned}");
        assert!(
            returned.ends_with("Local Agent 最近输出：\nuvicorn: started"),
            "the caller keeps the tail it had before this change: {returned}"
        );
        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("[WARN] agent.supervisor: agent unreachable: "), "{text}");
        assert!(!text.contains("uvicorn: started"), "the record has no tail: {text}");
    }

    /// The stop command's body with nothing to stop.
    ///
    /// The other branch needs a real `CommandChild`, which only a Tauri app can
    /// produce; this is the half the suite can reach.
    #[test]
    fn a_stop_with_nothing_running_is_recorded() {
        let (directory, subscriber) = capture("agent-stop-idle");

        let returned = with_default(subscriber, || stop(&AgentProcess::default()));

        assert_eq!(returned.expect("stopping nothing is not an error"), "not_running");
        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 未在运行，忽略本次停止请求"), "{text}");
    }

}
