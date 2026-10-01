//! The bind flow: a one-use Cloud ticket becomes a Cloud node registration,
//! then a Local Agent node write, then a runtime-fact report.

use crate::dto::{
    BindResponse, BindSessionArgs, CloudEnvelope, LocalAgentStatus, LocalErrorBody,
    RefreshRuntimeArgs, RegisterLocalNodeRequest, RegisterLocalNodeResponse,
};
use crate::device_identity::DeviceIdentity;
use crate::http::{CloudClient, LocalAgentClient};
use crate::local_agent::BoundNodeFacts;
use crate::preflight::{self, NO_BINDING_REFRESH, NO_CLOUD_ADDRESS_BIND, NO_CLOUD_ADDRESS_REFRESH};
use crate::state::{RuntimeBinding, RuntimeBindingState};
use tauri::State;

use super::agent::local_agent_status;

/// Bind the Desktop to the Local Agent, receiving a session token.
#[tauri::command]
pub async fn local_agent_bind(client: State<'_, LocalAgentClient>) -> Result<BindResponse, String> {
    let resp = client
        .post("/api/v1/bind")
        .json(&serde_json::json!({"node_id": "wt-media-desktop", "binding_token": "desktop-init"}))
        .send()
        .await
        .map_err(|e| format!("bind failed: {}", e))?;
    let body: BindResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid bind response: {}", e))?;
    Ok(body)
}
/// What Desktop posts to the Agent's bind route.
///
/// Split out of both call sites for the reason the record functions in
/// `commands::agent` are: a command takes `State`, which no test can build, and
/// this body is the whole of CHG-061's node-credential channel — the credential
/// Cloud issues for this node has to arrive at the Agent that will spend it, and
/// this object is the only place it does.
///
/// The three fields are not three claims of the same weight. The Agent reads
/// `node_id` and `node_credential`; `binding_token` is **not read by it at all**
/// (`local_api/server.py::record_binding` reads no such key), and the value below
/// is a constant that was never a ticket. It is left where it is rather than
/// removed: this body's shape is asserted by nothing on either side, and
/// deleting a field to tidy a line nobody reads is a change with a wire
/// consequence and no reader. Named here so the constant is not mistaken for the
/// credential travelling beside it.
pub(crate) fn bind_body(node_id: &str, node_credential: &str) -> serde_json::Value {
    serde_json::json!({
        "node_id": node_id,
        "binding_token": "cloud-runtime-bound",
        "node_credential": node_credential,
    })
}

/// Hand the Agent the Cloud node id and the credential for it.
///
/// One implementation with two callers: the bind flow below, which has just
/// received both from Cloud, and the start-time re-push
/// (`commands::downloads::offer_stored_choices`), which has to say them again
/// because the Agent keeps the credential **in memory only** (CHG-061 T-04's
/// ruling) and loses it on every restart. Two bodies spelling the same three
/// keys would be two chances for the re-push to be the one that drifted.
///
/// The status check is not enough on its own, and the field it also reads is why
/// the Agent answers `has_node_credential` at all: a 200 whose body says `false`
/// is this write having been **dropped** — the route did not fail, so no status
/// code can report it, and from the outside an Agent holding nothing looks
/// exactly like one holding a credential until the first download refuses.
pub(crate) async fn bind_node(
    client: &LocalAgentClient,
    node_id: &str,
    node_credential: &str,
) -> Result<(), String> {
    let response = client
        .post("/api/v1/bind")
        .json(&bind_body(node_id, node_credential))
        .send()
        .await
        .map_err(|e| format!("写入Local Agent节点失败: {}", e))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        // The status and the Agent's own **code**, never the body it came in:
        // this request carried the credential, and a peer that echoed its input
        // would put it in a message that `commands::agent::failed` records.
        return Err(match LocalErrorBody::code_of(&text) {
            Some(code) => format!("写入Local Agent节点失败: {}（{}）", status, code),
            None => format!("写入Local Agent节点失败: {}", status),
        });
    }
    let answer: BindResponse =
        serde_json::from_str(&text).map_err(|e| format!("解析Local Agent绑定响应失败: {}", e))?;
    if !answer.has_node_credential {
        return Err("Local Agent 回答里没有登记节点凭据，下载任务将无法上报".into());
    }
    Ok(())
}

/// Bind this Desktop/Local Agent session to Cloud using a one-use Cloud ticket.
///
/// The Vue layer receives the one-use ticket from Cloud and passes it into this
/// native command. Rust consumes the ticket, stores the node credential in native
/// memory, updates Local Agent with the Cloud node id, and reports runtime facts
/// back to Cloud. The credential is never returned to Vue.
#[tauri::command]
pub async fn local_agent_bind_session(
    app: tauri::AppHandle,
    client: State<'_, LocalAgentClient>,
    cloud: State<'_, CloudClient>,
    binding_state: State<'_, RuntimeBindingState>,
    args: BindSessionArgs,
) -> Result<BoundNodeFacts, String> {
    let binding_ticket = args.binding_ticket.trim().to_string();
    if binding_ticket.is_empty() {
        return Err("Cloud绑定票据为空，请重新登录后再检测".into());
    }
    let cloud_base_url =
        preflight::require_cloud_base_url(&args.cloud_base_url, NO_CLOUD_ADDRESS_BIND)?;

    let status = local_agent_status(client.clone()).await?;
    preflight::require_verified_bitbrowser(&status)?;

    let device = DeviceIdentity::for_app(&app)?;

    let register_url = format!("{}/api/v1/local-agent/nodes/register", cloud_base_url);
    let register_payload = RegisterLocalNodeRequest {
        binding_token: binding_ticket,
        agent_id: status.agent_id.clone(),
        device_id: device.device_id.clone(),
        device_public_key: device.public_key(),
        device_signature: device.sign_ticket(&binding_ticket),
        device_name: device.device_name.clone(),
        bind_device: args.bind_device,
        agent_version: status
            .agent_version
            .clone()
            .unwrap_or_else(|| "0.2.2".into()),
        contract_major_version: "v1".into(),
        contract_revision: "2026.10.01.1".into(),
    };
    let register_resp = cloud
        .post(&register_url)
        .json(&register_payload)
        .send()
        .await
        .map_err(|e| format!("Cloud节点注册失败: {}", e))?;
    let register_status = register_resp.status();
    let register_text = register_resp
        .text()
        .await
        .map_err(|e| format!("读取Cloud节点注册响应失败: {}", e))?;
    if !register_status.is_success() {
        return Err(format!(
            "Cloud节点注册失败: {} {}",
            register_status, register_text
        ));
    }
    let register_body: CloudEnvelope<RegisterLocalNodeResponse> =
        serde_json::from_str(&register_text)
            .map_err(|e| format!("Cloud节点注册响应格式错误: {}", e))?;
    if register_body.errcode != 0 {
        return Err(if register_body.message.is_empty() {
            "Cloud节点注册失败".into()
        } else {
            register_body.message
        });
    }
    let registration = register_body
        .data
        .ok_or_else(|| "Cloud节点注册响应缺少数据".to_string())?;

    bind_node(
        &client,
        &registration.node.id,
        &registration.node_credential,
    )
    .await?;

    preflight::sync_runtime_facts(
        &cloud,
        &cloud_base_url,
        &registration.node.id,
        &registration.node_credential,
        &status,
    )
    .await?;

    binding_state
        .0
        .lock()
        .map_err(|_| "runtime binding lock poisoned")?
        .replace(RuntimeBinding {
            node_id: registration.node.id.clone(),
            node_credential: registration.node_credential,
        });

    Ok(BoundNodeFacts {
        id: registration.node.id,
        agent_id: registration.node.agent_id,
        user_id: registration.node.user_id.to_string(),
        status: registration.node.status,
    })
}
#[tauri::command]
pub async fn local_agent_refresh_runtime(
    client: State<'_, LocalAgentClient>,
    cloud: State<'_, CloudClient>,
    binding_state: State<'_, RuntimeBindingState>,
    args: RefreshRuntimeArgs,
) -> Result<LocalAgentStatus, String> {
    let cloud_base_url =
        preflight::require_cloud_base_url(&args.cloud_base_url, NO_CLOUD_ADDRESS_REFRESH)?;
    let binding = preflight::require_binding(&binding_state, NO_BINDING_REFRESH)?;
    let status = local_agent_status(client.clone()).await?;
    preflight::sync_runtime_facts(
        &cloud,
        &cloud_base_url,
        &binding.node_id,
        &binding.node_credential,
        &status,
    )
    .await?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// The credential the tests hand over, spelled so that finding it anywhere
    /// it should not be is unmistakable.
    const CREDENTIAL: &str = "node-credential-9f3a-secret";

    /// A client pointed at `port`, built through the shipped configuration.
    ///
    /// Through `PRODUCTION_TOML` rather than a hand-built struct for the reason
    /// `agent.rs`'s `config_to` gives: the client under test is the one `main.rs`
    /// builds, and a constructor the tests can call is not.
    fn client_to(port: u16) -> LocalAgentClient {
        let text = PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}"));
        let config =
            load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config");
        LocalAgentClient::new(&config, RuntimeToken::generate())
    }

    /// A listener answering `status`/`body` to one request, **and the bytes it
    /// received**.
    ///
    /// Port 0, never a fixed one: the socket is the test's own, and a test that
    /// picked 8765 would fight the developer's running Agent for it.
    ///
    /// The request is kept because 「凭据确实到了 Agent」is a claim about what
    /// left the process, and no assertion on a `RequestBuilder` can make it: the
    /// body is serialised by serde at send time, so a value read before `send()`
    /// is a baseline that cannot match the wire. The body is read to its
    /// `content-length` rather than in one `read`, because a credential-bearing
    /// JSON body is not guaranteed to arrive in the same packet as the headers.
    fn recording(status: &str, body: &str) -> (u16, Arc<Mutex<Vec<u8>>>) {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = server.local_addr().expect("the bound address").port();
        let status = status.to_string();
        let body = body.to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        std::thread::spawn(move || {
            if let Ok((mut socket, _)) = server.accept() {
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while let Ok(read) = socket.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    match request.windows(4).position(|w| w == b"\r\n\r\n") {
                        // Headers complete: keep going until the body is whole.
                        Some(end) => {
                            let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                            let length = headers
                                .lines()
                                .find_map(|line| line.strip_prefix("content-length:"))
                                .and_then(|value| value.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if request.len() >= end + 4 + length {
                                break;
                            }
                        }
                        None => {}
                    }
                }
                sink.lock()
                    .expect("the recorder")
                    .extend_from_slice(&request);
                let response = format!(
                    "HTTP/1.1 {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        (port, seen)
    }

    /// What the stub received, as text.
    fn received(seen: &Arc<Mutex<Vec<u8>>>) -> String {
        let bytes = seen.lock().expect("the recorder").clone();
        String::from_utf8(bytes).expect("an HTTP request is ASCII")
    }

    /// The Agent's answer to a bind that stored the credential.
    fn holding_credential() -> String {
        r#"{"node_id":"node-a","session_token":"session-a","status":"active","has_node_credential":true}"#
            .to_string()
    }

    /// The node id and the credential are both on the wire, in the body.
    ///
    /// This is the whole of CHG-061's node-credential channel: `bind_body` is the
    /// only place the value Cloud issued for this node is written, and the
    /// assertion is against the bytes the Agent received rather than against the
    /// `json!` that produced them.
    #[test]
    fn the_node_id_and_the_credential_reach_the_bind_route() {
        let (port, seen) = recording("200 OK", &holding_credential());

        let returned =
            tauri::async_runtime::block_on(bind_node(&client_to(port), "node-a", CREDENTIAL));

        assert!(returned.is_ok(), "the Agent held it: {returned:?}");
        let request = received(&seen);
        assert!(
            request.starts_with("POST /api/v1/bind "),
            "the bind route: {request}"
        );
        assert!(
            request.contains(CREDENTIAL),
            "the credential has to travel: {request}"
        );
        assert!(
            request.contains("\"node_id\":\"node-a\""),
            "and the node it belongs to: {request}"
        );
    }

    /// A 200 whose body says the credential was **not** stored is a failure.
    ///
    /// The route succeeded, so no status code can report this: the Agent answers
    /// `has_node_credential` from what it holds precisely so that a write which
    /// was dropped is distinguishable from one which landed. Without this arm the
    /// caller would return `Ok` and the first download would be the thing that
    /// discovered it.
    #[test]
    fn an_agent_that_did_not_store_the_credential_is_a_failure() {
        let (port, _seen) = recording(
            "200 OK",
            r#"{"node_id":"node-a","session_token":"session-a","status":"active","has_node_credential":false}"#,
        );

        let returned =
            tauri::async_runtime::block_on(bind_node(&client_to(port), "node-a", CREDENTIAL));

        assert!(returned.is_err(), "a dropped write is not a success");
    }

    /// A refusal is reported by its **code**, not by the body it arrived in.
    #[test]
    fn a_refusal_is_named_by_the_agents_own_code() {
        let (port, _seen) = recording(
            "503 Service Unavailable",
            r#"{"error":{"code":"node_credential_unavailable","message":"no credential held"}}"#,
        );

        let returned =
            tauri::async_runtime::block_on(bind_node(&client_to(port), "node-a", CREDENTIAL));

        let message = returned.expect_err("a 503 is a failure");
        assert!(
            message.contains("node_credential_unavailable"),
            "the code is what a caller can act on: {message}"
        );
    }

    /// A peer that echoes its input cannot put the credential in a message.
    ///
    /// The arm `LocalErrorBody` exists for. This request carried the credential,
    /// and the message `bind_node` returns is recorded by the caller
    /// (`commands::agent::failed` puts it in the lifecycle record), so a body
    /// passed through verbatim would be a credential in a log. The stub below is
    /// the worst case: an Agent that answers a refusal **containing its own
    /// request**. `code_of` reads the code and drops the rest, and the assertion
    /// is on the whole message so no future edit can reintroduce the body.
    #[test]
    fn a_refusal_that_echoes_the_request_never_puts_the_credential_in_the_message() {
        // Built from `CREDENTIAL` rather than restated: a stub that hard-codes
        // the value would stop being the worst case the moment the constant
        // changes, and the test would pass while asserting nothing.
        let echoed = format!(
            r#"{{"error":{{"code":"node_credential_unavailable","message":"rejected {{\"node_credential\":\"{CREDENTIAL}\"}}"}}}}"#
        );
        let (port, _seen) = recording("503 Service Unavailable", &echoed);

        let returned =
            tauri::async_runtime::block_on(bind_node(&client_to(port), "node-a", CREDENTIAL));

        let message = returned.expect_err("a 503 is a failure");
        assert!(
            !message.contains(CREDENTIAL),
            "the credential came back and was passed on: {message}"
        );
        assert!(
            !message.contains('{'),
            "no fragment of the body it arrived in: {message}"
        );
        assert!(
            message.contains("node_credential_unavailable"),
            "the code is still named: {message}"
        );
    }

    /// A body that is not an Agent refusal is reported as the status alone.
    ///
    /// An intermediary (a proxy, a half-written response, an unreachable server
    /// that still answers) can produce a non-JSON body, and 「拒绝，但没说为什么」
    /// is a different sentence from a code the caller can name. Asserted on the
    /// status being present and no code being invented.
    #[test]
    fn a_body_that_is_not_an_agents_refusal_reports_the_status_alone() {
        let (port, _seen) = recording("500 Internal Server Error", "oops");

        let returned =
            tauri::async_runtime::block_on(bind_node(&client_to(port), "node-a", CREDENTIAL));

        let message = returned.expect_err("a 500 is a failure");
        assert!(
            message.contains("500"),
            "the status still says what happened: {message}"
        );
        assert!(
            !message.contains('（'),
            "nothing is invented as a code: {message}"
        );
    }

    /// The key set is pinned, and it is three keys.
    ///
    /// The body's shape is asserted by nothing on either side of the wire (the
    /// Agent's `record_binding` reads keys it is given and ignores the rest), so
    /// this test is the only place a rename would be caught. `binding_token` is
    /// named here rather than noted as dead: a later hand that removes it is
    /// changing a wire shape, and this is the assertion that says so out loud.
    #[test]
    fn the_bind_body_carries_the_node_the_constant_and_the_credential() {
        let body = bind_body("node-a", CREDENTIAL);

        let keys: Vec<&str> = body
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["binding_token", "node_credential", "node_id"]);
        assert_eq!(body["node_id"], "node-a");
        assert_eq!(body["node_credential"], CREDENTIAL);
    }
}
