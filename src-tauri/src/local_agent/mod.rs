//! Local Agent lifecycle and observation command boundary.

use serde::Serialize;

pub const STATUS_COMMAND: &str = "local_agent_status";
pub const START_COMMAND: &str = "local_agent_start";
pub const STOP_COMMAND: &str = "local_agent_stop";
pub const NEXT_STATUS_EVENT_COMMAND: &str = "local_agent_next_status_event";
pub const BIND_SESSION_COMMAND: &str = "local_agent_bind_session";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BoundNodeFacts {
    pub id: String,
    pub agent_id: String,
    pub user_id: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BindSessionError {
    EmptyTicket,
    Transport(String),
}

/// Owns the Local Agent registration exchange. Implementations must retain any
/// resulting credential in native secure state and return only non-secret facts.
pub trait BindingTransport {
    fn consume_binding_ticket(
        &mut self,
        binding_ticket: String,
    ) -> Result<BoundNodeFacts, BindSessionError>;
}

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

    /// Moves the one-use ticket into the native transport. The bridge cannot
    /// retain or clone it, and its return type has no field for credentials.
    pub fn bind_session<T: BindingTransport>(
        binding_ticket: String,
        transport: &mut T,
    ) -> Result<BoundNodeFacts, BindSessionError> {
        if binding_ticket.trim().is_empty() {
            return Err(BindSessionError::EmptyTicket);
        }
        transport.consume_binding_ticket(binding_ticket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingTransport {
        tickets: Vec<String>,
    }

    impl BindingTransport for RecordingTransport {
        fn consume_binding_ticket(
            &mut self,
            binding_ticket: String,
        ) -> Result<BoundNodeFacts, BindSessionError> {
            self.tickets.push(binding_ticket);
            Ok(BoundNodeFacts {
                id: "node-1".into(),
                agent_id: "agent-1".into(),
                user_id: "user-1".into(),
                status: "online".into(),
            })
        }
    }

    #[test]
    fn binding_ticket_is_moved_once_and_only_facts_return() {
        let mut transport = RecordingTransport::default();

        let result = LocalAgentBridge::bind_session("ticket-1".into(), &mut transport).unwrap();

        assert_eq!(transport.tickets, vec!["ticket-1"]);
        assert_eq!(result.id, "node-1");
        assert_eq!(result.status, "online");
    }

    #[test]
    fn empty_binding_ticket_is_rejected_before_transport() {
        let mut transport = RecordingTransport::default();

        let result = LocalAgentBridge::bind_session("  ".into(), &mut transport);

        assert_eq!(result, Err(BindSessionError::EmptyTicket));
        assert!(transport.tickets.is_empty());
    }
}
