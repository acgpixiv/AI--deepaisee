<script setup lang="ts">
// 搜索视图：关键词/语义检索 + 相似检索 + 结果表 + 预览 + 导出清单
import { computed, onMounted, ref, watch } from "vue";
import { api, type Hit, type SearchQuery } from "../api";
import { refreshStats, store, toast } from "../store";
import ResultTable from "../components/ResultTable.vue";
import PreviewPane from "../components/PreviewPane.vue";

const props = defineProps<{ thumbDir: string }>();

const text = ref("");
const semantic = ref(true);
const fulltext = ref(true);
const pinyin = ref(true);
const visual = ref(true);
const kind = ref("all");
const sort = ref("relevance");
const desc = ref(true);
const hits = ref<Hit[]>([]);
const loading = ref(false);
const selected = ref<Hit | null>(null);
const elapsed = ref(0);

// 相似检索模式
const similarMode = ref(false);
const similarSource = ref<Hit | null>(null);

const KINDS = [
  { v: "all", t: "全部" },
  { v: "image", t: "图片" },
  { v: "video", t: "视频" },
  { v: "audio", t: "音频" },
  { v: "document", t: "文档" },
  { v: "text", t: "文本" },
  { v: "archive", t: "压缩包" },
];

const SORTS = [
  { v: "relevance", t: "相关度" },
  { v: "mtime", t: "修改时间" },
  { v: "size", t: "大小" },
  { v: "name", t: "名称" },
  { v: "kind", t: "类型" },
];

let timer: number | undefined;

function buildQuery(): SearchQuery {
  return {
    text: text.value,
    kinds: kind.value === "all" ? [] : [kind.value],
    exts: [],
    roots: [],
    categories: [],
    limit: 300,
    sort: sort.value,
    desc: desc.value,
    use_fts: fulltext.value,
    use_pinyin: pinyin.value,
    use_fuzzy: true,
    use_semantic: semantic.value,
    cross_modal: visual.value,
  };
}

async function run() {
  if (similarMode.value) return runSimilar();
  loading.value = true;
  const t0 = performance.now();
  try {
    const q = buildQuery();
    const res =
      semantic.value && text.value.trim()
        ? await api.searchSemantic(q)
        : await api.search(q);
    hits.value = res;
    elapsed.value = performance.now() - t0;
    if (selected.value) {
      const again = res.find((h) => h.id === selected.value!.id);
      selected.value = again || null;
    }
  } catch (e) {
    toast(`搜索失败：${e}`, "error");
    hits.value = [];
  } finally {
    loading.value = false;
  }
}

async function runSimilar() {
  if (!similarSource.value) return;
  loading.value = true;
  const t0 = performance.now();
  try {
    const res = await api.similar(
      similarSource.value.id,
      kind.value === "all" ? undefined : kind.value,
      100
    );
    hits.value = res;
    elapsed.value = performance.now() - t0;
  } catch (e) {
    toast(`相似检索失败：${e}`, "error");
  } finally {
    loading.value = false;
  }
}

function schedule() {
  window.clearTimeout(timer);
  timer = window.setTimeout(run, 260);
}

watch([text, semantic, fulltext, pinyin, kind, sort, desc], schedule);
watch(
  () => similarMode.value,
  (v) => {
    if (v) runSimilar();
    else run();
  }
);

function enterSimilar(h: Hit) {
  similarSource.value = h;
  similarMode.value = true;
  selected.value = h;
}

function exitSimilar() {
  similarMode.value = false;
  similarSource.value = null;
  run();
}

async function open(h: Hit) {
  try {
    await api.openPath(h.path);
  } catch (e) {
    toast(`打开失败：${e}`, "error");
  }
}

async function reveal(h: Hit) {
  try {
    await api.revealPath(h.path);
  } catch (e) {
    toast(`定位失败：${e}`, "error");
  }
}

// ------------------------------------------------------------ 导出清单
const selectedIds = ref<Set<number>>(new Set());

function toggleAll() {
  if (selectedIds.value.size === hits.value.length) selectedIds.value = new Set();
  else selectedIds.value = new Set(hits.value.map((h) => h.id));
}

const checkedPaths = computed(() =>
  hits.value.filter((h) => selectedIds.value.has(h.id)).map((h) => h.path)
);

async function doExport() {
  const paths = checkedPaths.value;
  if (!paths.length) return toast("请先勾选文件", "warn");
  try {
    const dir = await api.dataDir();
    const out = `${dir}\\export-${Date.now()}.csv`;
    const n = await api.opExport(paths, out, "csv");
    toast(`已导出 ${n} 条到 ${out}`, "ok");
  } catch (e) {
    toast(`导出失败：${e}`, "error");
  }
}

onMounted(() => {
  api
    .recent(80)
    .then((r) => {
      hits.value = r;
    })
    .catch(() => {});
});

defineExpose({ run, enterSimilar });
</script>

<template>
  <div class="view">
    <div class="bar">
      <div class="searchbox">
        <span class="ico">⌕</span>
        <input
          v-model="text"
          :placeholder="
            similarMode
              ? '相似检索中：结果按向量相似度排序'
              : '输入文件名、拼音（huiyi）、关键词，或一句话描述…'
          "
          :disabled="similarMode"
          @keyup.enter="run"
        />
        <button v-if="text" class="clear" @click="text = ''">×</button>
      </div>
      <button
        class="tab"
        :class="{ active: fulltext }"
        @click="fulltext = !fulltext"
        :disabled="similarMode"
        title="在 Word / Excel / PDF / TXT 等文档正文里检索"
      >
        全文
      </button>
      <button
        class="tab"
        :class="{ active: pinyin }"
        @click="pinyin = !pinyin"
        :disabled="similarMode"
        title="输入 huiyijilu / hyjl 也能找到「会议记录」"
      >
        拼音
      </button>
      <button
        class="tab"
        :class="{ active: semantic }"
        @click="semantic = !semantic"
        :disabled="similarMode"
        title="用一句话描述你想要的画面/内容"
      >
        语义
      </button>
      <button
        class="tab"
        :class="{ active: visual }"
        @click="visual = !visual"
        :disabled="similarMode"
        title="用一句话描述画面或声音：找图片 / 视频 / 音频（需要装跨模态模型）"
      >
        找图找声
      </button>
      <select v-model="sort" class="sel" :disabled="similarMode">
        <option v-for="s in SORTS" :key="s.v" :value="s.v">{{ s.t }}</option>
      </select>
      <button class="iconbtn" @click="desc = !desc" :title="desc ? '降序' : '升序'">
        {{ desc ? "↓" : "↑" }}
      </button>
    </div>

    <div class="chips">
      <button
        v-for="k in KINDS"
        :key="k.v"
        class="tab"
        :class="{ active: kind === k.v }"
        @click="kind = k.v"
      >
        {{ k.t }}
      </button>
      <span class="grow"></span>
      <button v-if="similarMode" class="ghost" @click="exitSimilar">
        ← 退出相似检索
      </button>
      <button class="ghost" @click="toggleAll">
        {{ selectedIds.size === hits.length && hits.length ? "取消全选" : "全选" }}
      </button>
    </div>

    <div class="main">
      <div class="left card">
        <div class="meta">
          <span v-if="similarMode" class="simtag">
            相似于：{{ similarSource?.name }}
          </span>
          <span class="muted">
            {{ hits.length }} 个结果 · {{ elapsed.toFixed(0) }} ms
            <template v-if="selectedIds.size"> · 已选 {{ selectedIds.size }}</template>
          </span>
        </div>
        <ResultTable
          :hits="hits"
          :selected-id="selected?.id ?? null"
          :loading="loading"
          :query="text"
          @select="selected = $event"
          @open="open"
          @reveal="reveal"
        />
      </div>
      <PreviewPane
        :hit="selected"
        :thumb-dir="thumbDir"
        @open="open"
        @reveal="reveal"
        @similar="enterSimilar"
      />
    </div>

    <div class="foot">
      <button class="ghost" @click="doExport">导出清单</button>
      <span class="grow"></span>
      <span class="muted tiny">
        双击打开 · 选中后点「找相似」按内容检索
      </span>
    </div>
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
.bar { display: flex; gap: 8px; align-items: center; }
.searchbox {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 0 12px;
  height: 42px;
  transition: border-color .15s;
}
.searchbox:focus-within { border-color: var(--accent); }
.ico { color: var(--muted); font-size: 17px; }
.searchbox input {
  flex: 1;
  background: transparent;
  border: none;
  padding: 0;
  height: 100%;
  font-size: 14px;
}
.searchbox input:focus { border: none; }
.clear {
  background: transparent;
  border: none;
  color: var(--muted);
  font-size: 17px;
  padding: 0 4px;
}
.sel { height: 38px; padding: 0 10px; }
.iconbtn { width: 38px; height: 38px; padding: 0; }
.chips { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
.grow { flex: 1; }
.main {
  flex: 1;
  display: grid;
  grid-template-columns: minmax(0, 1fr) 300px;
  gap: 8px;
  min-height: 0;
}
.left {
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow: hidden;
}
.meta {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 12px;
  border-bottom: 1px solid var(--border);
  font-size: 12px;
}
.simtag {
  color: var(--accent);
  border: 1px solid rgba(79, 140, 255, .45);
  background: rgba(79, 140, 255, .12);
  border-radius: 6px;
  padding: 1px 7px;
}
.foot {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 2px;
}
.tiny { font-size: 11px; }
</style>
