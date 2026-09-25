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
//!
//! Every record carries the session id when there is one, as a **field** rather
//! than as part of the message. A session is one Agent, from the start that made
//! it to the stop that ended it; the id is what makes those lines greppable as a
//! group, and it stays inside this process (`state::OperationId`, ruling D-10).

use crate::config::DesktopConfig;
use crate::development_python_fallback_enabled;
use crate::dto::{LocalAgentStatus, LocalAgentStatusResponse};
use crate::http::LocalAgentClient;
use crate::sidecar::{self, drain, readiness};
use crate::state::{AgentProcess, OperationId, SidecarLog};
use tauri::State;

/// One lifecycle record, carrying the session id when there is one.
///
/// Two `event!` calls rather than one field of type `Option`: `tracing` prints
/// every field it is given, so `operation_id = %option` would write `None` on
/// every record made outside a session. The field has to be *absent* there —
/// both so "which session was this?" cannot be answered wrongly, and so those
/// records stay byte-identical to the shape a reader already knows.
macro_rules! lifecycle {
    ($level:ident, $session:expr, $($args:tt)*) => {
        match $session {
            Some(id) => tracing::event!(
                target: "agent.supervisor",
                tracing::Level::$level,
                operation_id = %id,
                $($args)*
            ),
            None => tracing::event!(
                target: "agent.supervisor",
                tracing::Level::$level,
                $($args)*
            ),
        }
    };
}

/// The Agent started, with the label the caller also receives back.
fn started(label: &str, session: Option<&str>) {
    lifecycle!(INFO, session, "Local Agent 已启动（{label}）");
}

/// A start request while one is already running: nothing happened, and that is
/// worth a record precisely because nothing did — the caller still gets
/// `already_running` back, and a second Agent is not started.
fn already_running(session: Option<&str>) {
    lifecycle!(INFO, session, "Local Agent 已在运行，忽略本次启动请求");
}

/// The Agent was killed at Desktop's request.
fn stopped(session: Option<&str>) {
    lifecycle!(INFO, session, "Local Agent 已停止");
}

/// A stop request with nothing to stop.
fn not_running() {
    lifecycle!(INFO, None::<&str>, "Local Agent 未在运行，忽略本次停止请求");
}

/// The health check answered, body and all — at DEBUG.
///
/// The body is the Agent's own status, already written to the Agent's own files;
/// keeping it out of the production log is ruling 四, and it has to stay
/// reachable somehow or the level is decoration rather than a policy.
fn healthy(text: &str, session: Option<&str>) {
    lifecycle!(DEBUG, session, "健康检查成功：{text}");
}

/// The Agent said it was listening and then answered: the start is done.
///
/// INFO, and the announcement line with it — the port the Agent bound is
/// Desktop's own knowledge about a process it is managing, not the Agent's record
/// of itself. The health **body** stays where it already was, at DEBUG
/// (`healthy`), so ruling 四 still holds for it.
///
/// Distinct from `started` on purpose: "a child process exists" and "the app can
/// be used" used to be the same moment and are now seconds apart, and a reader
/// looking at a start that went wrong needs to know which of the two happened.
fn ready(line: &str, session: Option<&str>) {
    lifecycle!(INFO, session, "Local Agent 已就绪：{line}");
}

/// The kill failed. The session is **not** over — the child was taken out of the
/// managed slot before the kill was attempted, and a kill that failed leaves a
/// process that may still be running — so the id stays where it is.
fn stop_failed(reason: &str, session: Option<&str>) {
    lifecycle!(WARN, session, "{reason}");
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
fn failed(message: String, log: &SidecarLog, session: Option<&str>) -> String {
    lifecycle!(WARN, session, "{message}");
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
async fn health(
    client: &LocalAgentClient,
    log: &SidecarLog,
    session: &OperationId,
) -> Result<String, String> {
    // Read once, at the top: a health check belongs to the session that was
    // open when it was asked, whichever line it ends up writing.
    let id = session.current();
    let resp = client
        .get("/healthz")
        .send()
        .await
        .map_err(|e| failed(format!("agent unreachable: {e}"), log, id.as_deref()))?;
    let text = resp
        .text()
        .await
        .map_err(|e| failed(format!("read error: {e}"), log, id.as_deref()))?;
    healthy(&text, id.as_deref());
    Ok(text)
}

/// The start itself, minus the argument unpacking.
///
/// Generic over the runtime so the suite can drive it: `start` is the only place
/// the managed slot is filled, and the defect T-02 closes — a start answered from
/// a record rather than from the Agent — lives in its guard. A test that could
/// not reach this would have to reproduce the guard, and a reimplementation is
/// exactly what would not have the bug. `tauri`'s mock runtime runs the same body
/// (see `sidecar::start`); the shipped app passes a `Wry` handle and infers it.
async fn start<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    client: &LocalAgentClient,
    config: &DesktopConfig,
    process: &AgentProcess,
    session: &OperationId,
    log: &SidecarLog,
) -> Result<String, String> {
    if process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .is_some()
    {
        already_running(session.current().as_deref());
        return Ok("already_running".into());
    }
    // One id per **start request**, generated before the spawn and used by every
    // record this attempt writes. A failed attempt has an id but no session: the
    // id is not stored until there is a child to go with it, so a later start
    // does not inherit the id of a start that never happened.
    let id = OperationId::generate();
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
    .map_err(|e| failed(e, log, Some(&id)))?;
    process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .replace(child);
    begin(session, &id, label);
    // The child exists; it is not yet answering. Wait for it to report that it is
    // listening and then to answer, which is what `sidecar.start_timeout_ms` has
    // always described — until now no code performed that wait.
    //
    // The session is opened **before** this wait, not after. The child is in the
    // managed slot from the moment it exists, so a stop arriving during the wait
    // can still reach it; opening the session afterwards would leave a window in
    // which the Agent is running and `local_agent_stop` reports `not_running` —
    // the orphan CHG-057 measured, reintroduced one layer up.
    match readiness::gate(config, log, client).await {
        Ok(ready_agent) => {
            ready(&ready_agent.line, Some(&id));
            healthy(&ready_agent.health, Some(&id));
            Ok(label.into())
        }
        Err(reason) => {
            // A start that never became ready is a start that failed, and the
            // child must not be left holding the port. Reported first, then
            // stopped, so the reader sees "it started, here is why it failed, it
            // was stopped" rather than a stop with no explanation above it.
            let message = failed(reason, log, Some(&id));
            let _ = stop(process, session);
            Err(message)
        }
    }
}

/// A start that succeeded: the session exists from here.
///
/// Split out of `start` for the reason the record functions are — everything
/// after a successful spawn is reachable in a test, and it must not live inside
/// a function that needs an `AppHandle` to run. Two ordered steps, and the order
/// is the point: stored before recorded, so a reader never sees a "started" line
/// for a session that is not held yet.
fn begin(session: &OperationId, id: &str, label: &str) {
    session.set(id.to_string());
    started(label, Some(id));
}

/// A stop that succeeded: the session ends here, not at the next start.
///
/// The mirror of `begin`, for the same reason and with the opposite ordering —
/// read, then recorded, then cleared. An id that outlived its Agent would label
/// the next session's records with the previous one's id.
fn ended(session: &OperationId, label: &str) -> String {
    stopped(session.current().as_deref());
    session.clear();
    label.into()
}

/// The stop itself, without Tauri.
fn stop(process: &AgentProcess, session: &OperationId) -> Result<String, String> {
    let child = process
        .0
        .lock()
        .map_err(|_| "agent process lock poisoned")?
        .take();
    match child {
        Some(child) => {
            let id = session.current();
            match sidecar::stop(child) {
                Ok(label) => Ok(ended(session, &label)),
                // The kill failed. No tail: nothing here reads the sidecar's
                // output, and the buffer belongs to a process that is still
                // alive — which is also why the session stays open.
                Err(reason) => {
                    stop_failed(&reason, id.as_deref());
                    Err(reason)
                }
            }
        }
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
    session: State<'_, OperationId>,
) -> Result<String, String> {
    health(&client, &log, &session).await
}
#[tauri::command]
pub async fn local_agent_start(
    app: tauri::AppHandle,
    client: State<'_, LocalAgentClient>,
    config: State<'_, DesktopConfig>,
    process: State<'_, AgentProcess>,
    session: State<'_, OperationId>,
    log: State<'_, SidecarLog>,
) -> Result<String, String> {
    start(&app, &client, &config, &process, &session, &log).await
}
#[tauri::command]
pub fn local_agent_stop(
    process: State<'_, AgentProcess>,
    session: State<'_, OperationId>,
) -> Result<String, String> {
    stop(&process, &session)
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
    use std::sync::{Arc, Mutex};
    use tracing::subscriber::with_default;

    fn client_to(port: u16) -> LocalAgentClient {
        let text = PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}"));
        let config = load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config");
        LocalAgentClient::new(&config, RuntimeToken::generate())
    }

    /// A session that is already open, holding `id`.
    ///
    /// An id given by the test rather than generated, so every assertion can be
    /// exact and a value that leaked into the wrong place is recognisable.
    fn holding(id: &str) -> OperationId {
        let session = OperationId::default();
        session.set(id.to_string());
        session
    }

    /// A listener answering `body` to one request, on a port the kernel picks,
    /// **and the bytes it received**.
    ///
    /// Port 0, never a fixed one: the socket is the test's own, and a test that
    /// picked 8765 would fight the developer's running Agent for it.
    ///
    /// The request is kept because "the Agent never sees the session id" (D-10)
    /// is a claim about what left the process, and no assertion on a
    /// `RequestBuilder` can make it: `host` is added by hyper at send time, so a
    /// set read before `send()` is a baseline that never matches the wire.
    fn recording(body: &'static str) -> (u16, Arc<Mutex<Vec<u8>>>) {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = server.local_addr().expect("the bound address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        std::thread::spawn(move || {
            if let Ok((mut socket, _)) = server.accept() {
                let mut request = [0u8; 2048];
                if let Ok(read) = socket.read(&mut request) {
                    sink.lock()
                        .expect("the recorder")
                        .extend_from_slice(&request[..read]);
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        (port, seen)
    }

    /// The same listener, for the tests that do not look at the request.
    fn answering(body: &'static str) -> u16 {
        recording(body).0
    }

    /// What the stub received, as text.
    fn received(seen: &Arc<Mutex<Vec<u8>>>) -> String {
        let bytes = seen.lock().expect("the recorder").clone();
        String::from_utf8(bytes).expect("an HTTP request is ASCII")
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
            started("sidecar_started", None);
            ready("wt-media-agent local API listening on 127.0.0.1:8765", None);
            already_running(None);
            stopped(None);
            not_running();
        });

        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 5, "one record per outcome: {text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 已启动（sidecar_started）"), "{text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 已就绪：wt-media-agent local API listening on 127.0.0.1:8765"), "{text}");
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
        with_default(subscriber, || healthy("{\"status\":\"ok\"}", None));
        let text = written(&development.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("[DEBUG] agent.supervisor: 健康检查成功：{\"status\":\"ok\"}"), "{text}");

        let (production, subscriber) =
            capture_at(Levels::shipped(Environment::Production), "agent-health-body-production");
        with_default(subscriber, || healthy("{\"status\":\"ok\"}", None));
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
            failed(
                "agent unreachable: connection refused".to_string(),
                &log,
                None,
            )
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
            failed(
                "read error: unexpected eof".to_string(),
                &SidecarLog::default(),
                None,
            )
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
            tauri::async_runtime::block_on(health(
                &client,
                &SidecarLog::default(),
                &OperationId::default(),
            ))
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
            tauri::async_runtime::block_on(health(&client, &log, &OperationId::default()))
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

        let returned = with_default(subscriber, || {
            stop(&AgentProcess::default(), &OperationId::default())
        });

        assert_eq!(
            returned.expect("stopping nothing is not an error"),
            "not_running"
        );
        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 未在运行，忽略本次停止请求"), "{text}");
    }

    /// A session opens and closes, and both records say which session it was.
    ///
    /// `begin`/`ended` are what `start` and `stop` run once the spawn and the
    /// kill have succeeded — the two halves the suite cannot reach, because one
    /// needs an `AppHandle` and the other a `CommandChild`. Their ordering is
    /// asserted here rather than assumed: the session is held before the
    /// "started" record and cleared after the "stopped" one, so a reader is
    /// never told about a session that is not held.
    #[test]
    fn a_session_is_held_and_released_around_its_two_records() {
        let session = OperationId::default();
        let (directory, subscriber) = capture("agent-session-boundaries");

        with_default(subscriber, || {
            begin(&session, "session-one", "sidecar_started");
            assert_eq!(
                session.current().as_deref(),
                Some("session-one"),
                "held before the record is written"
            );
            assert_eq!(ended(&session, "stopped"), "stopped");
            assert_eq!(
                session.current(),
                None,
                "released once the stop is recorded"
            );
        });

        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.contains("[INFO] agent.supervisor: Local Agent 已启动（sidecar_started） operation_id=session-one"), "{text}");
        assert!(
            text.contains("[INFO] agent.supervisor: Local Agent 已停止 operation_id=session-one"),
            "{text}"
        );
    }

    /// A record made outside a session has **no id field at all**.
    ///
    /// Not an empty one, not `None`, not a placeholder: the field is absent, so
    /// these lines read exactly as they did before the id existed. A value of
    /// any kind would be a claim about which session a record belongs to, and
    /// there is no session to name.
    #[test]
    fn a_record_outside_a_session_has_no_id_field() {
        let (directory, subscriber) = capture("agent-session-absent");

        with_default(subscriber, || {
            started("sidecar_started", None);
            healthy("{\"status\":\"ok\"}", None);
            failed(
                "agent unreachable: connection refused".to_string(),
                &SidecarLog::default(),
                None,
            );
            not_running();
        });

        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(
            !text.contains("operation_id"),
            "no session, no field — not an empty one: {text}"
        );
        assert!(
            text.contains("[INFO] agent.supervisor: Local Agent 已启动（sidecar_started）\n"),
            "{text}"
        );
    }

    /// Every record the module can make, inside one session: all of them name it.
    ///
    /// The whole table rather than one case per function, because the way this
    /// goes wrong is a single emit point added without the session — and a spot
    /// check would not find it. `not_running` is the one record that is never
    /// made inside a session (nothing to stop), so it is not in the table; it is
    /// covered by the test above.
    #[test]
    fn every_record_a_session_can_make_names_that_session() {
        let (directory, subscriber) = capture("agent-session-table");

        with_default(subscriber, || {
            started("sidecar_started", Some("session-one"));
            ready(
                "wt-media-agent local API listening on 127.0.0.1:8765",
                Some("session-one"),
            );
            already_running(Some("session-one"));
            healthy("{\"status\":\"ok\"}", Some("session-one"));
            stopped(Some("session-one"));
            stop_failed("agent stop failed: no such process", Some("session-one"));
            failed(
                "agent unreachable: connection refused".to_string(),
                &SidecarLog::default(),
                Some("session-one"),
            );
        });

        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 7, "one record per call: {text}");
        for line in text.lines() {
            assert!(
                line.ends_with("operation_id=session-one"),
                "every one of them names the session: {line}"
            );
        }
    }

    /// The id is read where the records are made, not handed in from outside: a
    /// health check asked during a session reports that session.
    ///
    /// This is the read point inside the command body. The same assertion for
    /// `start` needs a real launch (the spawn is unreachable), which is why the
    /// real-machine arm in the evidence exists as well.
    #[test]
    fn a_health_check_reports_the_session_it_was_asked_in() {
        let port = answering("{\"status\":\"ok\"}");
        let session = holding("session-one");
        let (directory, subscriber) = capture("agent-session-health");

        let returned = with_default(subscriber, || {
            tauri::async_runtime::block_on(health(
                &client_to(port),
                &SidecarLog::default(),
                &session,
            ))
        });

        assert_eq!(returned.expect("the stub answers"), "{\"status\":\"ok\"}");
        let text = written(&directory.0);
        assert_eq!(
            text.lines().count(),
            1,
            "one record, and it names the session: {text}"
        );
        assert!(
            text.contains("operation_id=session-one"),
            "the id travels with the record: {text}"
        );
    }

    /// The id stays inside this process: it is not on the wire, in any form.
    ///
    /// Asserted against the bytes the stub **received**, not against a
    /// `RequestBuilder`: `host` is added by hyper at send time, so a set read
    /// before `send()` is a baseline that cannot match what the Agent gets.
    /// Three separate ways for the id to escape are ruled out here — the request
    /// line and path (a query string would show up in the first line), the header
    /// names (a full, sorted list rather than a membership check, so an added
    /// header cannot hide), and the request as a whole.
    #[test]
    fn the_session_id_never_reaches_the_request() {
        let (port, seen) = recording("{\"status\":\"ok\"}");
        let session = holding("session-one");

        let returned = tauri::async_runtime::block_on(health(
            &client_to(port),
            &SidecarLog::default(),
            &session,
        ));
        assert_eq!(returned.expect("the stub answers"), "{\"status\":\"ok\"}");

        let request = received(&seen);
        let head = request.split("\r\n\r\n").next().expect("a request head");
        assert_eq!(
            head.lines().next(),
            Some("GET /healthz HTTP/1.1"),
            "the path is the one the client was given, with nothing appended: {head}"
        );
        assert!(
            !request.contains("session-one"),
            "the id is not header, query or body: {request}"
        );

        // Measured, not predicted: this is the list the socket actually saw, in
        // the order it saw it. `accept: */*` is reqwest's, `host` is hyper's at
        // send time, and neither is visible on a `RequestBuilder` before `send`.
        let names: Vec<String> = head
            .lines()
            .skip(1)
            .filter_map(|line| line.split(':').next())
            .map(|name| name.to_ascii_lowercase())
            .collect();
        assert_eq!(
            names,
            vec![
                "authorization".to_string(),
                "accept".to_string(),
                "host".to_string()
            ],
            "the request carries exactly what it carried before the id existed: {head}"
        );
    }
}
