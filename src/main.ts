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
      // Local Agent /api/v1/status returns the status object directly (no {data:...} wrapper).
      let body = await fetchJson("/api/v1/status");
      if (body) {
        return {
          agent_id: String(body.agent_id ?? DEFAULT.agent_id),
          status: String(body.status ?? DEFAULT.status),
          current_task_id: body.current_task_id ?? null,
          current_task_progress: body.current_task_progress ?? null,
          current_task_status: body.current_task_status ?? null,
          pending_result_count: Number(body.pending_result_count ?? 0),
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
