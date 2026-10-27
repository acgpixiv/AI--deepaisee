<script setup lang="ts">
// 模型管理：列出本地 GGUF、切换嵌入模型、导入 / 删除、切回哈希基线
import { computed, onMounted, ref } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AudioLines, FileText, Film, FolderOpen, Image as ImageIcon, Power, Save, SlidersHorizontal } from "lucide-vue-next";
import { api, type AppConfig, type GpuDevice, type ModelEntry, type ModelInfo, type ModelImportResult } from "../api";
import { refreshStats, store, toast, humanSize } from "../store";
import { t } from "../i18n";

const props = defineProps<{
  cfg?: AppConfig;
  saveModelConfig?: () => Promise<boolean | void>;
}>();

const list = ref<ModelEntry[]>([]);
const info = ref<ModelInfo | null>(null);
const dir = ref("");
const importedDir = ref("");
const loading = ref(false);
const busy = ref("");
const filter = ref("all");
const selected = ref<ModelEntry | null>(null);
const gpuDevices = ref<GpuDevice[]>([]);
const gpuLoadError = ref("");

const PARALLEL_MODES = [
  { key: "eco", label: "节能", hint: "降低后台资源占用", className: "eco" },
  { key: "auto", label: "自动", hint: "按设备状态动态调节", className: "auto" },
  { key: "turbo", label: "狂暴", hint: "尽可能提高后台吞吐", className: "turbo" },
];

const FILTERS = [
  { v: "all", label: "全部" },
  { v: "embed", label: "嵌入" },
  { v: "chat", label: "对话" },
  { v: "projector", label: "投影" },
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
    toast(`${t("读取模型列表失败：")}${e}`, "error");
  } finally {
    loading.value = false;
  }
}

async function loadGpus() {
  try {
    gpuDevices.value = (await api.gpuDevices()).sort((a, b) => b.total_memory - a.total_memory);
  } catch (error) {
    gpuLoadError.value = String(error);
  }
}

const shown = computed(() =>
  filter.value === "all" ? list.value : list.value.filter((m) => m.kind === filter.value)
);

const totalSize = computed(() => list.value.reduce((s, m) => s + m.size, 0));

// The backend is selected from the model contents by Rust. The UI only
// exposes whether a complete, supported model package is available.
function canActivate(model: ModelEntry): boolean {
  return model.kind !== "projector" && model.direct_loadable;
}

const gpuOptions = computed(() => {
  const seen = new Set<string>();
  return gpuDevices.value.filter((device) => {
    const key = `${device.name.trim().toLocaleLowerCase()}|${device.total_memory}|${device.integrated}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
});

const activeModelFormat = computed(() => list.value.find((model) => model.active)?.format || "");
const integratedGpuNote = computed(() => {
  const hasVulkanIntegrated = gpuOptions.value.some(
    (device) => device.integrated && device.backend.toLowerCase() === "vulkan",
  );
  if (!hasVulkanIntegrated) return "";
  if (activeModelFormat.value === "safetensors") {
    return t("当前 safetensors 模型仅使用 NVIDIA GPU；切换到配套 GGUF 后可尝试核显与独显协同。");
  }
  if (activeModelFormat.value === "gguf") {
    return t("当前 GGUF 模型可尝试由核显与独显共同处理索引批次；核显会共享系统内存。");
  }
  return t("核显与独显协同取决于当前模型格式和设备可用资源。");
});

function activationLabel(model: ModelEntry): string {
  if (busy.value === model.path) return t("加载中…");
  if (model.active) return t("当前使用");
  if (model.kind === "projector") return t("配套文件");
  return canActivate(model) ? t("设为当前模型") : t("缺少完整文件");
}

/** 把某个 GGUF 设为当前嵌入模型 */
async function activate(m: ModelEntry) {
  if (m.kind === "projector") return toast(t("这是模型配套投影文件，不能单独作为嵌入模型加载"), "warn");
  if (!canActivate(m)) return toast(t("此模型缺少可识别的完整配套文件，暂时不能加载"), "warn");
  busy.value = m.path;
  try {
    const out = await api.modelLoad("auto", m.path);
    info.value = out;
    store.model = out;
    if (out.ready) toast(`${t("已切换到")} ${m.file}（${out.dim} ${t("维")}）`, "ok");
    else toast(out.detail || t("模型加载失败"), "warn");
    await load();
    await refreshStats();
  } catch (e) {
    toast(`${t("切换失败：")}${e}`, "error");
  } finally {
    busy.value = "";
  }
}

/** 切回哈希基线：不需要任何模型文件，秒启动 */
async function useHash() {
  busy.value = "hash";
  try {
    const out = await api.modelDeactivate();
    info.value = out;
    store.model = out;
    toast(t("已停用当前模型，改用基础检索"), "ok");
    await load();
    await refreshStats();
  } catch (e) {
    toast(`${t("切换失败：")}${e}`, "error");
  } finally {
    busy.value = "";
  }
}

async function importFile() {
  try {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: t("选择模型文件夹（自动识别 GGUF / safetensors）"),
    });
    if (typeof picked !== "string") return;
    busy.value = "import";
    const result: ModelImportResult = await api.modelImportFolder(picked);
    importedDir.value = result.destination;
    const loadState = result.direct_loadable || result.candidate_backends?.length
      ? t("已识别，可尝试加载")
      : t("已识别，但缺少可用的完整模型文件");
    const components = result.components.length ? `（${result.components.join(" + ")}）` : "";
    const architecture = result.architecture ? `${t("架构：")}${result.architecture}${components}` : `${t("分类：")}${result.primary_kind}`;
    toast(`${t("已导入")} ${result.copied_files} ${t("个配套文件")}（${humanSize(result.total_bytes)}），${architecture}；${loadState}`, result.direct_loadable || result.candidate_backends?.length ? "ok" : "warn");
    for (const warning of result.warnings) toast(warning, "warn");
    await load();
  } catch (e) {
    toast(`${t("导入失败：")}${e}`, "error");
  } finally {
    busy.value = "";
  }
}

async function remove(m: ModelEntry) {
  if (m.active) return toast(t("正在使用的模型不能删除，先切换到其它模型"), "warn");
  if (!confirm(`${t("确定删除模型文件？")}\n\n${m.file}\n${m.size_label}\n\n${t("删除后不可恢复。")}`)) return;
  busy.value = m.path;
  try {
    await api.modelDelete(m.path);
    toast(`${t("已删除")} ${m.file}`, "ok");
    await load();
  } catch (e) {
    toast(`${t("删除失败：")}${e}`, "error");
  } finally {
    busy.value = "";
  }
}

async function openModelDir(m: ModelEntry) {
  try {
    await api.openPath(m.model_dir);
  } catch (e) {
    toast(`${t("打开模型目录失败：")}${e}`, "error");
  }
}

async function openImportedDir() {
  try {
    await api.openPath(importedDir.value);
  } catch (e) {
    toast(`${t("打开导入目录失败：")}${e}`, "error");
  }
}

function selectModel(model: ModelEntry) {
  selected.value = selected.value?.path === model.path ? null : model;
}

async function saveParams() {
  if (!props.cfg || !props.saveModelConfig) {
    toast(t("当前页面没有可保存的配置上下文"), "warn");
    return;
  }
  const ok = await props.saveModelConfig();
  if (ok) toast(t("推理参数已保存；重新加载模型后生效"), "ok");
}

function formatMemory(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return t("显存未知");
  const gib = bytes / 1024 ** 3;
  return `${gib.toFixed(gib >= 10 ? 0 : 1)} GiB`;
}

async function setParallelMode(mode: string) {
  if (!props.cfg || !props.saveModelConfig) return;
  props.cfg.model.parallel_mode = mode;
  const saved = await props.saveModelConfig();
  if (!saved) return;
  const modelPath = props.cfg.model.embed_model.trim();
  if (!modelPath || !info.value?.ready) {
    toast(t("并行策略已保存；加载嵌入模型后生效"), "ok");
    return;
  }
  busy.value = "parallel-mode";
  try {
    const out = await api.modelLoad("auto", modelPath);
    info.value = out;
    store.model = out;
    await load();
    toast(out.ready ? `${t("并行策略已应用：")}${out.detail}` : out.detail, out.ready ? "ok" : "warn");
  } catch (error) {
    toast(`${t("策略已保存，但模型重载失败：")}${error}`, "error");
  } finally {
    busy.value = "";
  }
}

const modalityCards = computed(() => {
  const selectedModalities = selected.value?.modalities || [];
  const declared = (key: string) => selectedModalities.length > 0
    ? selectedModalities.includes(key)
    : info.value?.modalities?.includes(key) || false;
  const selectedReady = selected.value?.direct_loadable && selected.value?.active;
  const activeSupports = (key: string) => !!info.value?.ready && info.value.modalities.includes(key);
  return [
    {
      key: "text",
      label: "文本",
      icon: FileText,
      ok: activeSupports("text") || selectedReady,
      status: activeSupports("text") || selectedReady ? "可用" : (selected.value?.format === "safetensors" && declared("text") ? "声明" : "未配置"),
      detail: declared("text") && !selectedReady && selected.value?.format === "safetensors"
        ? "模型声明 · 需配套组件"
        : info.value?.ready ? `${info.value.dim} ${t("维")}` : "未加载",
    },
    {
      key: "image",
      label: "图片",
      icon: ImageIcon,
      ok: activeSupports("image") || (selected.value?.format === "safetensors" && declared("image") && !!selectedReady),
      status: activeSupports("image") || (selected.value?.format === "safetensors" && declared("image") && !!selectedReady) ? "可用" : (selected.value?.format === "safetensors" && declared("image") ? "声明" : "未配置"),
      detail: declared("image") && !selectedReady && selected.value?.format === "safetensors" ? "模型声明 · 需加载" : activeSupports("image") ? "共享空间" : "未配置",
    },
    {
      key: "audio",
      label: "音频",
      icon: AudioLines,
      ok: activeSupports("audio") || (selected.value?.format === "safetensors" && declared("audio") && !!selectedReady),
      status: activeSupports("audio") || (selected.value?.format === "safetensors" && declared("audio") && !!selectedReady) ? "可用" : (selected.value?.format === "safetensors" && declared("audio") ? "声明" : "未配置"),
      detail: declared("audio") && !selectedReady && selected.value?.format === "safetensors" ? "模型声明 · 需加载" : activeSupports("audio") ? "共享空间" : "未配置",
    },
    {
      key: "video",
      label: "视频",
      icon: Film,
      ok: activeSupports("video") || (selected.value?.format === "safetensors" && declared("video") && !!selectedReady),
      status: activeSupports("video") || (selected.value?.format === "safetensors" && declared("video") && !!selectedReady) ? "可用" : (selected.value?.format === "safetensors" && declared("video") ? "声明" : "未配置"),
      detail: declared("video") && !selectedReady && selected.value?.format === "safetensors" ? "模型声明 · 需加载" : activeSupports("video") ? "共享空间" : "未配置",
    },
  ];
});

onMounted(() => { void Promise.all([load(), loadGpus()]); });
</script>

<template>
  <div class="view">
    <!-- 第一行：GPU 选择 -->
    <div class="gpu-row card">
      <div>
        <div class="sec">{{ t("GPU 推理设备") }}</div>
        <div class="muted tiny">{{ t("所选设备作为主推理设备；并行模式会按后端兼容性使用其他可用设备。没有可用加速后端时自动退回 CPU。") }}</div>
        <div v-if="integratedGpuNote" class="muted tiny gpu-note">{{ integratedGpuNote }}</div>
      </div>
      <div class="gpu-controls">
        <select v-if="props.cfg" v-model="props.cfg.model.gpu_device" :aria-label="t('GPU 推理设备')">
          <option value="auto">{{ t("自动选择（按显存）") }}</option>
          <option value="cpu">{{ t("CPU（无可用加速设备时）") }}</option>
          <option v-for="device in gpuOptions" :key="device.id" :value="device.id">
            {{ device.name }} · {{ formatMemory(device.total_memory) }}{{ device.integrated ? t(" · 核显") : "" }}
          </option>
        </select>
        <span v-if="gpuLoadError" class="tiny warn-text">{{ t("GPU 列表加载失败：") }}{{ gpuLoadError }}</span>
        <span v-else-if="!gpuOptions.length" class="tiny muted">{{ t("未检测到可用加速设备，将使用 CPU 推理") }}</span>
        <button v-if="props.saveModelConfig" class="ghost icon-action" :title="t('保存 GPU 设置')" @click="saveParams"><Save :size="15" /></button>
      </div>
    </div>

    <div class="parallel-row card">
      <div>
        <div class="sec">{{ t("模型并行策略") }}</div>
        <div class="muted tiny">{{ t("按当前设备状态动态调节后台工作负载；模型实例会按后端能力安全复用") }}</div>
      </div>
      <div class="parallel-buttons" role="group" :aria-label="t('模型并行策略')">
        <button
          v-for="mode in PARALLEL_MODES"
          :key="mode.key"
          class="parallel-button"
          :class="[mode.className, { active: (props.cfg?.model?.parallel_mode || 'auto') === mode.key }]"
          :title="`${t(mode.label)}：${t(mode.hint)}`"
          @click="setParallelMode(mode.key)"
        >{{ t(mode.label) }}</button>
      </div>
    </div>

    <!-- 第二行：四种模态状态 -->
    <div class="modality-grid">
      <div v-for="card in modalityCards" :key="card.key" class="modality-card card" :class="{ supported: card.ok }">
        <component :is="card.icon" :size="18" aria-hidden="true" />
        <div class="modality-copy"><strong>{{ t(card.label) }}</strong><span>{{ t(card.detail) }}</span></div>
        <span class="modality-state">{{ t(card.status) }}</span>
      </div>
    </div>

    <!-- 第三行：模型列表工具条 -->
    <div class="bar">
      <div class="chips">
        <button
          v-for="f in FILTERS"
          :key="f.v"
          class="tab"
          :class="{ active: filter === f.v }"
          @click="filter = f.v"
        >
          {{ t(f.label) }}
          <template v-if="f.v !== 'all'">
            ({{ list.filter((m) => m.kind === f.v).length }})
          </template>
        </button>
      </div>
      <span class="grow"></span>
      <span class="muted tiny">
        {{ list.length }} {{ t("个模型") }} · {{ t("共") }} {{ humanSize(totalSize) }} · {{ dir }}
      </span>
      <button v-if="importedDir" class="ghost" @click="openImportedDir">{{ t("打开导入目录") }}</button>
      <button v-if="info?.ready" class="ghost danger" :disabled="!!busy" :title="t('停用当前模型')" @click="useHash">
        <Power :size="14" />{{ t("停用模型") }}
      </button>
      <button class="ghost" :disabled="loading" @click="load">{{ t("刷新") }}</button>
      <button class="primary" :disabled="!!busy" @click="importFile">{{ t("导入模型") }}</button>
    </div>

    <div class="list card">
      <div v-if="loading" class="none">{{ t("扫描中…") }}</div>
      <div v-else-if="!shown.length" class="empty">
        <div class="big">{{ t("模型目录里还没有 GGUF 或 safetensors 权重") }}</div>
        <div class="muted tiny">{{ t("选择模型文件夹后，程序会自动识别权重及 tokenizer、配置等配套文件") }}</div>
      </div>
      <div v-for="m in shown" :key="m.path" class="item" :class="{ active: m.active, selected: selected?.path === m.path }" @click="selectModel(m)">
        <div class="left">
          <div class="row">
            <span class="nm" :title="m.name">{{ m.name }}</span>
            <span class="kind" :class="m.kind">{{ m.kind_label }}</span>
            <span class="kind format">{{ m.format.toUpperCase() }}</span>
            <span v-if="m.active" class="inuse">{{ t("使用中") }}</span>
          </div>
          <div class="file mono" :title="m.path">{{ m.file }} · {{ m.size_label }}</div>
          <div class="meta muted tiny">{{ m.mtime_label }} · {{ m.path }}</div>
        </div>
        <div class="ops">
          <button
            @click.stop
            class="primary"
            :disabled="m.active || !!busy || !canActivate(m)"
            @click="activate(m)"
          >
            {{ activationLabel(m) }}
          </button>
          <button class="ghost" :title="`${t('打开模型文件夹：')}${m.model_dir}`" @click.stop="openModelDir(m)">
            <FolderOpen :size="14" />
          </button>
          <button class="ghost danger" :disabled="m.active || !!busy" @click.stop="remove(m)">
            {{ t("删除") }}
          </button>
        </div>
      </div>
    </div>

    <!-- 点击模型后展开推理参数 -->
    <section v-if="selected && props.cfg" class="params card">
      <div class="params-head">
        <div><strong>{{ selected.name }}</strong><span class="kind format">{{ selected.format.toUpperCase() }}</span></div>
        <SlidersHorizontal :size="16" aria-hidden="true" />
      </div>
      <div class="params-grid">
        <label>{{ t("上下文长度") }}<input type="number" min="512" step="256" v-model.number="props.cfg.model.context_size" /></label>
        <label class="param-path">{{ t("模型路径") }}<input :value="selected.path" readonly /></label>
      </div>
      <p v-if="selected.format === 'safetensors'" class="params-warning">
        {{ t("safetensors 是权重容器；请确保配置文件、分片权重和 tokenizer 等配套文件位于同一目录。") }}
      </p>
      <div class="params-actions"><span class="muted tiny">{{ t("0 表示由运行时自动选择线程数。") }}</span><button class="primary" @click="saveParams"><Save :size="14" />{{ t("保存推理参数") }}</button></div>
    </section>

    <p class="muted tiny foot">
      {{ t("嵌入模型用于语义检索；切换模型后已有向量会按新维度重建（可能提示重新索引）。") }}
      {{ t("程序会自动选择匹配的加载方式；当前可加载状态以列表中的按钮为准。") }}
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
.gpu-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 12px 14px;
  flex: none;
}
.gpu-controls { display: flex; align-items: center; gap: 8px; min-width: min(100%, 420px); justify-content: flex-end; }
.gpu-controls select { min-width: min(100%, 360px); }
.gpu-note { margin-top: 3px; max-width: 740px; line-height: 1.45; }
.icon-action { display: inline-grid; place-items: center; width: 32px; height: 32px; padding: 0; }
.parallel-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 10px 14px; flex: none; }
.parallel-buttons { display: inline-flex; gap: 0; }
.parallel-button { min-width: 82px; padding: 8px 14px; border: 1px solid #111; border-right: 0; border-radius: 0; color: #111; background: #fff; cursor: pointer; }
.parallel-button:last-child { border-right: 1px solid #111; }
.parallel-button.eco.active { color: #fff; background: #238636; }
.parallel-button.auto.active { color: #fff; background: #1d6fd1; }
.parallel-button.turbo.active { color: #fff; background: #d9572b; }
.warn-text { color: var(--warn); }
.modality-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; flex: none; }
.modality-card { display: flex; align-items: center; gap: 9px; min-width: 0; padding: 11px 12px; color: #555; }
.modality-card.supported { color: #111; }
.modality-copy { display: flex; flex-direction: column; min-width: 0; gap: 2px; }
.modality-copy strong { font-size: 12px; font-weight: 600; }
.modality-copy span { font-size: 10px; color: #777; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.modality-state { margin-left: auto; font-size: 10px; white-space: nowrap; color: #888; }
.modality-card.supported .modality-state { color: #111; }
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
.item.selected { outline: 1px solid #111; outline-offset: -1px; background: #fafafa; }
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
.backend-list { margin-top: 4px; color: #555; }
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
.params { flex: none; padding: 13px 14px; }
.params-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 11px; }
.params-head > div { display: flex; align-items: center; gap: 8px; min-width: 0; }
.params-head strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.params-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; }
.params-grid label { display: flex; flex-direction: column; gap: 5px; color: #555; font-size: 11px; }
.params-grid input { width: 100%; }
.params-grid .param-path { grid-column: span 1; }
.params-actions { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-top: 11px; }
.params-actions button { display: inline-flex; align-items: center; gap: 6px; }
.params-warning { margin: 10px 0 0; padding: 8px 10px; border: 1px solid #8c6410; color: #6f4e00; font-size: 11px; line-height: 1.5; background: #fffaf0; }
.tiny { font-size: 11px; }
.foot { margin: 0; line-height: 1.6; flex: none; }
/* Flat white/black model management surface. */
.view { color: #111; }
.cur, .list { border: 1px solid #111; border-radius: 2px; background: #fff; }
.curpath, .file { color: #222; }
.pill { color: #111; background: #f2f2f2; border-color: #111; border-radius: 2px; }
.pill.muted { color: #555; background: #fff; border-color: #aaa; }
.badge { border-radius: 2px; color: #555; }
.badge.ok { color: #111; border-color: #111; background: #f2f2f2; }
.badge.warn { color: #8c6410; border-color: #8c6410; background: #fffaf0; }
.list { padding: 4px; }
.item { border-radius: 2px; border-color: transparent; border-bottom-color: #ddd; }
.item:hover { background: #f0f0f0; }
.item.active { background: #f7f7f7; border-color: #111; }
.kind { border-radius: 2px; color: #333; }
.kind.embed, .kind.chat, .kind.projector { color: #111; border-color: #777; }
.inuse { color: #111; }
.danger { color: #9b2923; border-color: #9b2923; }
.empty .big { color: #111; }
@media (max-width: 700px) {
  .gpu-row { align-items: flex-start; flex-direction: column; }
  .gpu-controls { width: 100%; justify-content: flex-start; }
  .gpu-controls select { flex: 1; min-width: 0; }
  .modality-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .params-grid { grid-template-columns: 1fr; }
  .params-grid .param-path { grid-column: auto; }
  .params-actions { align-items: flex-start; flex-direction: column; }
  .parallel-row { align-items: flex-start; flex-direction: column; }
  .parallel-buttons { width: 100%; }
  .parallel-button { flex: 1; }
}
</style>
