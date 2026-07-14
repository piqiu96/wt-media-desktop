import { createLocalAgentStatusPage } from "./local-pages/local-agent-status.js";
import { createMockLocalAgentService } from "./services/local-agent.js";
import { createLocalAgentStore } from "./stores/local-agent-store.js";

export async function createDesktopStatusPreview() {
  const service = createMockLocalAgentService();
  const store = createLocalAgentStore({ service });
  await store.refresh();
  return createLocalAgentStatusPage(store.snapshot());
}

createDesktopStatusPreview().then((page) => {
  console.info(`wt-media-desktop local agent: ${page.primaryStatus}`);
});
