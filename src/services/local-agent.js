// Real Tauri invoke-based Local Agent service (M1-R5).
// Replaces the M0 mock service. Tokens and credentials are never held in Vue.

export const LOCAL_AGENT_COMMANDS = Object.freeze({
  status: "local_agent_status",
  health: "local_agent_health",
  start: "local_agent_start",
  stop: "local_agent_stop",
  taskStatus: "local_agent_task_status",
  bind: "local_agent_bind",
});

const DEFAULT_STATUS = Object.freeze({
  agent_id: "local-agent-dev",
  status: "stopped",
  current_task_id: null,
  current_task_progress: null,
  current_task_status: null,
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
    current_task_progress: source.current_task_progress ?? null,
    current_task_status: source.current_task_status ?? null,
    pending_result_count: Number(source.pending_result_count ?? 0),
  };
}

export function normalizeBoundNode(value) {
  const source = value && typeof value === "object" ? value : {};
  return {
    id: String(source.node_id ?? ""),
    agent_id: String(source.agent_id ?? ""),
    user_id: String(source.user_id ?? ""),
    status: String(source.status ?? "unknown"),
  };
}

// Real Tauri invoke-based service.
// Falls back to mock when Tauri is unavailable (Vite dev mode).
export function createLocalAgentService({ invoke }) {
  if (typeof invoke !== "function") {
    return createMockLocalAgentService();
  }

  return {
    async status() {
      return normalizeLocalAgentStatus(await invoke(LOCAL_AGENT_COMMANDS.status));
    },
    async health() {
      return invoke(LOCAL_AGENT_COMMANDS.health);
    },
    async start() {
      return invoke(LOCAL_AGENT_COMMANDS.start);
    },
    async stop() {
      return invoke(LOCAL_AGENT_COMMANDS.stop);
    },
    async taskStatus(taskId) {
      return normalizeLocalAgentStatus(
        await invoke(LOCAL_AGENT_COMMANDS.taskStatus, { taskId })
      );
    },
    async bind() {
      const result = await invoke(LOCAL_AGENT_COMMANDS.bind);
      return normalizeBoundNode(result);
    },
  };
}

// Mock service for standalone Vite dev when Tauri is not available.
export function createMockLocalAgentService(initialStatus = DEFAULT_STATUS) {
  let current = normalizeLocalAgentStatus(initialStatus);

  return {
    async status() {
      return clone(current);
    },
    async health() {
      return "ok";
    },
    async start() {
      current = normalizeLocalAgentStatus({ ...current, status: "running" });
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
    async taskStatus(_taskId) {
      return clone({ ...current, current_task_id: _taskId });
    },
    async bind() {
      return { id: "node-mock", agent_id: current.agent_id, user_id: "user-mock", status: "bound" };
    },
  };
}
