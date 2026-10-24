<script setup lang="ts">
// deepaisee 主界面：搜索 / 树状图 / 扫描索引 / 模型 四模式 + 设置抽屉 + 状态栏
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { api } from "./api";
import { refreshInfo, refreshModel, refreshStats, store } from "./store";
import SearchView from "./views/SearchView.vue";
import TreeView from "./views/TreeView.vue";
import ScanView from "./views/ScanView.vue";
import ModelsView from "./views/ModelsView.vue";
import SettingsDrawer from "./components/SettingsDrawer.vue";

const mode = ref<"search" | "tree" | "scan" | "models">("search");
const drawer = ref(false);
const drawerTab = ref("settings");
const thumbDir = ref("");
const indexLabel = ref("");

const searchRef = ref<InstanceType<typeof SearchView> | null>(null);

const MODES = [
  { k: "search", t: "搜索 / 相似", icon: "⌕" },
  { k: "tree", t: "树状图", icon: "▦" },
  { k: "scan", t: "扫描 / 索引", icon: "⟳" },
  { k: "models", t: "模型", icon: "◆" },
] as const;

function openDrawer(tab = "settings") {
  drawerTab.value = tab;
  drawer.value = true;
}

let tick: number | undefined;

onMounted(async () => {
  await refreshInfo();
  await refreshModel();
  await refreshStats();
  try {
    const dir = await api.dataDir();
    thumbDir.value = `${dir}\\thumbs`;
  } catch {}
  tick = window.setInterval(async () => {
    await refreshStats();
    await refreshModel(); // 后端是异步加载模型的，加载完成后要自动反映到状态栏
    const s = store.stats;
    indexLabel.value = s ? `索引 ${s.files.toLocaleString()} 个文件 · ${s.bytes_label}` : "";
  }, 3000);
});

onBeforeUnmount(() => {
  if (tick) window.clearInterval(tick);
});

const modelText = computed(() => {
  const m = store.model;
  if (!m) return "模型未就绪";
  if (!m.ready) return "模型未加载";
  return `${m.backend} · ${m.dim} 维 · ${m.device}`;
});

const modelOk = computed(() => !!store.model?.ready);
</script>

<template>
  <div class="app">
    <!-- 顶栏 -->
    <header class="top">
      <div class="brand">
        <span class="logo">deepaisee</span>
        <span class="ver">v{{ store.info?.display_version || "0.01" }}</span>
      </div>
      <nav class="modes">
        <button
          v-for="m in MODES"
          :key="m.k"
          class="mode"
          :class="{ active: mode === m.k }"
          @click="mode = m.k as any"
        >
          <span class="ic">{{ m.icon }}</span>{{ m.t }}
        </button>
      </nav>
      <span class="grow"></span>
      <span class="stat muted">{{ indexLabel }}</span>
      <button class="ghost" @click="openDrawer('lan')">局域网</button>
      <button class="ghost" @click="openDrawer('log')">日志</button>
      <button class="ghost" @click="mode = 'models'">模型</button>
      <button class="primary" @click="openDrawer('settings')">设置</button>
    </header>

    <!-- 主体 -->
    <main class="body">
      <div class="pane">
        <SearchView v-show="mode === 'search'" ref="searchRef" :thumb-dir="thumbDir" />
        <TreeView v-if="mode === 'tree'" />
        <ScanView v-if="mode === 'scan'" />
        <ModelsView v-if="mode === 'models'" />
      </div>
      <SettingsDrawer
        :open="drawer"
        :tab="drawerTab"
        @close="drawer = false"
        @tab="drawerTab = $event"
      />
    </main>

    <!-- 状态栏 -->
    <footer class="foot">
      <span class="dot" :class="{ on: modelOk }"></span>
      <span class="mono">{{ modelText }}</span>
      <span class="sep">|</span>
      <span class="muted" v-if="store.stats">
        {{ store.stats.files.toLocaleString() }} 个文件 ·
        {{ store.stats.bytes_label }} ·
        正文 {{ store.stats.text_ok }} · 向量 {{ store.stats.embed_ok }}
      </span>
      <span class="grow"></span>
      <span class="muted tiny">本地离线 · 数据不出机器</span>
    </footer>

    <!-- 提示条 -->
    <div class="toasts">
      <div v-for="t in store.toasts" :key="t.id" class="toast" :class="t.level">
        {{ t.text }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.app {
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.top {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  background: linear-gradient(180deg, #131b2b, #0e1420);
  flex: none;
}
.brand { display: flex; align-items: baseline; gap: 8px; }
.logo {
  font-size: 19px;
  font-weight: 700;
  letter-spacing: .5px;
  background: linear-gradient(90deg, #6ea8ff, #22c55e);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}
.ver { font-size: 11px; color: var(--muted); }
.modes { display: flex; gap: 4px; }
.mode {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: transparent;
  border: 1px solid transparent;
  padding: 6px 14px;
  border-radius: 8px;
  color: var(--muted);
  font-size: 13px;
}
.mode:hover { background: rgba(79, 140, 255, .08); color: var(--text); }
.mode.active {
  background: rgba(79, 140, 255, .16);
  border-color: rgba(79, 140, 255, .45);
  color: #dce8ff;
}
.ic { font-size: 14px; }
.grow { flex: 1; }
.stat { font-size: 12px; }
.body { flex: 1; position: relative; min-height: 0; display: flex; }
.pane { flex: 1; padding: 10px 12px; min-width: 0; min-height: 0; }
.pane > * { height: 100%; }
.foot {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 14px;
  border-top: 1px solid var(--border);
  background: #0d131e;
  font-size: 11px;
  color: var(--muted);
  flex: none;
}
.dot { width: 7px; height: 7px; border-radius: 50%; background: #64748b; }
.dot.on { background: #22c55e; box-shadow: 0 0 8px rgba(34, 197, 94, .7); }
.sep { color: #33415c; }
.tiny { font-size: 11px; }
.toasts {
  position: fixed;
  right: 16px;
  bottom: 40px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  z-index: 90;
  pointer-events: none;
}
.toast {
  background: #1b2436;
  border: 1px solid var(--border);
  border-left: 3px solid var(--accent);
  border-radius: 8px;
  padding: 8px 14px;
  font-size: 12px;
  max-width: 460px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, .5);
}
.toast.ok { border-left-color: #22c55e; }
.toast.warn { border-left-color: #f59e0b; }
.toast.error { border-left-color: #ef4444; }
</style>
