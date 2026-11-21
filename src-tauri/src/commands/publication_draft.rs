//! Narrow new-task input bridge. Page identity is only an expected-user check.
use crate::dto::GuardRefusal;
use crate::http::LocalAgentClient;
use crate::state::AgentTokenState;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::State;

const TASK_DRAFT_PATH: &str = "/api/v1/publication-draft/task";
const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
pub struct DraftReadArgs {
    pub user_id: String,
}
#[derive(Deserialize)]
pub struct DraftSaveArgs {
    pub user_id: String,
    pub expected_revision: String,
    pub payload: Value,
}
#[derive(Deserialize)]
pub struct DraftClearArgs {
    pub user_id: String,
    pub expected_revision: String,
}

fn refused(code: &str, message: impl Into<String>) -> GuardRefusal {
    GuardRefusal {
        code: code.into(),
        message: message.into(),
        ..Default::default()
    }
}

async fn request(
    client: &LocalAgentClient,
    tokens: &AgentTokenState,
    expected_user: &str,
    method: &str,
    body: Option<Value>,
) -> Result<Value, GuardRefusal> {
    let user_id = tokens
        .publication_user(expected_user)
        .map_err(|message| refused("draft_identity_required", message))?;
    let request = match method {
        "PUT" => client.put(TASK_DRAFT_PATH),
        "DELETE" => client.delete(TASK_DRAFT_PATH),
        _ => client.get(TASK_DRAFT_PATH),
    }
    .header("X-Publication-User-ID", user_id);
    let request = if let Some(body) = body {
        request.json(&body)
    } else {
        request
    };
    let response = request.send().await.map_err(GuardRefusal::unreachable)?;
    let status = response.status();
    let text = response.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))?;
    // A previous user's reply is never returned to the current page.
    tokens
        .publication_user(expected_user)
        .map_err(|message| refused("draft_identity_required", message))?;
    value
        .get("data")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| GuardRefusal::unreadable(status))
}

#[tauri::command]
pub async fn local_publication_draft_get(
    client: State<'_, LocalAgentClient>,
    tokens: State<'_, AgentTokenState>,
    args: DraftReadArgs,
) -> Result<Value, GuardRefusal> {
    request(&client, &tokens, &args.user_id, "GET", None).await
}

#[tauri::command]
pub async fn local_publication_draft_save(
    client: State<'_, LocalAgentClient>,
    tokens: State<'_, AgentTokenState>,
    args: DraftSaveArgs,
) -> Result<Value, GuardRefusal> {
    if !args.payload.is_object()
        || serde_json::to_vec(&args.payload).map_or(true, |bytes| bytes.len() > MAX_INPUT_BYTES)
    {
        return Err(refused("draft_input_invalid", "续编输入无效或超过大小限制"));
    }
    request(
        &client,
        &tokens,
        &args.user_id,
        "PUT",
        Some(json!({"payload": args.payload, "expected_revision": args.expected_revision})),
    )
    .await
}

#[tauri::command]
pub async fn local_publication_draft_clear(
    client: State<'_, LocalAgentClient>,
    tokens: State<'_, AgentTokenState>,
    args: DraftClearArgs,
) -> Result<Value, GuardRefusal> {
    request(
        &client,
        &tokens,
        &args.user_id,
        "DELETE",
        Some(json!({"expected_revision": args.expected_revision})),
    )
    .await
}

#[tauri::command]
pub fn local_publication_draft_deactivate(tokens: State<'_, AgentTokenState>) {
    tokens.clear_publication_user();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{requests, serving};
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::state::AgentToken;
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;

    #[test]
    fn draft_bridge_uses_fixed_path_and_authenticated_owner() {
        let (port, seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"data":{"payload":null,"revision":"0","saved_at":null,"schema_version":1}}"#
                    .into(),
            )
        });
        let config = load_with(
            &BTreeMap::new(),
            &PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}")),
            Environment::Production,
        )
        .unwrap();
        let client = LocalAgentClient::new(&config, RuntimeToken::generate());
        let tokens = AgentTokenState::default();
        let generation = tokens.begin_configuration().unwrap();
        tokens.set_for_user(
            AgentToken {
                token: "cloud-secret".into(),
                expires_at: Some(4_000_000_000),
            },
            "9".into(),
            generation,
        );
        assert!(
            tauri::async_runtime::block_on(request(&client, &tokens, "10", "GET", None)).is_err()
        );
        assert!(requests(&seen).is_empty());
        let value =
            tauri::async_runtime::block_on(request(&client, &tokens, "9", "GET", None)).unwrap();
        assert_eq!(value["revision"], "0");
        let request = requests(&seen).remove(0);
        assert!(request.starts_with("GET /api/v1/publication-draft/task "));
        assert!(request.to_lowercase().contains("x-publication-user-id: 9"));
        assert!(request.to_lowercase().contains("authorization: bearer "));
        assert!(!request.contains("cloud-secret"));
    }
}
