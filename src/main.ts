import { createApp } from "vue";
import App from "./App.vue";
import { createLocalAgentStatusPage } from "./local-pages/local-agent-status.js";
import { createLocalAgentService, createMockLocalAgentService } from "./services/local-agent.js";
import { createLocalAgentStore } from "./stores/local-agent-store.js";

// In standalone Vite dev mode, use HTTP calls against the Local Agent.
function createHttpLocalAgentService() {
  const BASE = "http://127.0.0.1:8765";

  async function fetchJson(path) {
    try {
      const resp = await fetch(`${BASE}${path}`);
      if (!resp.ok) return null;
      return resp.json();
    } catch {
      return null;
    }
  }

  const DEFAULT = { agent_id: "local-agent-dev", status: "stopped", current_task_id: null, current_task_progress: null, current_task_status: null, pending_result_count: 0 };

  return {
    async status() {
      // Agent returns {data: {agent_id, status, ...}} or flat {agent_id, status, ...}.
      let body = await fetchJson("/api/v1/status");
      const src = body?.data ?? body;
      if (src) {
        return {
          agent_id: String(src.agent_id ?? DEFAULT.agent_id),
          status: String(src.status ?? DEFAULT.status),
          current_task_id: src.current_task_id ?? null,
          current_task_progress: src.current_task_progress ?? null,
          current_task_status: src.current_task_status ?? null,
          pending_result_count: Number(src.pending_result_count ?? 0),
        };
      }
      return { ...DEFAULT };
    },
    async health() {
      const body = await fetchJson("/healthz");
      return body?.status ?? "unreachable";
    },
    async start() { return "start_requested"; },
    async stop() { return "stop_requested"; },
    async taskStatus(_taskId) { return { ...DEFAULT, current_task_id: _taskId }; },
    async bind() { return { id: "node-http", agent_id: "local-agent-dev", user_id: "", status: "bound" }; },
  };
}

function isTauri() {
  return typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined;
}

export async function createDesktopStatusPreview() {
  let service;

  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    service = createLocalAgentService({ invoke });
  } else {
    // Standalone Vite dev — use HTTP directly against the local Agent.
    service = createHttpLocalAgentService();
  }

  const store = createLocalAgentStore({ service });
  await store.refresh();
  const snapshot = store.snapshot();
  console.info(`wt-media-desktop local agent: ${snapshot.status}`);
  return createLocalAgentStatusPage(snapshot);
}

createApp(App).mount("#app");
