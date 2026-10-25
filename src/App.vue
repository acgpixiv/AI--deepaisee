<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { Database, Search, Settings2 } from "lucide-vue-next";
import { api } from "./api";
import { refreshInfo, refreshStats, store } from "./store";
import SearchView from "./views/SearchView.vue";
import TreeView from "./views/TreeView.vue";
import SettingsDrawer from "./components/SettingsDrawer.vue";

type Page = "home" | "tree" | "settings";
const page = ref<Page>("home");
const settingsTab = ref("index");
const thumbDir = ref("");
let statsTimer: number | undefined;

function showSettings(tab = "index") {
  settingsTab.value = tab;
  page.value = "settings";
}

onMounted(async () => {
  await Promise.all([refreshInfo(), refreshStats()]);
  try {
    thumbDir.value = `${await api.dataDir()}\\thumbs`;
  } catch {}
  statsTimer = window.setInterval(refreshStats, 5000);
});

onBeforeUnmount(() => {
  if (statsTimer) window.clearInterval(statsTimer);
});
</script>

<template>
  <div class="app-shell">
    <header class="topbar">
      <button
        v-if="page === 'settings'"
        class="search-link"
        aria-label="文件搜索"
        @click="page = 'home'"
      >
        <Search :size="18" aria-hidden="true" />
        <span>文件搜索</span>
      </button>
      <button
        class="index-link"
        :class="{ current: page === 'tree' }"
        :title="page === 'tree' ? '返回搜索主页' : '查看目录树状图'"
        @click="page = page === 'tree' ? 'home' : 'tree'"
      >
        <Database :size="15" aria-hidden="true" />
        <span>
          {{ (store.stats?.files ?? 0).toLocaleString() }} 个文件
          <span class="index-divider">·</span>
          {{ store.stats?.bytes_label ?? "0 B" }}
        </span>
      </button>
      <button class="icon-button settings-button" title="设置" aria-label="设置" @click="page === 'settings' ? (page = 'home') : showSettings()">
        <Settings2 :size="19" aria-hidden="true" />
      </button>
    </header>

    <main class="app-content">
      <SearchView v-if="page === 'home'" :thumb-dir="thumbDir" />
      <TreeView v-else-if="page === 'tree'" />
      <SettingsDrawer
        v-else
        :open="true"
        :tab="settingsTab"
        @tab="settingsTab = $event"
      />
    </main>

    <div class="toasts" aria-live="polite">
      <div v-for="item in store.toasts" :key="item.id" class="toast" :class="item.level">
        {{ item.text }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.app-shell { height: 100%; display: flex; flex-direction: column; overflow: hidden; }
.topbar {
  height: 54px;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 0 22px;
  border-bottom: 1px solid var(--border);
  background: var(--bg);
  flex: none;
}
.search-link {
  width: 36px;
  height: 36px;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: flex-start;
  gap: 8px;
  padding: 0 8px;
  overflow: hidden;
  border: 1px solid transparent;
  background: transparent;
  color: var(--text);
  white-space: nowrap;
  transition: width .16s ease, border-color .16s ease, background .16s ease;
}
.search-link :deep(svg) { width: 18px; height: 18px; flex: none; }
.search-link span { width: 0; overflow: hidden; opacity: 0; transition: width .16s ease, opacity .16s ease; }
.search-link:hover, .search-link:focus-visible { width: 112px; border-color: var(--border); background: var(--panel); }
.search-link:hover span, .search-link:focus-visible span { width: 56px; opacity: 1; }
.index-link {
  min-width: 210px;
  margin-left: auto;
  display: inline-flex;
  justify-content: center;
  align-items: center;
  gap: 9px;
  padding: 7px 10px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--muted);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
.index-link:hover, .index-link.current { background: var(--panel); border-color: var(--border); color: var(--text); }
.index-divider { padding: 0 3px; color: #59615d; }
.index-link :deep(svg:last-child) { color: #707a73; }
.icon-button {
  width: 36px;
  height: 36px;
  display: inline-grid;
  place-items: center;
  padding: 0;
  border: 1px solid transparent;
  background: transparent;
  color: var(--muted);
}
.icon-button:hover { background: var(--panel); border-color: var(--border); color: var(--text); }
.settings-button { margin-left: 4px; }
.app-content { flex: 1; min-height: 0; min-width: 0; overflow: hidden; }
.toasts { position: fixed; right: 18px; bottom: 18px; z-index: 100; display: flex; flex-direction: column; align-items: flex-end; gap: 8px; pointer-events: none; }
.toast { max-width: min(420px, calc(100vw - 36px)); padding: 9px 12px; border: 1px solid var(--border); border-radius: 4px; background: #fff; color: var(--text); box-shadow: 0 5px 18px rgba(0, 0, 0, .12); font-size: 12px; }
.toast.ok { border-color: #85b894; background: #f1faf3; color: #245a32; }
.toast.warn { border-color: #cfad6b; background: #fffaf0; color: #715414; }
.toast.error { border-color: #d58b86; background: #fff5f4; color: #7b2b27; }
@media (max-width: 560px) {
  .topbar { padding: 0 12px; }
  .index-link { min-width: 0; gap: 6px; padding-inline: 7px; }
  .index-link span { max-width: 175px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
}
</style>
