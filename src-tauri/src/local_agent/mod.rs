//! Local Agent lifecycle and observation command boundary.

pub const STATUS_COMMAND: &str = "local_agent_status";
pub const START_COMMAND: &str = "local_agent_start";
pub const STOP_COMMAND: &str = "local_agent_stop";
pub const NEXT_STATUS_EVENT_COMMAND: &str = "local_agent_next_status_event";

pub struct LocalAgentBridge;

impl LocalAgentBridge {
    pub fn command_names() -> [&'static str; 4] {
        [
            STATUS_COMMAND,
            START_COMMAND,
            STOP_COMMAND,
            NEXT_STATUS_EVENT_COMMAND,
        ]
    }
}
