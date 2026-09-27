//! Where a download lands: the operator's choice, handed to the Local Agent.
//!
//! One fact, two owners, and this module is the seam. The operator chooses the
//! folder on the 本机设置 page and Desktop stores it in its own settings file
//! (`commands::settings`); the **Agent** is what writes the bytes, so it has to
//! be told, and its own copy is what the download executor reads. Neither side
//! can derive the other's value, so the choice travels over the Agent's
//! save-directory route — the shape of the channel CHG-061 T-04 settled on
//! (「新增 Local Agent 本机 API 端点」), not a second command path: the Agent's
//! server checks the runtime token on **every** route, so a push over the same
//! loopback HTTP the bind flow uses is authenticated by construction
//! (`local_api/server.py::_check_auth`, no per-route exception).
//!
//! ## Why the push is its own function and not part of a settings write
//!
//! A settings write is local and a push is a network call, and entangling them
//! would make 「保存失败」mean either 「这台机器的设置文件写不进去」or 「Agent 没在
//! 运行」 — two sentences the operator has to act on differently. So
//! `local_settings_set` stores and nothing else; the push happens here, and a
//! push that fails leaves the choice stored, which is the state the operator can
//! retry from.
//!
//! ## Why a start re-pushes what it already pushed
//!
//! The Agent is a child process of this one and keeps no memory across a restart:
//! the node credential is held **in memory only** (T-04's ruling) and the save
//! directory is in the Agent's own SQLite, which can be older than the choice on
//! disk. A `local_agent_start` that succeeded is therefore the one moment where
//! saying both again is cheap and correct, and the two `offer_*` functions below
//! are that moment's whole content. They are best-effort **and separate**: the
//! Agent is up and healthy by the time they run, so a push that fails must not
//! turn a start that plainly worked into an error, and the credential failing
//! must not stop the directory from being offered.
//!
//! Both are split out of the start for the reason the record functions in
//! `commands::agent` are: a command takes `State`, which no test can build, and
//! a function nobody can call is a function nobody can check.

use crate::dto::{LocalErrorBody, SaveDirectoryFacts, SaveDirectoryResponse};
use crate::state::RuntimeBinding;

use super::bind;

/// What Desktop posts to the Agent's save-directory route.
///
/// One key, and it is the Agent's spelling: `local_api/server.py::
/// set_save_directory_response` reads `body.get("save_dir")` and answers a
/// `save_directory_invalid` to anything that is not a string — so a rename here
/// is not a cosmetic change, it is a 400 that reads like a bad path.
pub(crate) fn save_directory_body(save_dir: &str) -> serde_json::Value {
    serde_json::json!({ "save_dir": save_dir })
}

/// Hand the Agent the folder to write downloads into, and read back its answer.
///
/// The answer is the Agent's **stored** facts rather than an echo of the request
/// (`set_save_directory_response` reads back), and it is returned rather than
/// discarded because the two facts beside the directory — `writable`,
/// `free_bytes` — are measurements the page would otherwise have to guess at.
///
/// The path is deliberately **not** in any error message. It is this machine's
/// filesystem layout, CHG-061 §4 keeps local paths out of records, and the
/// caller here records what it returns (`commands::agent::offer_failed`) — so a
/// message built with `format!("…{save_dir}…")` would be the one line that puts
/// the operator's home directory in the log file. The Agent's own refusals
/// already carry no path for the same reason; this keeps Desktop's side of the
/// same rule.
pub(crate) async fn push(
    client: &crate::http::LocalAgentClient,
    save_dir: &str,
) -> Result<SaveDirectoryFacts, String> {
    let response = client
        .post("/api/v1/save-directory")
        .json(&save_directory_body(save_dir))
        .send()
        .await
        .map_err(|e| format!("写入Local Agent保存目录失败: {}", e))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(match LocalErrorBody::code_of(&text) {
            Some(code) => format!("写入Local Agent保存目录失败: {}（{}）", status, code),
            None => format!("写入Local Agent保存目录失败: {}", status),
        });
    }
    let answer: SaveDirectoryResponse = serde_json::from_str(&text)
        .map_err(|e| format!("解析Local Agent保存目录响应失败: {}", e))?;
    Ok(answer.data)
}

/// Tell a freshly started Agent the Cloud identity it has to report under.
///
/// `None` when there is nothing to say or it was said: no binding means this
/// Desktop has not been bound in this launch, and the credential that would be
/// pushed is one Cloud issues at bind time — Desktop holds nothing to re-state.
/// `Some(reason)` only when an attempt was made and failed, which is the case
/// worth a record: the Agent is running with no way to report a download.
///
/// The push itself is `bind::bind_node`, the **same** function the bind flow
/// calls. A second implementation would be the one that drifted, and the drift
/// would be silent — the Agent would answer 200 to both and only the download
/// would notice.
pub(crate) async fn offer_credential(
    client: &crate::http::LocalAgentClient,
    binding: Option<&RuntimeBinding>,
) -> Option<String> {
    let held = binding?;
    bind::bind_node(client, &held.node_id, &held.node_credential)
        .await
        .err()
}

/// Tell a freshly started Agent which folder this machine's downloads go in.
///
/// `None` when the choice was pushed, or when there is no choice to push.
///
/// **「没有选择」is not pushed as a clearing**, because the route cannot express
/// one: its body is a `save_dir` string, and the Agent answers a
/// `save_directory_invalid` to a missing or `null` value rather than treating it
/// as 「忘了那个目录」. So an operator who clears the choice leaves the Agent on
/// the folder it was last given, and downloads keep landing there until a new
/// one is chosen. Registered in CHG-061's checkpoint rather than worked around
/// here: teaching the route to clear is a change to the frozen Local Agent
/// contract, which is a decision and not a debugging step.
pub(crate) async fn offer_directory(
    client: &crate::http::LocalAgentClient,
    chosen: Option<&str>,
) -> Option<String> {
    push(client, chosen?).await.err()
}

/// The whole of what a start says to the Agent that just came up, and what it
/// could not say.
///
/// The composition, split out of `commands::agent::start` for the reason every
/// other body there is: `start` takes an `AppHandle` and reaches its success arm
/// only against a real Agent, so a sequence written inside it would be reachable
/// by no test at all — and the property this function exists for is exactly a
/// property of the sequence. **A failure is collected, never propagated**: an
/// Agent that is up is what the caller asked for, and one push failing must not
/// stop the other from being attempted.
///
/// `chosen` is a `Result` rather than an `Option` because reading the operator's
/// settings file can fail, and that failure is one of the things a start has to
/// report: it is a different sentence from 「没有选择」 and from 「Agent 拒绝了」,
/// and all three end with a download that will not run. Read by the caller
/// (`settings::chosen_save_dir`) rather than here, so this function is a function
/// of its arguments — no file, no layout, and a test that can produce all four
/// cases.
pub(crate) async fn offer_stored_choices(
    client: &crate::http::LocalAgentClient,
    binding: Option<&RuntimeBinding>,
    chosen: Result<Option<String>, String>,
) -> Vec<String> {
    let mut failures = Vec::new();
    if let Some(reason) = offer_credential(client, binding).await {
        failures.push(reason);
    }
    match chosen {
        Ok(chosen) => {
            if let Some(reason) = offer_directory(client, chosen.as_deref()).await {
                failures.push(reason);
            }
        }
        Err(reason) => failures.push(reason),
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::http::LocalAgentClient;
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// The folder the tests hand over, and the one that must never come back in
    /// a message. Absolute, because the Agent refuses anything else, and under
    /// `/Volumes` so it reads as a path on this machine rather than as a fixture.
    const FOLDER: &str = "/Volumes/Movies/WTMedia";
    const CREDENTIAL: &str = "node-credential-77c1-secret";

    fn client_to(port: u16) -> LocalAgentClient {
        let text = PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}"));
        let config =
            load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config");
        LocalAgentClient::new(&config, RuntimeToken::generate())
    }

    /// A listener answering **every** request on it, recording all of them.
    ///
    /// Accepts until the client stops coming, rather than once, because a start
    /// offers two things and an arm that only ever answered one request could not
    /// tell 「the second was not sent」from 「the second was sent and nobody was
    /// listening」— which is the difference between the two pushes being
    /// independent and one of them being skipped.
    ///
    /// `answer` maps a status line to the body for it, so a test can refuse one
    /// route and accept the other.
    fn serving(
        answer: impl Fn(&str) -> (String, String) + Send + 'static,
    ) -> (u16, Arc<Mutex<Vec<String>>>) {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = server.local_addr().expect("the bound address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        std::thread::spawn(move || {
            for incoming in server.incoming() {
                let Ok(mut socket) = incoming else { break };
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                let mut complete = false;
                while let Ok(read) = socket.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            complete = true;
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&request).to_string();
                let path = text
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                sink.lock().expect("the recorder").push(text);
                if !complete {
                    break;
                }
                let (status, body) = answer(&path);
                let response = format!(
                    "HTTP/1.1 {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                if socket.write_all(response.as_bytes()).is_err() {
                    break;
                }
            }
        });
        (port, seen)
    }

    /// The Agent's answer to a stored directory.
    fn stored(dir: &str) -> String {
        format!(r#"{{"data":{{"save_dir":"{dir}","writable":true,"free_bytes":4096}}}}"#)
    }

    /// What the stub received, joined.
    fn received(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock().expect("the recorder").clone()
    }

    /// How many requests arrived for `route`.
    fn arrivals(seen: &Arc<Mutex<Vec<String>>>, route: &str) -> usize {
        received(seen)
            .iter()
            .filter(|request| {
                request
                    .split_whitespace()
                    .nth(1)
                    .is_some_and(|path| path == route)
            })
            .count()
    }

    /// A port nothing is listening on: bound, then released.
    ///
    /// The negative arms use this rather than a stub that records nothing, and
    /// the difference matters: an attempted push against a closed port is an
    /// `Err` from `send()`, so 「什么也没推」 is only proved by a call that would
    /// have failed had it tried.
    fn silent_port() -> u16 {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        server.local_addr().expect("the bound address").port()
    }

    fn binding() -> RuntimeBinding {
        RuntimeBinding {
            node_id: "node-a".into(),
            node_credential: CREDENTIAL.into(),
        }
    }

    /// The body is one key, spelled the Agent's way.
    #[test]
    fn the_save_directory_body_is_the_agents_one_key() {
        let body = save_directory_body(FOLDER);

        let keys: Vec<&str> = body
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["save_dir"]);
        assert_eq!(body["save_dir"], FOLDER);
    }

    /// The folder reaches the route, and the Agent's own facts come back.
    #[test]
    fn a_pushed_directory_answers_with_the_agents_facts() {
        let (port, seen) = serving(|_| ("200 OK".into(), stored(FOLDER)));

        let returned = tauri::async_runtime::block_on(push(&client_to(port), FOLDER));

        let facts = returned.expect("the Agent stored it");
        assert_eq!(
            facts,
            SaveDirectoryFacts {
                save_dir: Some(FOLDER.to_string()),
                writable: true,
                free_bytes: 4096,
            }
        );
        let requests = received(&seen);
        assert_eq!(arrivals(&seen, "/api/v1/save-directory"), 1);
        assert!(
            requests[0].contains(FOLDER),
            "the folder has to travel: {}",
            requests[0]
        );
    }

    /// A refusal is named by the Agent's code, and **without the path**.
    ///
    /// The second half is the one a future edit could undo without noticing: the
    /// message this returns is what `commands::agent::offer_failed` records, and
    /// CHG-061 §4 keeps this machine's absolute paths out of records. Asserting
    /// on the whole message rather than on a fragment, so a message that grows a
    /// `format!("…{save_dir}…")` fails here rather than in a log file.
    #[test]
    fn a_refused_directory_is_named_by_its_code_and_carries_no_path() {
        let (port, _seen) = serving(|_| {
            (
                "400 Bad Request".into(),
                r#"{"error":{"code":"save_directory_invalid"}}"#.into(),
            )
        });

        let returned = tauri::async_runtime::block_on(push(&client_to(port), FOLDER));

        let message = returned.expect_err("a 400 is a failure");
        assert!(
            message.contains("save_directory_invalid"),
            "the code is what a caller can act on: {message}"
        );
        assert!(
            !message.contains(FOLDER) && !message.contains("Movies"),
            "a local path reached a message that is recorded: {message}"
        );
    }

    /// A body that is not a refusal is the status alone, and still no path.
    #[test]
    fn an_unrecognised_refusal_body_reports_the_status_alone() {
        let (port, _seen) = serving(|_| ("503 Service Unavailable".into(), "oops".into()));

        let message = tauri::async_runtime::block_on(push(&client_to(port), FOLDER))
            .expect_err("a 503 is a failure");

        assert!(message.contains("503"), "{message}");
        assert!(!message.contains('（'), "nothing is invented: {message}");
        assert!(!message.contains("Movies"), "no path: {message}");
    }

    /// 「没有选择」puts nothing on the wire.
    ///
    /// Asserted against a **closed** port: had a request been attempted it would
    /// have come back as a connection error and this arm would return
    /// `Some(reason)` instead of `None`. A stub that merely recorded nothing
    /// would pass for the wrong reason here, because `offer_directory` returns
    /// `None` for a successful push too.
    #[test]
    fn an_unchosen_directory_pushes_nothing() {
        let port = silent_port();

        let returned = tauri::async_runtime::block_on(offer_directory(&client_to(port), None));

        assert_eq!(returned, None, "nothing to say, so nothing is said");
    }

    /// An unbound Desktop has no credential to re-state.
    #[test]
    fn an_unbound_desktop_offers_no_credential() {
        let port = silent_port();

        let returned = tauri::async_runtime::block_on(offer_credential(&client_to(port), None));

        assert_eq!(returned, None);
    }

    /// Both things a start says arrive, over their two routes.
    #[test]
    fn a_start_offers_the_credential_and_the_folder_together() {
        let (port, seen) = serving(|path| {
            match path {
            "/api/v1/bind" => (
                "200 OK".into(),
                r#"{"node_id":"node-a","session_token":"s","status":"active","has_node_credential":true}"#
                    .into(),
            ),
            _ => ("200 OK".into(), stored(FOLDER)),
        }
        });

        let failures = tauri::async_runtime::block_on(offer_stored_choices(
            &client_to(port),
            Some(&binding()),
            Ok(Some(FOLDER.to_string())),
        ));

        assert_eq!(failures, Vec::<String>::new(), "nothing to report");
        let requests = received(&seen);
        assert_eq!(arrivals(&seen, "/api/v1/bind"), 1);
        assert_eq!(arrivals(&seen, "/api/v1/save-directory"), 1);
        let bind = requests
            .iter()
            .find(|request| request.contains("/api/v1/bind"))
            .expect("the bind request");
        assert!(
            bind.contains(CREDENTIAL) && bind.contains("node-a"),
            "the credential and the node it belongs to: {bind}"
        );
        let directory = requests
            .iter()
            .find(|request| request.contains("/api/v1/save-directory"))
            .expect("the directory request");
        assert!(directory.contains(FOLDER), "{directory}");
    }

    /// One push failing does not stop the other.
    ///
    /// The property `offer_stored_choices` is a separate function for, and one
    /// `?` would undo: the Agent answers a 503 to the credential route and a 200
    /// to the directory route, and **both** requests arrive. Without this arm a
    /// composition that propagated the first failure would be indistinguishable
    /// from a correct one in every other test here, and the folder would go
    /// unset for a reason that has nothing to do with the folder.
    #[test]
    fn a_refused_credential_does_not_stop_the_folder_from_being_offered() {
        let (port, seen) = serving(|path| {
            if path == "/api/v1/bind" {
                (
                    "503 Service Unavailable".into(),
                    r#"{"error":{"code":"node_credential_unavailable"}}"#.into(),
                )
            } else {
                ("200 OK".into(), stored(FOLDER))
            }
        });

        let failures = tauri::async_runtime::block_on(offer_stored_choices(
            &client_to(port),
            Some(&binding()),
            Ok(Some(FOLDER.to_string())),
        ));

        assert_eq!(
            failures.len(),
            1,
            "one refusal, reported once: {failures:?}"
        );
        assert!(
            failures[0].contains("node_credential_unavailable"),
            "{failures:?}"
        );
        assert_eq!(
            arrivals(&seen, "/api/v1/save-directory"),
            1,
            "the folder was still offered"
        );
    }

    /// A settings file that cannot be read is reported, and stops nothing.
    ///
    /// The `Err` arm of `chosen`: a corrupt or foreign-schema settings file is a
    /// reason the folder will not be set, and it is a different sentence from
    /// 「没有选择」 — so it is recorded rather than read as 「nothing chosen」. The
    /// credential push still happens, because the two have nothing to do with
    /// each other.
    #[test]
    fn a_settings_file_that_cannot_be_read_is_reported_and_offers_nothing() {
        let (port, seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"node_id":"node-a","session_token":"s","status":"active","has_node_credential":true}"#
                    .into(),
            )
        });

        let failures = tauri::async_runtime::block_on(offer_stored_choices(
            &client_to(port),
            Some(&binding()),
            Err("读取设置失败：schema_version 不认识".into()),
        ));

        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("读取设置失败"), "{failures:?}");
        assert_eq!(
            arrivals(&seen, "/api/v1/save-directory"),
            0,
            "a value that could not be read must not be guessed at"
        );
        assert_eq!(
            arrivals(&seen, "/api/v1/bind"),
            1,
            "and the credential still goes"
        );
    }

    /// A start whose Agent cannot be reached reports both, and does not panic.
    ///
    /// The failure mode of a re-push against an Agent that died between the
    /// readiness gate and this call — narrow, and the reason the composition
    /// collects reasons rather than returning an error into the start. Two
    /// failures, because the two attempts are independent, and no path in
    /// either — these strings are what `offer_failed` records.
    #[test]
    fn an_unreachable_agent_is_reported_rather_than_propagated() {
        let port = silent_port();

        let failures = tauri::async_runtime::block_on(offer_stored_choices(
            &client_to(port),
            Some(&binding()),
            Ok(Some(FOLDER.to_string())),
        ));

        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(
            failures[0].contains("写入Local Agent节点失败"),
            "{failures:?}"
        );
        assert!(
            failures[1].contains("写入Local Agent保存目录失败"),
            "{failures:?}"
        );
        for reason in &failures {
            assert!(
                !reason.contains("Movies") && !reason.contains(CREDENTIAL),
                "no path and no secret on the way to a record: {reason}"
            );
        }
    }
}
