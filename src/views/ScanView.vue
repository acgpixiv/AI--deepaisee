<script setup lang="ts">
// 扫描视图：选择目录 → 扫描 → 进度 → 建立索引
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { Check } from "lucide-vue-next";
import { api, type AppConfig, type ScanProgress, type IndexStats, type RootIndexStats } from "../api";
import { refreshStats, store, toast } from "../store";
import { t } from "../i18n";

const cfg = ref<AppConfig | null>(null);
const roots = ref<string[]>([]);
const rootEnabled = ref<Record<string, boolean>>({});
const selectedRoot = ref<string | null>(null);
const rootStats = ref<Record<string, RootIndexStats>>({});
const progress = ref<ScanProgress | null>(null);
const idx = ref<IndexStats | null>(null);
const polling = ref<number | null>(null);
const operationActive = ref(false);
const cancelledIndexPct = ref<number | null>(null);
let lastRootRefresh = 0;

const TYPES = [
  { k: "images", label: "图片" },
  { k: "videos", label: "视频" },
  { k: "audios", label: "音频" },
  { k: "pdf", label: "PDF" },
  { k: "documents", label: "文档" },
  { k: "archives", label: "压缩包" },
  { k: "others", label: "其它" },
];

const typeState = ref<Record<string, boolean>>({
  images: true,
  videos: true,
  audios: true,
  pdf: true,
  documents: true,
  archives: true,
  others: false,
});

async function loadCfg() {
  try {
    cfg.value = await api.getConfig();
    roots.value = [...(cfg.value.scan.roots || [])];
    const disabled = new Set<string>(cfg.value.scan.disabled_roots || []);
    rootEnabled.value = Object.fromEntries(roots.value.map((r) => [r, !disabled.has(r)]));
    const s = cfg.value.scan;
    typeState.value = {
      images: s.images !== false,
      videos: s.videos !== false,
      audios: s.audios !== false,
      pdf: s.pdf !== false,
      documents: s.documents !== false,
      archives: s.archives !== false,
      others: !!s.others,
    };
  } catch (e) {
    toast(`${t("读取配置失败：")}${e}`, "error");
  }
}

const activeRoots = computed(() => roots.value.filter((r) => rootEnabled.value[r] !== false));
const rootKey = (root: string) => {
  let normalized = root.replaceAll("/", "\\").replace(/\\+/g, "\\");
  if (!/^[a-z]:\\$/i.test(normalized)) normalized = normalized.replace(/\\+$/, "");
  return normalized.toLowerCase();
};
const selectedRootStats = computed(() => selectedRoot.value ? rootStats.value[rootKey(selectedRoot.value)] : undefined);

async function refreshRootStats() {
  try {
    const rows = await api.indexRoots();
    rootStats.value = Object.fromEntries(rows.map((row) => [rootKey(row.root), row]));
  } catch {
    rootStats.value = {};
  }
}

async function toggleRoot(root: string) {
  rootEnabled.value[root] = rootEnabled.value[root] === false;
  selectedRoot.value = root;
  try {
    await saveRoots();
  } catch (e) {
    rootEnabled.value[root] = !rootEnabled.value[root];
    toast(`${t("保存目录状态失败：")}${e}`, "error");
  }
}

function rootProgress(stats?: RootIndexStats) {
  return stats ? Math.max(0, Math.min(100, Math.round(stats.index_progress))) : 0;
}

const persistedIndexPct = computed(() => {
  const rows = Object.values(rootStats.value);
  const total = rows.reduce((sum, row) => sum + row.files, 0);
  if (!total) return 0;
  const pending = rows.reduce((sum, row) => sum + row.embed_pending, 0);
  return Math.max(0, Math.min(100, Math.round(((total - pending) / total) * 100)));
});

function formatRootScanTime(value: number) {
  if (!value) return t("尚未扫描");
  return new Date(value * 1000).toLocaleString();
}

async function pickDir() {
  try {
    const p = await open({ directory: true, multiple: true, title: t("选择要扫描的目录") });
    if (!p) return;
    const arr = Array.isArray(p) ? p : [p];
    for (const d of arr) {
      if (!roots.value.includes(d)) {
        roots.value.push(d);
        rootEnabled.value[d] = true;
      }
    }
  } catch (e) {
    toast(`${t("选择目录失败：")}${e}`, "error");
  }
}

function removeRoot(r: string) {
  roots.value = roots.value.filter((x) => x !== r);
  delete rootEnabled.value[r];
  if (selectedRoot.value === r) selectedRoot.value = null;
}

async function saveRoots() {
  await api.saveConfig({
    scan: {
      roots: roots.value,
      disabled_roots: roots.value.filter((r) => rootEnabled.value[r] === false),
    },
  });
  await refreshRootStats();
}

async function startScan() {
  if (!activeRoots.value.length) return toast(t("至少启用一个扫描目录"), "warn");
  await api.saveConfig({
    scan: {
      roots: roots.value,
      disabled_roots: roots.value.filter((r) => rootEnabled.value[r] === false),
      images: typeState.value.images,
      videos: typeState.value.videos,
      audios: typeState.value.audios,
      pdf: typeState.value.pdf,
      documents: typeState.value.documents,
      archives: typeState.value.archives,
      others: typeState.value.others,
    },
  });
  try {
    await api.scanStart(activeRoots.value, "auto");
    store.scanBusy = true;
    operationActive.value = true;
    startPolling();
    toast(t("开始扫描…"), "ok");
  } catch (e) {
    toast(`${t("扫描失败：")}${e}`, "error");
  }
}

async function cancelScan() {
  cancelledIndexPct.value = indexPct.value;
  // Stop presenting the task as active immediately. The backend cancellation
  // remains cooperative so an in-flight model call can checkpoint safely.
  operationActive.value = false;
  store.scanBusy = false;
  try {
    await api.scanCancel();
    await api.indexCancel();
    toast(t("已取消当前任务，已完成部分会保留"), "warn");
  } catch (e) {
    toast(`${e}`, "error");
  }
}

function startPolling() {
  stopPolling();
  polling.value = window.setInterval(async () => {
    try {
      progress.value = await api.scanProgress();
      idx.value = await api.indexStats();
      if (Date.now() - lastRootRefresh > 2500) {
        lastRootRefresh = Date.now();
        await refreshRootStats();
      }
      if (operationActive.value && (
        (progress.value?.cancelled && !progress.value.running) ||
        (idx.value?.cancelled && !progress.value?.running && !idx.value.running && idx.value.queued === 0) ||
        (progress.value?.done && !progress.value.running
          && idx.value && !idx.value.running && idx.value.queued === 0)
      )) {
        operationActive.value = false;
        store.scanBusy = false;
        await refreshStats();
        await refreshRootStats();
      }
    } catch {
      /* ignore */
    }
  }, 500);
}

function stopPolling() {
  if (polling.value !== null) {
    window.clearInterval(polling.value);
    polling.value = null;
  }
}

async function buildIndex(what: "auto" | "text" | "embed") {
  try {
    if (!activeRoots.value.length) return toast(t("至少启用一个扫描目录"), "warn");
    await api.saveConfig({
      scan: {
        roots: roots.value,
        disabled_roots: roots.value.filter((r) => rootEnabled.value[r] === false),
        images: typeState.value.images,
        videos: typeState.value.videos,
        audios: typeState.value.audios,
        pdf: typeState.value.pdf,
        documents: typeState.value.documents,
        archives: typeState.value.archives,
        others: typeState.value.others,
      },
    });
    cancelledIndexPct.value = null;
    await api.scanStart(activeRoots.value, what);
    operationActive.value = true;
    startPolling();
    toast(`${t("已重新扫描目录，随后继续")}${what === "auto" ? t("全文和向量") : what === "text" ? t("正文") : t("向量")}${t("索引")}`, "ok");
  } catch (e) {
    toast(`${t("索引失败：")}${e}`, "error");
  }
}

const operationBusy = computed(() => operationActive.value || store.scanBusy || !!idx.value?.running || !!idx.value?.queued);

const pct = computed(() => {
  const p = progress.value;
  if (!p) return 0;
  return p.done ? 100 : p.running ? 35 : 0;
});

const indexPct = computed(() => {
  if (cancelledIndexPct.value !== null) return cancelledIndexPct.value;
  if (selectedRoot.value) return rootProgress(selectedRootStats.value);
  const s = idx.value;
  if (s && s.total) {
    return Math.min(100, Math.round((s.completed / s.total) * 100));
  }
  return persistedIndexPct.value;
});

const displayedIndexTotal = computed(() => selectedRoot.value
  ? selectedRootStats.value?.files ?? 0
  : idx.value?.total ?? 0);
const displayedIndexDone = computed(() => selectedRoot.value
  ? selectedRootStats.value
    ? Math.max(0, Math.round(displayedIndexTotal.value * rootProgress(selectedRootStats.value) / 100))
    : 0
  : idx.value?.completed ?? 0);

onMounted(async () => {
  await loadCfg();
  await refreshRootStats();
  startPolling();
});
onBeforeUnmount(stopPolling);
</script>

<template>
  <div class="view">
    <div class="grid">
      <section class="card blk roots-card">
        <h3>{{ t("扫描目录") }}</h3>
        <div class="roots">
          <div
            v-for="r in roots"
            :key="r"
            class="root"
            :class="{ selected: selectedRoot === r, disabled: rootEnabled[r] === false }"
            @click="selectedRoot = r"
          >
            <button
              class="root-toggle"
              type="button"
              role="checkbox"
              :aria-checked="rootEnabled[r] !== false"
              :title="rootEnabled[r] === false ? t('点击启用此目录') : t('点击跳过此目录')"
              @click.stop="toggleRoot(r)"
            ><Check v-if="rootEnabled[r] !== false" :size="16" :stroke-width="3" /></button>
            <span class="mono root-name">{{ r }}</span>
            <button class="mini" @click.stop="removeRoot(r)">{{ t("移除") }}</button>
          </div>
          <div v-if="!roots.length" class="none">{{ t("还没有添加目录") }}</div>
        </div>
        <div class="row">
          <button class="primary" @click="pickDir">{{ t("+ 添加目录") }}</button>
          <button class="ghost" @click="saveRoots">{{ t("保存") }}</button>
        </div>
        <div v-if="selectedRoot" class="root-detail">
          <div class="root-detail-head"><b>{{ selectedRoot }}</b><span>{{ rootEnabled[selectedRoot] === false ? t("已跳过") : t("已启用") }}</span></div>
          <div class="root-detail-grid">
            <span>{{ t("文件类型") }}</span><b>{{ selectedRootStats?.kinds_label || t("暂无文件") }}</b>
            <span>{{ t("建立索引") }}</span><b>{{ rootProgress(selectedRootStats) }}%</b>
            <span>{{ t("总文件数") }}</span><b>{{ (selectedRootStats?.files ?? 0).toLocaleString() }}</b>
            <span>{{ t("目录大小") }}</span><b>{{ selectedRootStats?.bytes_label || '0 B' }}</b>
            <span>{{ t("上次扫描") }}</span><b>{{ formatRootScanTime(selectedRootStats?.last_scan ?? 0) }}</b>
            <span>{{ t("新增文件") }}</span><b>{{ (selectedRootStats?.added ?? 0).toLocaleString() }}</b>
            <span>{{ t("更新文件") }}</span><b>{{ (selectedRootStats?.updated ?? 0).toLocaleString() }}</b>
            <span>{{ t("跳过 / 错误") }}</span><b>{{ (selectedRootStats?.skipped ?? 0).toLocaleString() }} / {{ (selectedRootStats?.errors ?? 0).toLocaleString() }}</b>
            <span>{{ t("正文完成 / 待处理") }}</span><b>{{ (selectedRootStats?.text_ok ?? 0).toLocaleString() }} / {{ (selectedRootStats?.text_pending ?? 0).toLocaleString() }}</b>
            <span>{{ t("向量完成 / 待处理") }}</span><b>{{ (selectedRootStats?.embed_ok ?? 0).toLocaleString() }} / {{ (selectedRootStats?.embed_pending ?? 0).toLocaleString() }}</b>
            <span>{{ t("已消失文件") }}</span><b>{{ (selectedRootStats?.missing ?? 0).toLocaleString() }}</b>
          </div>
          <p v-if="selectedRootStats?.last_error" class="root-error">{{ selectedRootStats.last_error }}</p>
        </div>
      </section>

      <section class="card blk types-card">
        <h3>{{ t("文件类型") }}</h3>
        <div class="types">
          <button
            v-for="item in TYPES"
            :key="item.k"
            type="button"
            class="type-tag"
            :class="{ active: typeState[item.k] }"
            :aria-pressed="typeState[item.k]"
            @click="typeState[item.k] = !typeState[item.k]"
          >{{ t(item.label) }}</button>
        </div>
        <p class="muted tiny">
          {{ t("默认跳过系统目录、回收站、node_modules、.git 等；网盘目录可手动加排除。") }}
        </p>
        <div class="row">
          <button class="primary" :disabled="operationBusy" @click="startScan">
            {{ operationBusy ? t("处理中…") : t("扫描并建立索引") }}
          </button>
          <button class="ghost" :disabled="!operationBusy" @click="cancelScan">{{ t("取消") }}</button>
        </div>
      </section>
    </div>

    <section class="card blk grow">
      <h3>{{ t("进度") }}</h3>
      <div class="prog">
        <div class="pbar"><div class="pfill" :class="{ scanning: progress?.running }" :style="{ width: pct + '%' }"></div></div>
        <span class="muted mono">{{ progress?.done ? "100%" : progress?.running ? t("扫描中") : "0%" }}</span>
      </div>
      <div class="stats" v-if="progress">
        <div><b>{{ progress.files.toLocaleString() }}</b><span>{{ t("文件") }}</span></div>
        <div><b>{{ progress.dirs.toLocaleString() }}</b><span>{{ t("目录") }}</span></div>
        <div><b>{{ progress.added.toLocaleString() }}</b><span>{{ t("新增") }}</span></div>
        <div><b>{{ progress.updated.toLocaleString() }}</b><span>{{ t("更新") }}</span></div>
        <div><b>{{ progress.skipped.toLocaleString() }}</b><span>{{ t("跳过") }}</span></div>
        <div><b>{{ progress.errors.toLocaleString() }}</b><span>{{ t("错误") }}</span></div>
        <div :class="{ warn: progress.missing > 0 }">
          <b>{{ (progress.missing || 0).toLocaleString() }}</b><span>{{ t("已消失") }}</span>
        </div>
        <div><b>{{ (progress.bytes / 1048576).toFixed(1) }}</b><span>MB</span></div>
        <div><b>{{ progress.seconds.toFixed(1) }}</b><span>{{ t("秒") }}</span></div>
      </div>
      <div class="cur mono" v-if="progress?.current">正在扫描：{{ progress.current }}</div>

      <h3 class="mt">{{ t("索引") }}</h3>
      <div class="prog">
        <div class="pbar"><div class="pfill index-fill" :style="{ width: indexPct + '%' }"></div></div>
        <span class="muted mono">{{ indexPct }}%</span>
      </div>
      <div class="stats" v-if="idx">
        <div><b>{{ idx.text_done }}</b><span>{{ t("正文已抽取") }}</span></div>
        <div><b>{{ idx.emb_done }}</b><span>{{ t("向量已完成") }}</span></div>
        <div><b>{{ idx.thumb_done }}</b><span>{{ t("缩略图") }}</span></div>
        <div><b>{{ idx.queued }}</b><span>{{ t("队列") }}</span></div>
        <div><b>{{ displayedIndexDone }} / {{ displayedIndexTotal }}</b><span>{{ selectedRoot ? t("所选目录已处理 / 总数") : t("本批次进度") }}</span></div>
        <div><b>{{ idx.emb_fail }}</b><span>{{ t("失败") }}</span></div>
      </div>
      <div class="row">
        <button class="primary" :disabled="operationBusy" @click="buildIndex('auto')">{{ t("建立索引") }}</button>
        <button class="ghost" :disabled="!operationBusy" @click="cancelScan">{{ t("取消") }}</button>
        <button class="ghost" :disabled="operationBusy" @click="buildIndex('text')">{{ t("仅补正文") }}</button>
        <button class="ghost" :disabled="operationBusy" @click="buildIndex('embed')">{{ t("仅补向量") }}</button>
      </div>
      <p class="muted tiny index-help">{{ t("正文用于全文检索，向量用于语义检索。日常使用“扫描并建立索引”即可；只有修改了抽取规则或更换模型时，才需要单独补其中一项。") }}</p>
      <p class="muted tiny processing-files" v-if="idx?.running && idx.current_items?.length">
        正在处理：{{ idx.current_items.join("、") }}
      </p>
    </section>
  </div>
</template>

<style scoped>
.view { display: flex; flex-direction: column; height: 100%; gap: 8px; min-height: 0; overflow: auto; }
.grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
.blk { padding: 14px; }
.roots-card { grid-column: 1 / -1; }
.blk.grow { flex: 1; }
h3 { margin: 0 0 10px; font-size: 13px; color: #b7c4da; font-weight: 600; }
h3.mt { margin-top: 18px; }
.roots { display: flex; flex-direction: column; gap: 6px; }
.root {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  background: var(--panel-2); border: 1px solid var(--border); border-radius: 7px;
  padding: 6px 10px; font-size: 12px;
}
.root.selected { outline: 2px solid #111; outline-offset: -2px; }
.root.disabled { color: #777; background: #f2f2f2; }
.root-toggle { width: 22px; height: 22px; flex: none; display: grid; place-items: center; padding: 0; border: 0; background: transparent; color: #111; font-size: 0; line-height: 0; cursor: pointer; }
.root.disabled .root-toggle { color: #888; }
.root-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mini { padding: 2px 8px; font-size: 11px; }
.none { color: var(--muted); font-size: 12px; padding: 8px 0; }
.row { display: flex; gap: 8px; margin-top: 12px; flex-wrap: wrap; }
.types-card { grid-column: 1 / -1; }
.types-card .row { display: none; }
.types { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); gap: 8px; }
.type-tag { min-width: 0; padding: 7px 5px; border: 1px solid #111; border-radius: 0; background: #e8e8e8; color: #666; font-size: 13px; cursor: pointer; white-space: nowrap; }
.type-tag.active { background: #111; color: #fff; }
.type-tag:focus-visible, .root-toggle:focus-visible { outline: 2px solid #111; outline-offset: 2px; }
.root-detail { margin-top: 12px; border-top: 1px solid #111; padding-top: 10px; }
.root-detail-head { display: flex; justify-content: space-between; gap: 10px; font-size: 12px; }
.root-detail-head span { color: #666; }
.root-detail-grid { display: grid; grid-template-columns: max-content 1fr max-content 1fr; gap: 5px 14px; margin-top: 8px; font-size: 11px; }
.root-detail-grid span { color: #666; }
.root-detail-grid b { overflow-wrap: anywhere; font-weight: 600; }
.root-error { margin: 9px 0 0; color: #8c1d18; font-size: 11px; overflow-wrap: anywhere; }
.prog { display: flex; align-items: center; gap: 10px; }
.pbar { flex: 1; height: 8px; background: #1b2333; border-radius: 5px; overflow: hidden; }
.pfill { height: 100%; background: linear-gradient(90deg, #4f8cff, #22c55e); transition: width .3s; }
.stats { display: flex; gap: 22px; flex-wrap: wrap; margin-top: 14px; }
.stats div { display: flex; flex-direction: column; gap: 2px; }
.stats b { font-size: 18px; font-weight: 600; }
.stats span { font-size: 11px; color: var(--muted); }
.stats .warn b { color: var(--warn); }
.cur { margin-top: 10px; font-size: 11px; color: var(--muted); overflow: hidden; overflow-wrap: anywhere; line-height: 1.45; max-height: 3.1em; }
.tiny { font-size: 11px; }
.processing-files { margin: 8px 0 0; line-height: 1.45; overflow-wrap: anywhere; }
/* Flat white/black scan surface. */
.view { color: #111; }
.blk { border: 1px solid #111; border-radius: 2px; background: #fff; }
h3 { color: #111; }
.root { border-color: #111; border-radius: 2px; background: #fff; }
.root.disabled { background: #eee; }
.pbar { background: #e9e9e9; border: 1px solid #111; border-radius: 0; }
.pfill { background: #111; }
.index-fill { background: #555; }
.pfill.scanning { animation: scan-progress 1.35s ease-in-out infinite alternate; }
@keyframes scan-progress { from { transform: translateX(0); } to { transform: translateX(185%); } }
.stats b { color: #111; }
.stats span, .cur { color: #555; }
.stats .warn b { color: #8c6410; }
</style>
