import { existsSync, readFileSync } from "node:fs";

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function readText(path) {
  return readFileSync(path, "utf8");
}

const pkg = readJson("package.json");
const tauri = readJson("src-tauri/tauri.conf.json");
const lock = readJson("contracts.lock.json");

const requiredFiles = [
  "src/services/local-agent.js",
  "src/stores/local-agent-store.js",
  "src/local-pages/local-agent-status.js",
];

const mainSource = readText("src/main.ts");
const nativeBridgeSource = readText("src-tauri/src/local_agent/mod.rs");

const checks = [
  ["package name", pkg.name === "wt-media-desktop"],
  ["tauri product", tauri.productName === "WT Media"],
  ["tauri window", Array.isArray(tauri.app?.windows) && tauri.app.windows.length > 0],
  ["local agent files", requiredFiles.every((path) => existsSync(path))],
  ["main uses local agent status page", mainSource.includes("createLocalAgentStatusPage")],
  ["local agent api lock", lock.consumes?.local_agent_api === "v1@2026.07.14.7"],
  ["local event schema lock", lock.consumes?.local_event_schemas === "profile-guard@2026.07.14.8"],
  ["cloud agent api lock", lock.consumes?.cloud_agent_api === "v1@2026.07.14.7"],
  ["binding command declared", readText("src/services/local-agent.js").includes("local_agent_bind_session")],
  ["native binding transport declared", nativeBridgeSource.includes("consume_binding_ticket")],
  ["native binding result excludes credentials", nativeBridgeSource.includes("BoundNodeFacts") && !nativeBridgeSource.includes("node_credential")],
];

const failed = checks.filter(([, ok]) => !ok).map(([name]) => name);

if (failed.length > 0) {
  console.error(`wt-media-desktop health failed: ${failed.join(", ")}`);
  process.exit(1);
}

const { createMockLocalAgentService } = await import("../src/services/local-agent.js");
const { createLocalAgentStore } = await import("../src/stores/local-agent-store.js");
const { createLocalAgentStatusPage } = await import("../src/local-pages/local-agent-status.js");

const service = createMockLocalAgentService();
const store = createLocalAgentStore({ service });
await store.refresh();

const startResult = await store.start();
const runningPage = createLocalAgentStatusPage(store.snapshot());

await store.applyEvent({
  event: "status",
  data: {
    agent_id: "local-agent-dev",
    status: "idle",
    current_task_id: null,
    pending_result_count: 2,
  },
});

const idlePage = createLocalAgentStatusPage(store.snapshot());
const stopResult = await store.stop();

const behaviorChecks = [
  ["start action", startResult.status === "running"],
  ["running display", runningPage.primaryStatus === "Running"],
  ["pending result display", idlePage.pendingResultText === "2 pending results"],
  ["stop action", stopResult.status === "stopped"],
  ["no direct local agent url", !requiredFiles.some((path) => /127\\.0\\.0\\.1|localhost|local_token/i.test(readText(path)))],
];

const invokeCalls = [];
const nativeService = (await import("../src/services/local-agent.js")).createLocalAgentService({
  async invoke(command, payload) {
    invokeCalls.push({ command, payload });
    return { id: "node-1", agent_id: "local-agent-dev", user_id: "user-1", status: "online" };
  },
});
const boundNode = await nativeService.bindSession("one-use-ticket");
behaviorChecks.push(
  ["binding ticket passed once", invokeCalls.length === 1 && invokeCalls[0].payload?.bindingTicket === "one-use-ticket"],
  ["binding returns no credential", boundNode.id === "node-1" && !("node_credential" in boundNode)],
  ["binding ticket not persisted in service", !/localStorage|sessionStorage|writeFile|console\.(?:log|info).*binding/i.test(readText("src/services/local-agent.js"))],
);

const behaviorFailed = behaviorChecks.filter(([, ok]) => !ok).map(([name]) => name);

if (behaviorFailed.length > 0) {
  console.error(`wt-media-desktop local agent behavior failed: ${behaviorFailed.join(", ")}`);
  process.exit(1);
}

console.log("wt-media-desktop health ok");
