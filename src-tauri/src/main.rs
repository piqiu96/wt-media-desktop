mod commands;
mod filesystem;
mod local_agent;
mod secure_store;
mod system;
mod updater;

use local_agent::{BindingTransport, BoundNodeFacts, LocalAgentBridge};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct LocalAgentStatus {
    agent_id: String,
    status: String,
    current_task_id: Option<String>,
    pending_result_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct LocalAgentStatusEvent {
    event: &'static str,
    data: LocalAgentStatus,
}

struct M0BindingTransport;

impl BindingTransport for M0BindingTransport {
    fn consume_binding_ticket(
        &mut self,
        binding_ticket: String,
    ) -> Result<BoundNodeFacts, local_agent::BindSessionError> {
        Ok(BoundNodeFacts {
            id: format!("node-{}", binding_ticket.len()),
            agent_id: "local-agent-dev".into(),
            user_id: "user-dev".into(),
            status: "online".into(),
        })
    }
}

fn stopped_status() -> LocalAgentStatus {
    LocalAgentStatus {
        agent_id: "local-agent-dev".into(),
        status: "stopped".into(),
        current_task_id: None,
        pending_result_count: 0,
    }
}

#[tauri::command]
fn local_agent_status() -> LocalAgentStatus {
    stopped_status()
}

#[tauri::command]
fn local_agent_start() -> LocalAgentStatus {
    LocalAgentStatus {
        status: "running".into(),
        ..stopped_status()
    }
}

#[tauri::command]
fn local_agent_stop() -> LocalAgentStatus {
    stopped_status()
}

#[tauri::command]
fn local_agent_next_status_event() -> LocalAgentStatusEvent {
    LocalAgentStatusEvent {
        event: "status",
        data: stopped_status(),
    }
}

#[tauri::command]
fn local_agent_bind_session(binding_ticket: String) -> Result<BoundNodeFacts, String> {
    let mut transport = M0BindingTransport;
    LocalAgentBridge::bind_session(binding_ticket, &mut transport)
        .map_err(|error| format!("{error:?}"))
}

fn main() {
    println!(
        "wt-media-desktop starting with {} local agent commands",
        LocalAgentBridge::command_names().len()
    );

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            local_agent_status,
            local_agent_start,
            local_agent_stop,
            local_agent_next_status_event,
            local_agent_bind_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running wt-media-desktop tauri application");
}
