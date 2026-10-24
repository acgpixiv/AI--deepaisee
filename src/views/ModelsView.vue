<script setup lang="ts">
// 模型管理：列出本地 GGUF、切换嵌入模型、导入 / 删除、切回哈希基线
import { computed, onMounted, ref } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api, type ModelEntry, type ModelInfo } from "../api";
import { refreshStats, store, toast, humanSize } from "../store";

const list = ref<ModelEntry[]>([]);
const info = ref<ModelInfo | null>(null);
const dir = ref("");
const loading = ref(false);
const busy = ref("");
const filter = ref("all");

const FILTERS = [
  { v: "all", t: "全部" },
  { v: "embed", t: "嵌入" },
  { v: "chat", t: "对话" },
  { v: "projector", t: "投影" },
];

async function load() {
  loading.value = true;
  try {
    const [l, m, d] = await Promise.all([api.modelList(), api.modelInfo(), api.modelDir()]);
    list.value = l;
    info.value = m;
    store.model = m;
    dir.value = d;
  } catch (e) {
    toast(`读取模型列表失败：${e}`, "error");
  } finally {
    loading.value = false;
  }
}

const shown = computed(() =>
  filter.value === "all" ? list.value : list.value.filter((m) => m.kind === filter.value)
);

const totalSize = computed(() => list.value.reduce((s, m) => s + m.size, 0));

/** 把某个 GGUF 设为当前嵌入模型 */
async function activate(m: ModelEntry) {
  if (m.kind === "projector") return toast("视觉投影文件不能单独加载", "warn");
  busy.value = m.path;
  try {
    const out = await api.modelLoad("llama", m.path);
    info.value = out;
    store.model = out;
    if (out.ready) toast(`已切换到 ${m.file}（${out.dim} 维 / ${out.device}）`, "ok");
    else toast(out.detail || "模型加载失败", "warn");
    await load();
    await refreshStats();
  } catch (e) {
    toast(`切换失败：${e}`, "error");
  } finally {
    busy.value = "";
  }
}

/** 切回哈希基线：不需要任何模型文件，秒启动 */
async function useHash() {
  busy.value = "hash";
  try {
    const out = await api.modelLoad("hash", undefined);
    info.value = out;
    store.model = out;
    toast("已切回哈希基线（无需模型文件）", "ok");
    await load();
    await refreshStats();
  } catch (e) {
    toast(`切换失败：${e}`, "error");
  } finally {
    busy.value = "";
  }
}

async function importFile() {
  try {
    const picked = await openDialog({
      multiple: false,
      title: "选择要导入的 GGUF 模型",
      filters: [{ name: "GGUF", extensions: ["gguf"] }],
    });
    if (typeof picked !== "string") return;
    busy.value = "import";
    const dest = await api.modelImport(picked);
    toast(`已导入到 ${dest}`, "ok");
    await load();
  } catch (e) {
    toast(`导入失败：${e}`, "error");
  } finally {
    busy.value = "";
  }
}

async function remove(m: ModelEntry) {
  if (m.active) return toast("正在使用的模型不能删除，先切换到其它模型", "warn");
  if (!confirm(`确定删除模型文件？\n\n${m.file}\n${m.size_label}\n\n删除后不可恢复。`)) return;
  busy.value = m.path;
  try {
    await api.modelDelete(m.path);
    toast(`已删除 ${m.file}`, "ok");
    await load();
  } catch (e) {
    toast(`删除失败：${e}`, "error");
  } finally {
    busy.value = "";
  }
}

async function openDir() {
  try {
    await api.openPath(dir.value);
  } catch (e) {
    toast(`打开目录失败：${e}`, "error");
  }
}

/** 跨模态模型目录：优先用状态里回显的真实路径，没有就打开模型根目录 */
async function openCrossDir() {
  const m = info.value;
  const p = m?.clip || m?.clap || "";
  // describe() 返回「中文 CLIP · 512 维 · D:\...」，取最后一个分隔点后的路径
  const idx = p.lastIndexOf(" · ");
  const target = idx >= 0 ? p.slice(idx + 3) : dir.value;
  try {
    await api.openPath(target);
  } catch (e) {
    toast(`打开目录失败：${e}`, "error");
  }
}

const badge = computed(() => {
  const m = info.value;
  if (!m) return { text: "未知", cls: "muted" };
  if (m.ready) return { text: `${m.backend} · ${m.dim} 维 · ${m.device}`, cls: "ok" };
  return { text: m.detail || "未加载", cls: "warn" };
});

onMounted(load);
</script>

<template>
  <div class="view">
    <!-- 当前模型 -->
    <div class="cur card">
      <div class="curmain">
        <div class="row">
          <span class="sec">当前嵌入模型</span>
          <span class="badge" :class="badge.cls">{{ badge.text }}</span>
        </div>
        <div class="curpath mono" :title="info?.model">{{ info?.model || "（未加载）" }}</div>
        <div class="row tags">
          <span v-for="m in info?.modalities || []" :key="m" class="pill">{{ m }}</span>
          <span v-if="!info?.modalities?.length" class="pill muted">文本</span>
        </div>
      </div>
      <div class="curactions">
        <button class="ghost" :disabled="!!busy" @click="useHash">切回哈希基线</button>
        <button class="ghost" @click="openDir">打开模型目录</button>
      </div>
    </div>

    <!-- 跨模态模型：文字找图/找声 -->
    <div class="cur card">
      <div class="curmain">
        <div class="row">
          <span class="sec">跨模态模型</span>
          <span class="badge" :class="info?.clip ? 'ok' : 'muted'">
            {{ info?.clip ? "图片已启用" : "图片未安装" }}
          </span>
          <span class="badge" :class="info?.clap ? 'ok' : 'muted'">
            {{ info?.clap ? "声音已启用" : "声音未安装" }}
          </span>
        </div>
        <div class="curpath mono" :title="info?.clip">{{ info?.clip || "中文 CLIP 未安装：把 model.onnx + vocab.txt 放进 data/models/clip" }}</div>
        <div class="curpath mono" :title="info?.clap">{{ info?.clap || "CLAP 未安装：把 audio_model.onnx / text_model.onnx / vocab.txt 放进 data/models/clap" }}</div>
        <div class="muted tiny">
          两者都用于「用一句话找图片 / 视频 / 音频」。装好后在「搜索」页打开「找图找声」即可；
          已建立索引的文件需要重新建索引才会补上跨模态向量。
        </div>
      </div>
      <div class="curactions">
        <button class="ghost" @click="openCrossDir">打开跨模态目录</button>
      </div>
    </div>

    <!-- 工具条 -->
    <div class="bar">
      <div class="chips">
        <button
          v-for="f in FILTERS"
          :key="f.v"
          class="tab"
          :class="{ active: filter === f.v }"
          @click="filter = f.v"
        >
          {{ f.t }}
          <template v-if="f.v !== 'all'">
            ({{ list.filter((m) => m.kind === f.v).length }})
          </template>
        </button>
      </div>
      <span class="grow"></span>
      <span class="muted tiny">
        {{ list.length }} 个模型 · 共 {{ humanSize(totalSize) }} · {{ dir }}
      </span>
      <button class="ghost" :disabled="loading" @click="load">刷新</button>
      <button class="primary" :disabled="!!busy" @click="importFile">导入模型</button>
    </div>

    <!-- 列表 -->
    <div class="list card">
      <div v-if="loading" class="none">扫描中…</div>
      <div v-else-if="!shown.length" class="empty">
        <div class="big">模型目录里还没有 GGUF 文件</div>
        <div class="muted tiny">把 .gguf 放进 <code class="mono">{{ dir }}</code>，或点右上角「导入模型」</div>
      </div>
      <div v-for="m in shown" :key="m.path" class="item" :class="{ active: m.active }">
        <div class="left">
          <div class="row">
            <span class="nm" :title="m.name">{{ m.name }}</span>
            <span class="kind" :class="m.kind">{{ m.kind_label }}</span>
            <span v-if="m.active" class="inuse">使用中</span>
          </div>
          <div class="file mono" :title="m.path">{{ m.file }} · {{ m.size_label }}</div>
          <div class="meta muted tiny">{{ m.mtime_label }} · {{ m.path }}</div>
        </div>
        <div class="ops">
          <button
            class="primary"
            :disabled="m.active || !!busy || m.kind === 'projector'"
            @click="activate(m)"
          >
            {{ busy === m.path ? "加载中…" : m.active ? "当前使用" : "设为嵌入模型" }}
          </button>
          <button class="ghost danger" :disabled="m.active || !!busy" @click="remove(m)">
            删除
          </button>
        </div>
      </div>
    </div>

    <p class="muted tiny foot">
      嵌入模型用于语义检索；切换模型后已有向量会按新维度重建（可能提示重新索引）。
      对话模型与视觉投影文件只做登记，暂时不参与检索。
    </p>
  </div>
</template>

<style scoped>
.view {
  display: flex;
  flex-direction: column;
  height: 100%;
  gap: 8px;
  min-height: 0;
}
.cur {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 12px 14px;
  flex: none;
}
.curmain { min-width: 0; }
.sec { font-size: 12px; color: var(--muted); }
.curpath {
  font-size: 12px;
  color: #c4d0e3;
  margin-top: 5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 720px;
}
.curactions { display: flex; gap: 8px; flex: none; }
.row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.tags { margin-top: 6px; }
.pill {
  font-size: 11px;
  color: var(--accent);
  background: rgba(79, 140, 255, 0.12);
  border: 1px solid rgba(79, 140, 255, 0.35);
  border-radius: 999px;
  padding: 1px 8px;
}
.pill.muted { color: var(--muted); background: transparent; border-color: var(--border); }
.badge {
  display: inline-block;
  font-size: 11px;
  padding: 3px 9px;
  border-radius: 999px;
  border: 1px solid var(--border);
  color: var(--muted);
}
.badge.ok {
  color: #22c55e;
  border-color: rgba(34, 197, 94, 0.45);
  background: rgba(34, 197, 94, 0.12);
}
.badge.warn {
  color: #f59e0b;
  border-color: rgba(245, 158, 11, 0.45);
  background: rgba(245, 158, 11, 0.12);
}
.bar { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.chips { display: flex; gap: 6px; }
.grow { flex: 1; }
.list {
  flex: 1;
  overflow: auto;
  min-height: 0;
  padding: 6px;
}
.item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 10px 12px;
  border-radius: 9px;
  border: 1px solid transparent;
  border-bottom: 1px solid rgba(36, 48, 74, 0.55);
}
.item:hover { background: rgba(79, 140, 255, 0.06); }
.item.active { background: rgba(34, 197, 94, 0.08); border-color: rgba(34, 197, 94, 0.3); }
.left { min-width: 0; flex: 1; }
.nm { font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 460px; }
.kind {
  font-size: 11px;
  border-radius: 5px;
  padding: 0 6px;
  border: 1px solid var(--border);
  color: var(--muted);
}
.kind.embed { color: #38bdf8; border-color: rgba(56, 189, 248, 0.4); }
.kind.chat { color: #a78bfa; border-color: rgba(167, 139, 250, 0.4); }
.kind.projector { color: #f59e0b; border-color: rgba(245, 158, 11, 0.4); }
.inuse { font-size: 11px; color: #22c55e; }
.file { font-size: 12px; color: #b7c4da; margin-top: 4px; }
.meta { margin-top: 3px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ops { display: flex; gap: 8px; flex: none; }
.danger { color: #f87171; border-color: rgba(239, 68, 68, 0.4); }
.none { color: var(--muted); font-size: 12px; padding: 20px; text-align: center; }
.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  height: 100%;
  color: var(--muted);
}
.empty .big { font-size: 15px; color: #b7c4da; }
.tiny { font-size: 11px; }
.foot { margin: 0; line-height: 1.6; flex: none; }
</style>
