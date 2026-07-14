//! Local Agent lifecycle and observation command boundary.

pub const STATUS_COMMAND: &str = "local_agent_status";
pub const START_COMMAND: &str = "local_agent_start";
pub const STOP_COMMAND: &str = "local_agent_stop";
pub const NEXT_STATUS_EVENT_COMMAND: &str = "local_agent_next_status_event";
pub const BIND_SESSION_COMMAND: &str = "local_agent_bind_session";

pub struct LocalAgentBridge;

impl LocalAgentBridge {
    pub fn command_names() -> [&'static str; 5] {
        [
            STATUS_COMMAND,
            START_COMMAND,
            STOP_COMMAND,
            NEXT_STATUS_EVENT_COMMAND,
            BIND_SESSION_COMMAND,
        ]
    }
}
