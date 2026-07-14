export const LOCAL_AGENT_COMMANDS = Object.freeze({
  status: "local_agent_status",
  start: "local_agent_start",
  stop: "local_agent_stop",
  nextStatusEvent: "local_agent_next_status_event",
  bindSession: "local_agent_bind_session",
});

const DEFAULT_STATUS = Object.freeze({
  agent_id: "local-agent-dev",
  status: "stopped",
  current_task_id: null,
  pending_result_count: 0,
});

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

export function normalizeLocalAgentStatus(value) {
  const source = value && typeof value === "object" ? value : {};
  return {
    agent_id: String(source.agent_id ?? DEFAULT_STATUS.agent_id),
    status: String(source.status ?? DEFAULT_STATUS.status),
    current_task_id: source.current_task_id ?? null,
    pending_result_count: Number(source.pending_result_count ?? 0),
  };
}

export function normalizeBoundNode(value) {
  const source = value && typeof value === "object" ? value : {};
  return {
    id: String(source.id ?? ""),
    agent_id: String(source.agent_id ?? ""),
    user_id: String(source.user_id ?? ""),
    status: String(source.status ?? "unknown"),
  };
}

export function createLocalAgentService({ invoke }) {
  if (typeof invoke !== "function") {
    throw new TypeError("createLocalAgentService requires a Tauri invoke function");
  }

  return {
    async status() {
      return normalizeLocalAgentStatus(await invoke(LOCAL_AGENT_COMMANDS.status));
    },
    async start() {
      return normalizeLocalAgentStatus(await invoke(LOCAL_AGENT_COMMANDS.start));
    },
    async stop() {
      return normalizeLocalAgentStatus(await invoke(LOCAL_AGENT_COMMANDS.stop));
    },
    async nextStatusEvent() {
      return invoke(LOCAL_AGENT_COMMANDS.nextStatusEvent);
    },
    async bindSession(bindingTicket) {
      if (typeof bindingTicket !== "string" || bindingTicket.trim() === "") {
        throw new TypeError("bindSession requires a one-use binding ticket");
      }
      const result = await invoke(LOCAL_AGENT_COMMANDS.bindSession, { bindingTicket });
      return normalizeBoundNode(result);
    },
  };
}

export function createMockLocalAgentService(initialStatus = DEFAULT_STATUS) {
  let current = normalizeLocalAgentStatus(initialStatus);

  return {
    async status() {
      return clone(current);
    },
    async start() {
      current = normalizeLocalAgentStatus({
        ...current,
        status: "running",
      });
      return clone(current);
    },
    async stop() {
      current = normalizeLocalAgentStatus({
        ...current,
        status: "stopped",
        current_task_id: null,
      });
      return clone(current);
    },
    async nextStatusEvent() {
      return {
        event: "status",
        data: clone(current),
      };
    },
    async bindSession(bindingTicket) {
      if (typeof bindingTicket !== "string" || bindingTicket.trim() === "") {
        throw new TypeError("bindSession requires a one-use binding ticket");
      }
      return {id: "node-mock", agent_id: current.agent_id, user_id: "user-mock", status: "online"};
    },
  };
}
