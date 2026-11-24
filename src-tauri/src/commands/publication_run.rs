//! Controlled actions against one still-live original publication scene.
use crate::dto::GuardRefusal;
use crate::http::LocalAgentClient;
use crate::state::AgentTokenState;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::State;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationRunArgs {
    pub user_id: String,
    pub publication_id: String,
    pub attempt_id: String,
    pub expected_revision: String,
}
fn refused(code: &str, message: impl Into<String>) -> GuardRefusal {
    GuardRefusal {
        code: code.into(),
        message: message.into(),
        ..Default::default()
    }
}
fn canonical(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|id| id > 0 && id.to_string() == value)
}
async fn request(
    client: &LocalAgentClient,
    tokens: &AgentTokenState,
    args: PublicationRunArgs,
    resume: bool,
) -> Result<Value, GuardRefusal> {
    if !canonical(&args.publication_id)
        || !canonical(&args.attempt_id)
        || !canonical(&args.expected_revision)
    {
        return Err(refused(
            "publication_input_invalid",
            "任务、尝试或修订无效，请刷新后重试",
        ));
    }
    let user = tokens
        .publication_user(&args.user_id)
        .map_err(|message| refused("publication_identity_required", message))?;
    let operation = if resume { "resume" } else { "scene" };
    let path = format!("/api/v1/publication-run/{}/{operation}", args.attempt_id);
    let response=client.post(&path).header("X-Publication-User-ID",user).json(&json!({"publication_id":args.publication_id,"expected_revision":args.expected_revision})).send().await.map_err(GuardRefusal::unreachable)?;
    let status = response.status();
    let text = response.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    tokens
        .publication_user(&args.user_id)
        .map_err(|message| refused("publication_identity_required", message))?;
    let value: Value = serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))?;
    value
        .get("data")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| GuardRefusal::unreadable(status))
}
#[tauri::command]
pub async fn local_publication_scene(
    client: State<'_, LocalAgentClient>,
    tokens: State<'_, AgentTokenState>,
    args: PublicationRunArgs,
) -> Result<Value, GuardRefusal> {
    request(&client, &tokens, args, false).await
}
#[tauri::command]
pub async fn local_publication_resume(
    client: State<'_, LocalAgentClient>,
    tokens: State<'_, AgentTokenState>,
    args: PublicationRunArgs,
) -> Result<Value, GuardRefusal> {
    request(&client, &tokens, args, true).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{requests, serving};
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::state::AgentToken;
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;
    fn args(user: &str) -> PublicationRunArgs {
        PublicationRunArgs {
            user_id: user.into(),
            publication_id: "9007199254740993".into(),
            attempt_id: "9007199254740995".into(),
            expected_revision: "6".into(),
        }
    }
    #[test]
    fn scene_bridge_binds_native_owner_and_exact_attempt() {
        let (port, seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"data":{"attempt_id":"9007199254740995","status":"awaiting_human"}}"#.into(),
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
            tauri::async_runtime::block_on(request(&client, &tokens, args("10"), false)).is_err()
        );
        let mut invalid = args("9");
        invalid.attempt_id = "../2".into();
        assert!(tauri::async_runtime::block_on(request(&client, &tokens, invalid, false)).is_err());
        assert!(requests(&seen).is_empty());
        tauri::async_runtime::block_on(request(&client, &tokens, args("9"), false)).unwrap();
        let wire = requests(&seen).remove(0);
        assert!(wire.starts_with("POST /api/v1/publication-run/9007199254740995/scene "));
        assert!(wire.to_lowercase().contains("x-publication-user-id: 9"));
        assert!(wire.contains("9007199254740993"));
        assert!(!wire.contains("cloud-secret"));
    }
}
