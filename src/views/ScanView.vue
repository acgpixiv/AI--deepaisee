<script setup lang="ts">
// 扫描视图：选择目录 → 扫描 → 进度 → 建立索引
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type AppConfig, type ScanProgress, type IndexStats } from "../api";
import { refreshStats, store, toast } from "../store";

const cfg = ref<AppConfig | null>(null);
const roots = ref<string[]>([]);
const progress = ref<ScanProgress | null>(null);
const idx = ref<IndexStats | null>(null);
const polling = ref<number | null>(null);

const TYPES = [
  { k: "images", t: "图片" },
  { k: "videos", t: "视频" },
  { k: "audios", t: "音频" },
  { k: "documents", t: "文档" },
  { k: "archives", t: "压缩包" },
  { k: "others", t: "其它" },
];

const typeState = ref<Record<string, boolean>>({
  images: true,
  videos: true,
  audios: true,
  documents: true,
  archives: true,
  others: false,
});

async function loadCfg() {
  try {
    cfg.value = await api.getConfig();
    roots.value = [...(cfg.value.scan.roots || [])];
    const s = cfg.value.scan;
    typeState.value = {
      images: s.images !== false,
      videos: s.videos !== false,
      audios: s.audios !== false,
      documents: s.documents !== false,
      archives: s.archives !== false,
      others: !!s.others,
    };
  } catch (e) {
    toast(`读取配置失败：${e}`, "error");
  }
}

async function pickDir() {
  try {
    const p = await open({ directory: true, multiple: true, title: "选择要扫描的目录" });
    if (!p) return;
    const arr = Array.isArray(p) ? p : [p];
    for (const d of arr) if (!roots.value.includes(d)) roots.value.push(d);
  } catch (e) {
    toast(`选择目录失败：${e}`, "error");
  }
}

function removeRoot(r: string) {
  roots.value = roots.value.filter((x) => x !== r);
}

async function saveRoots() {
  if (!cfg.value) return;
  cfg.value.scan.roots = roots.value;
  await api.saveConfig(cfg.value);
}

async function startScan() {
  if (!roots.value.length) return toast("请先添加要扫描的目录", "warn");
  await saveRoots();
  if (cfg.value) {
    const s = cfg.value.scan;
    s.images = typeState.value.images;
    s.videos = typeState.value.videos;
    s.audios = typeState.value.audios;
    s.documents = typeState.value.documents;
    s.archives = typeState.value.archives;
    s.others = typeState.value.others;
    await api.saveConfig(cfg.value);
  }
  try {
    await api.scanStart(roots.value);
    store.scanBusy = true;
    startPolling();
    toast("开始扫描…", "ok");
  } catch (e) {
    toast(`扫描失败：${e}`, "error");
  }
}

async function cancelScan() {
  try {
    await api.scanCancel();
    toast("已请求取消扫描", "warn");
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
      if (progress.value && !progress.value.running) {
        store.scanBusy = false;
        await refreshStats();
      }
      if (idx.value && !idx.value.running && idx.value.queued === 0) {
        await refreshStats();
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
    const n = await api.indexStart(what);
    toast(`已入队 ${n} 个文件（${what === "auto" ? "全文+向量" : what}）`, "ok");
  } catch (e) {
    toast(`索引失败：${e}`, "error");
  }
}

const pct = computed(() => {
  const p = progress.value;
  if (!p || !p.files) return 0;
  return Math.min(100, Math.round(((p.added + p.updated + p.skipped) / p.files) * 100));
});

onMounted(async () => {
  await loadCfg();
  startPolling();
});
onBeforeUnmount(stopPolling);
</script>

<template>
  <div class="view">
    <div class="grid">
      <section class="card blk">
        <h3>扫描目录</h3>
        <div class="roots">
          <div v-for="r in roots" :key="r" class="root">
            <span class="mono">{{ r }}</span>
            <button class="mini" @click="removeRoot(r)">移除</button>
          </div>
          <div v-if="!roots.length" class="none">还没有添加目录</div>
        </div>
        <div class="row">
          <button class="primary" @click="pickDir">+ 添加目录</button>
          <button class="ghost" @click="saveRoots">保存</button>
        </div>
      </section>

      <section class="card blk">
        <h3>文件类型</h3>
        <div class="types">
          <label v-for="t in TYPES" :key="t.k" class="chk">
            <input type="checkbox" v-model="typeState[t.k]" />
            {{ t.t }}
          </label>
        </div>
        <p class="muted tiny">
          默认跳过系统目录、回收站、node_modules、.git 等；网盘目录可手动加排除。
        </p>
        <div class="row">
          <button class="primary" :disabled="store.scanBusy" @click="startScan">
            {{ store.scanBusy ? "扫描中…" : "开始扫描" }}
          </button>
          <button class="ghost" :disabled="!store.scanBusy" @click="cancelScan">取消</button>
        </div>
      </section>
    </div>

    <section class="card blk grow">
      <h3>进度</h3>
      <div class="prog">
        <div class="pbar"><div class="pfill" :style="{ width: pct + '%' }"></div></div>
        <span class="muted mono">{{ pct }}%</span>
      </div>
      <div class="stats" v-if="progress">
        <div><b>{{ progress.files.toLocaleString() }}</b><span>文件</span></div>
        <div><b>{{ progress.dirs.toLocaleString() }}</b><span>目录</span></div>
        <div><b>{{ progress.added.toLocaleString() }}</b><span>新增</span></div>
        <div><b>{{ progress.updated.toLocaleString() }}</b><span>更新</span></div>
        <div><b>{{ progress.skipped.toLocaleString() }}</b><span>跳过</span></div>
        <div><b>{{ progress.errors.toLocaleString() }}</b><span>错误</span></div>
        <div :class="{ warn: progress.missing > 0 }">
          <b>{{ (progress.missing || 0).toLocaleString() }}</b><span>已消失</span>
        </div>
        <div><b>{{ (progress.bytes / 1048576).toFixed(1) }}</b><span>MB</span></div>
        <div><b>{{ progress.seconds.toFixed(1) }}</b><span>秒</span></div>
      </div>
      <div class="cur mono" v-if="progress?.current">正在扫描：{{ progress.current }}</div>

      <h3 class="mt">索引</h3>
      <div class="stats" v-if="idx">
        <div><b>{{ idx.text_done }}</b><span>正文已抽取</span></div>
        <div><b>{{ idx.emb_done }}</b><span>向量已完成</span></div>
        <div><b>{{ idx.thumb_done }}</b><span>缩略图</span></div>
        <div><b>{{ idx.queued }}</b><span>队列</span></div>
        <div><b>{{ idx.emb_fail }}</b><span>失败</span></div>
      </div>
      <div class="row">
        <button @click="buildIndex('auto')">重建索引（全文+向量）</button>
        <button class="ghost" @click="buildIndex('text')">只抽正文</button>
        <button class="ghost" @click="buildIndex('embed')">只算向量</button>
      </div>
      <p class="muted tiny" v-if="idx?.current">正在处理：{{ idx.current }}</p>
    </section>
  </div>
</template>

<style scoped>
.view { display: flex; flex-direction: column; height: 100%; gap: 8px; min-height: 0; overflow: auto; }
.grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
.blk { padding: 14px; }
.blk.grow { flex: 1; }
h3 { margin: 0 0 10px; font-size: 13px; color: #b7c4da; font-weight: 600; }
h3.mt { margin-top: 18px; }
.roots { display: flex; flex-direction: column; gap: 6px; max-height: 150px; overflow: auto; }
.root {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  background: var(--panel-2); border: 1px solid var(--border); border-radius: 7px;
  padding: 6px 10px; font-size: 12px;
}
.root span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mini { padding: 2px 8px; font-size: 11px; }
.none { color: var(--muted); font-size: 12px; padding: 8px 0; }
.row { display: flex; gap: 8px; margin-top: 12px; flex-wrap: wrap; }
.types { display: flex; gap: 14px; flex-wrap: wrap; }
.chk { display: inline-flex; align-items: center; gap: 6px; font-size: 13px; cursor: pointer; }
.prog { display: flex; align-items: center; gap: 10px; }
.pbar { flex: 1; height: 8px; background: #1b2333; border-radius: 5px; overflow: hidden; }
.pfill { height: 100%; background: linear-gradient(90deg, #4f8cff, #22c55e); transition: width .3s; }
.stats { display: flex; gap: 22px; flex-wrap: wrap; margin-top: 14px; }
.stats div { display: flex; flex-direction: column; gap: 2px; }
.stats b { font-size: 18px; font-weight: 600; }
.stats span { font-size: 11px; color: var(--muted); }
.stats .warn b { color: var(--warn); }
.cur { margin-top: 10px; font-size: 11px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tiny { font-size: 11px; }
</style>
