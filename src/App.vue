<script setup lang="ts">
import { onMounted, ref } from "vue";
import { createDesktopStatusPreview } from "./main";

type DesktopStatusPreview = Awaited<ReturnType<typeof createDesktopStatusPreview>>;

const page = ref<DesktopStatusPreview | null>(null);

onMounted(async () => {
  page.value = await createDesktopStatusPreview();
});
</script>

<template>
  <main class="desktop-shell">
    <section class="status-card">
      <p class="eyebrow">WT Media Desktop</p>
      <h1>Local Agent 控制台</h1>
      <p class="summary">
        M0 最小闭环页面已启动：桌面壳、Vue 页面和 Local Agent 状态边界可以被本地健康检查访问。
      </p>

      <dl v-if="page" class="facts">
        <div>
          <dt>Agent</dt>
          <dd>{{ page.agentId }}</dd>
        </div>
        <div>
          <dt>Status</dt>
          <dd>{{ page.primaryStatus }}</dd>
        </div>
        <div>
          <dt>Current Task</dt>
          <dd>{{ page.currentTaskText }}</dd>
        </div>
        <div>
          <dt>Pending Results</dt>
          <dd>{{ page.pendingResultText }}</dd>
        </div>
      </dl>

      <p v-else class="loading">Loading local agent status…</p>
    </section>
  </main>
</template>

<style scoped>
.desktop-shell {
  min-height: 100vh;
  display: grid;
  place-items: center;
  margin: 0;
  background:
    radial-gradient(circle at top left, rgba(67, 97, 238, 0.28), transparent 34rem),
    linear-gradient(135deg, #101828 0%, #172033 50%, #0f172a 100%);
  color: #f8fafc;
  font-family:
    Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}

.status-card {
  width: min(760px, calc(100vw - 48px));
  padding: 40px;
  border: 1px solid rgba(148, 163, 184, 0.28);
  border-radius: 28px;
  background: rgba(15, 23, 42, 0.78);
  box-shadow: 0 28px 80px rgba(2, 6, 23, 0.46);
}

.eyebrow {
  margin: 0 0 12px;
  color: #93c5fd;
  font-size: 13px;
  font-weight: 700;
  letter-spacing: 0.18em;
  text-transform: uppercase;
}

h1 {
  margin: 0;
  font-size: clamp(34px, 6vw, 64px);
  line-height: 1;
}

.summary {
  max-width: 620px;
  margin: 20px 0 32px;
  color: #cbd5e1;
  font-size: 18px;
  line-height: 1.7;
}

.facts {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 16px;
  margin: 0;
}

.facts div {
  padding: 18px;
  border-radius: 18px;
  background: rgba(30, 41, 59, 0.78);
}

dt {
  color: #94a3b8;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
}

dd {
  margin: 8px 0 0;
  color: #ffffff;
  font-size: 20px;
  font-weight: 700;
}

.loading {
  color: #bfdbfe;
}
</style>
