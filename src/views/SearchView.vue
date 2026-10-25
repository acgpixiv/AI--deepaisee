<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { ArrowDownUp, Check, ChevronRight, Download, Search, X } from "lucide-vue-next";
import { api, type Hit, type SearchQuery } from "../api";
import { toast } from "../store";
import ResultGallery from "../components/ResultGallery.vue";

const props = defineProps<{ thumbDir: string }>();

const text = ref("");
const semantic = ref(true);
const fulltext = ref(true);
const pinyin = ref(true);
const fuzzy = ref(true);
const visual = ref(true);
const debounceMs = ref(260);
const pageSize = ref(1000);
const kind = ref("all");
const sort = ref("relevance");
const desc = ref(true);
const hits = ref<Hit[]>([]);
const recentHits = ref<Hit[]>([]);
const loading = ref(false);
const searched = ref(false);
const selected = ref<Hit | null>(null);
const elapsed = ref(0);
const similarMode = ref(false);
const similarSource = ref<Hit | null>(null);
const selectedIds = ref<Set<number>>(new Set());
const typedPrompt = ref("");
const recentOpen = ref(false);

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

let searchTimer: number | undefined;
let typeTimer: number | undefined;
let promptIndex = 0;
let deleting = false;
const prompts = ["今天想要搜索什么？", "找一份文件，或描述一张图片", "用一句话找到硬盘里的内容"];

function typePrompt() {
  const target = prompts[promptIndex];
  if (!deleting) {
    typedPrompt.value = target.slice(0, typedPrompt.value.length + 1);
    if (typedPrompt.value === target) {
      deleting = true;
      typeTimer = window.setTimeout(typePrompt, 1900);
      return;
    }
  } else {
    typedPrompt.value = target.slice(0, Math.max(0, typedPrompt.value.length - 1));
    if (!typedPrompt.value) {
      deleting = false;
      promptIndex = (promptIndex + 1) % prompts.length;
    }
  }
  typeTimer = window.setTimeout(typePrompt, deleting ? 45 : 85);
}

function buildQuery(): SearchQuery {
  return {
    text: text.value,
    kinds: kind.value === "all" ? [] : [kind.value],
    exts: [],
    roots: [],
    categories: [],
    limit: pageSize.value,
    sort: sort.value,
    desc: desc.value,
    use_fts: fulltext.value,
    use_pinyin: pinyin.value,
    use_fuzzy: fuzzy.value,
    use_semantic: semantic.value,
    cross_modal: visual.value,
  };
}

async function run() {
  if (similarMode.value) return runSimilar();
  searched.value = true;
  loading.value = true;
  const started = performance.now();
  try {
    let res: Hit[];
    if (!text.value.trim() && kind.value === "all" && sort.value === "relevance") {
      res = recentHits.value.length ? recentHits.value : await api.recent(200);
    } else {
      const q = buildQuery();
      res = semantic.value && text.value.trim() ? await api.searchSemantic(q) : await api.search(q);
    }
    hits.value = res;
    elapsed.value = performance.now() - started;
    if (selected.value) selected.value = res.find((hit) => hit.id === selected.value?.id) || null;
  } catch (e) {
    hits.value = [];
    toast(`搜索失败：${e}`, "error");
  } finally {
    loading.value = false;
  }
}

async function runSimilar() {
  if (!similarSource.value) return;
  searched.value = true;
  loading.value = true;
  const started = performance.now();
  try {
    hits.value = await api.similar(similarSource.value.id, kind.value === "all" ? undefined : kind.value, 1000);
    elapsed.value = performance.now() - started;
  } catch (e) {
    toast(`相似检索失败：${e}`, "error");
  } finally {
    loading.value = false;
  }
}

function schedule() {
  window.clearTimeout(searchTimer);
  if (!text.value.trim() && !similarMode.value) {
    searched.value = false;
    hits.value = [];
    selected.value = null;
    selectedIds.value = new Set();
    return;
  }
  searched.value = true;
  recentOpen.value = false;
  searchTimer = window.setTimeout(run, debounceMs.value);
}

function enterSimilar(hit: Hit) {
  similarSource.value = hit;
  similarMode.value = true;
  selected.value = hit;
}

function exitSimilar() {
  similarMode.value = false;
  similarSource.value = null;
}

function openRecent(hit: Hit) {
  recentOpen.value = false;
  void open(hit);
}

async function open(hit: Hit) {
  try { await api.openPath(hit.path); } catch (e) { toast(`打开失败：${e}`, "error"); }
}

async function reveal(hit: Hit) {
  try { await api.revealPath(hit.path); } catch (e) { toast(`定位失败：${e}`, "error"); }
}

async function rename(hit: Hit) {
  const current = hit.name;
  const name = window.prompt("输入新的文件名", current);
  if (!name || name === current) return;
  try {
    await api.renamePath(hit.path, name);
    toast("文件已重命名，请重新扫描目录以更新索引", "ok");
    await run();
  } catch (e) {
    toast(`重命名失败：${e}`, "error");
  }
}

function toggleSelected(id: number) {
  const next = new Set(selectedIds.value);
  if (next.has(id)) next.delete(id); else next.add(id);
  selectedIds.value = next;
}

function toggleAll() {
  selectedIds.value = selectedIds.value.size === hits.value.length
    ? new Set()
    : new Set(hits.value.map((hit) => hit.id));
}

async function doExport() {
  const paths = hits.value.filter((hit) => selectedIds.value.has(hit.id)).map((hit) => hit.path);
  if (!paths.length) return toast("请先勾选文件", "warn");
  try {
    const out = `${await api.dataDir()}\\export-${Date.now()}.csv`;
    const count = await api.opExport(paths, out, "csv");
    toast(`已导出 ${count} 条到 ${out}`, "ok");
  } catch (e) {
    toast(`导出失败：${e}`, "error");
  }
}

watch([text, semantic, fulltext, pinyin, fuzzy, visual, kind, sort, desc], schedule);
watch(similarMode, (value) => { if (value) runSimilar(); else if (searched.value) run(); });

onMounted(async () => {
  typePrompt();
  try {
    const [config, recent] = await Promise.all([api.getConfig(), api.recent(80)]);
    semantic.value = config.search.use_semantic !== false;
    fulltext.value = config.search.use_fulltext !== false;
    pinyin.value = config.search.use_pinyin !== false;
    fuzzy.value = config.search.use_fuzzy !== false;
    visual.value = config.model.clip_enabled !== false || config.model.clap_enabled !== false;
    // Migrate the old 200-result default so an existing config cannot keep
    // the previous practical ceiling. The backend still caps a single page
    // at 10,000 and supports offset pagination for larger collections.
    pageSize.value = Math.min(10000, Math.max(1000, Number(config.search.page_size ?? 1000)));
    debounceMs.value = Math.min(2000, Math.max(0, Number(config.search.debounce_ms ?? 200)));
    recentHits.value = recent;
  } catch {
    try { recentHits.value = await api.recent(80); } catch {}
  }
});

onBeforeUnmount(() => {
  window.clearTimeout(searchTimer);
  window.clearTimeout(typeTimer);
});

const resultLabel = computed(() => {
  if (similarMode.value) return `相似于 ${similarSource.value?.name || "当前文件"}`;
  return `${hits.value.length.toLocaleString()} 个结果`;
});

defineExpose({ run, enterSimilar });
</script>

<template>
  <div class="search-page" :class="{ searched }" @keydown.esc="recentOpen = false">
    <div v-if="!searched" class="triangle-field" aria-hidden="true">
      <span class="triangle triangle-a"></span>
      <span class="triangle triangle-b"></span>
      <span class="triangle triangle-c"></span>
      <span class="triangle triangle-d"></span>
      <span class="triangle triangle-e"></span>
    </div>
    <section class="hero-search">
      <div v-if="!searched" class="hero-copy">
        <div class="brand-name">deepaisee</div>
        <h1>{{ typedPrompt }}<span class="caret" aria-hidden="true"></span></h1>
      </div>
      <div class="query-wrap">
        <div class="query-shell" :class="{ focused: searched }">
          <Search :size="20" class="query-icon" aria-hidden="true" />
          <input
            v-model="text"
            autofocus
            :disabled="similarMode"
            :placeholder="similarMode ? '相似检索中：按内容距离排序' : '搜索文件名、正文、拼音或一句话描述…'"
            @focus="recentOpen = true"
            @keyup.enter="run"
          />
          <button v-if="text" class="query-action clear-action" title="清空" aria-label="清空搜索" @click="text = ''">
            <X :size="17" />
          </button>
          <button class="query-action run-action" title="搜索" aria-label="搜索" @click="run">
            <ChevronRight :size="20" />
          </button>
        </div>
        <div v-if="!searched && recentOpen && recentHits.length" class="recent-panel">
          <div class="recent-title">最近文件</div>
          <button v-for="hit in recentHits.slice(0, 6)" :key="hit.id" class="recent-item" @mousedown.prevent="openRecent(hit)">
            <span class="recent-name">{{ hit.name }}</span><span>{{ hit.size_label }}</span>
          </button>
        </div>
      </div>
    </section>

    <template v-if="searched">
      <div class="toolbar">
        <div class="filter-group">
          <button
            v-for="item in KINDS"
            :key="item.v"
            class="filter-pill"
            :class="{ active: kind === item.v }"
            @click="kind = item.v"
          >{{ item.t }}</button>
        </div>
        <div class="toolbar-spacer"></div>
        <button class="tool-button" :class="{ active: fulltext }" title="全文检索" @click="fulltext = !fulltext">全文</button>
        <button class="tool-button" :class="{ active: pinyin }" title="拼音检索" @click="pinyin = !pinyin">拼音</button>
        <button class="tool-button" :class="{ active: semantic }" title="语义检索" @click="semantic = !semantic">语义</button>
        <button class="tool-button" :class="{ active: fuzzy }" title="模糊匹配" @click="fuzzy = !fuzzy">模糊</button>
        <button class="tool-button" :class="{ active: visual }" title="跨模态检索" @click="visual = !visual">找图声</button>
        <select v-model="sort" class="sort-select" :disabled="similarMode" aria-label="排序方式">
          <option v-for="item in SORTS" :key="item.v" :value="item.v">{{ item.t }}</option>
        </select>
        <button class="icon-button compact" :title="desc ? '降序' : '升序'" :aria-label="desc ? '降序' : '升序'" @click="desc = !desc">
          <ArrowDownUp :size="16" />
        </button>
        <button v-if="similarMode" class="tool-button" @click="exitSimilar">退出相似</button>
      </div>

      <div class="result-meta">
        <span class="meta-primary">{{ resultLabel }}</span>
        <span class="muted">{{ elapsed.toFixed(0) }} ms</span>
        <span v-if="selectedIds.size" class="muted">已选 {{ selectedIds.size }}</span>
        <span class="toolbar-spacer"></span>
        <button class="meta-button" @click="toggleAll">
          <Check :size="14" /> {{ selectedIds.size === hits.length && hits.length ? "取消全选" : "全选" }}
        </button>
        <button class="meta-button" @click="doExport">
          <Download :size="14" /> 导出
        </button>
      </div>

      <div class="results-layout">
        <section class="gallery-panel">
          <div v-if="similarMode" class="similar-note">相似于：{{ similarSource?.name }}</div>
          <ResultGallery
            :hits="hits"
            :selected-id="selected?.id ?? null"
            :selected-ids="selectedIds"
            :thumb-dir="props.thumbDir"
            :loading="loading"
            :query="text"
            @select="selected = $event"
            @open="open"
            @toggle="toggleSelected"
            @reveal="reveal"
            @rename="rename"
          />
        </section>
      </div>
    </template>
  </div>
</template>

<style scoped>
.search-page { position: relative; height: 100%; display: flex; flex-direction: column; min-height: 0; overflow: hidden; background: var(--bg); }
.triangle-field { position: absolute; z-index: 0; inset: 0 0 50%; overflow: hidden; pointer-events: none; perspective: 900px; }
.triangle-field::after { content: ""; position: absolute; inset: 0; background: linear-gradient(to bottom, transparent 28%, var(--bg) 100%); }
.triangle { position: absolute; display: block; width: 0; height: 0; border-left: 90px solid transparent; border-right: 90px solid transparent; border-bottom: 170px solid rgba(17, 17, 17, .055); transform-style: preserve-3d; filter: drop-shadow(0 12px 14px rgba(0, 0, 0, .06)); animation: triangle-rise var(--duration) linear var(--delay) infinite; }
.triangle-a { left: 10%; top: 58%; --duration: 10s; --delay: -2s; --depth: 45px; --rx-start: 24deg; --rx-end: 110deg; --ry-start: -35deg; --ry-end: 30deg; --rz-start: -18deg; --rz-end: 22deg; }
.triangle-b { left: 29%; top: 67%; border-left-width: 130px; border-right-width: 130px; border-bottom-width: 220px; border-bottom-color: rgba(17, 17, 17, .035); --duration: 13s; --delay: -9s; --depth: -90px; --rx-start: -22deg; --rx-end: 70deg; --ry-start: 35deg; --ry-end: -60deg; --rz-start: 22deg; --rz-end: -28deg; }
.triangle-c { right: 24%; top: 54%; border-left-width: 100px; border-right-width: 100px; border-bottom-width: 185px; border-bottom-color: rgba(17, 17, 17, .045); --duration: 11s; --delay: -5s; --depth: 110px; --rx-start: 30deg; --rx-end: -65deg; --ry-start: -22deg; --ry-end: 58deg; --rz-start: 11deg; --rz-end: -20deg; }
.triangle-d { right: 7%; top: 72%; border-left-width: 150px; border-right-width: 150px; border-bottom-width: 250px; border-bottom-color: rgba(17, 17, 17, .03); --duration: 15s; --delay: -12s; --depth: -45px; --rx-start: -28deg; --rx-end: 55deg; --ry-start: -40deg; --ry-end: 72deg; --rz-start: -27deg; --rz-end: 14deg; }
.triangle-e { left: 51%; top: 83%; border-left-width: 70px; border-right-width: 70px; border-bottom-width: 130px; border-bottom-color: rgba(17, 17, 17, .04); --duration: 9s; --delay: -6s; --depth: 20px; --rx-start: 15deg; --rx-end: -80deg; --ry-start: 48deg; --ry-end: -34deg; --rz-start: 4deg; --rz-end: 35deg; }
@keyframes triangle-rise {
  0% { opacity: 0; transform: translate3d(0, 18vh, var(--depth)) rotateX(var(--rx-start)) rotateY(var(--ry-start)) rotateZ(var(--rz-start)) scale(.72); }
  18% { opacity: .13; }
  57% { opacity: .09; }
  78%, 100% { opacity: 0; transform: translate3d(0, -22vh, var(--depth)) rotateX(var(--rx-end)) rotateY(var(--ry-end)) rotateZ(var(--rz-end)) scale(1.12); }
}
@media (prefers-reduced-motion: reduce) { .triangle { animation-duration: 1ms; animation-iteration-count: 1; } }
.hero-search, .toolbar, .result-meta, .results-layout { position: relative; z-index: 1; }
.hero-search { flex: 1; min-height: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 34px; padding: 24px; }
.searched .hero-search { flex: none; align-items: stretch; gap: 0; padding: 26px clamp(16px, 8vw, 140px) 15px; }
.hero-copy { text-align: center; }
.hero-kicker { display: none; }
.brand-name { margin-bottom: 18px; color: #111; font: 600 16px/1 Consolas, "Segoe UI", sans-serif; letter-spacing: 0; }
h1 { min-height: 1.4em; margin: 0; color: #111; font-size: 42px; font-weight: 450; letter-spacing: 0; }
.caret { display: inline-block; width: 2px; height: .9em; margin-left: 4px; vertical-align: -.08em; background: var(--accent); animation: blink 1s steps(1) infinite; }
@keyframes blink { 50% { opacity: 0; } }
.query-wrap { position: relative; width: min(680px, 100%); }
.searched .query-wrap { width: 100%; }
.query-shell { width: 100%; height: 62px; display: flex; align-items: center; gap: 12px; padding: 0 12px 0 22px; border: 1px solid #111; border-radius: 32px; background: #fff; box-shadow: 0 12px 30px rgba(0,0,0,.08); transition: border-color .15s, box-shadow .15s; }
.query-shell.focused { width: 100%; height: 50px; border-radius: 25px; background: #fff; box-shadow: none; }
.query-shell:focus-within { border-color: #111; box-shadow: 0 0 0 3px rgba(17, 17, 17, .08); }
.query-icon { flex: none; color: #111; }
.query-shell input { flex: 1; min-width: 0; height: 100%; padding: 0; border: 0; outline: 0; background: transparent; color: var(--text); font-size: 15px; }
.query-shell input::placeholder { color: #777; }
.recent-panel { position: absolute; z-index: 10; top: calc(100% + 8px); left: 0; width: 100%; padding: 8px; border: 1px solid #111; border-radius: 3px; background: #fff; box-shadow: 0 10px 28px rgba(0,0,0,.12); }
.recent-title { padding: 5px 9px 8px; color: #666; font-size: 10px; }
.recent-item { width: 100%; display: flex; justify-content: space-between; gap: 12px; padding: 8px 9px; border: 0; border-radius: 2px; background: transparent; color: #111; text-align: left; font-size: 11px; }
.recent-item:hover { background: #f0f0f0; }.recent-item > span:last-child { flex: none; color: #666; }
.recent-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.query-action { width: 36px; height: 36px; display: grid; place-items: center; padding: 0; border: 0; border-radius: 50%; }
.query-action:hover { background: #f0f0f0; }
.clear-action { color: #666; }
.run-action { color: #fff; background: #111; }
.run-action:hover { background: #333; }
.toolbar { display: flex; align-items: center; gap: 7px; flex-wrap: wrap; padding: 0 clamp(16px, 8vw, 140px) 11px; }
.filter-group { display: flex; gap: 5px; overflow-x: auto; }
.filter-pill, .tool-button, .meta-button { padding: 5px 10px; border: 1px solid transparent; border-radius: 2px; background: transparent; color: #555; font-size: 11px; white-space: nowrap; }
.filter-pill:hover, .tool-button:hover, .meta-button:hover { background: #f0f0f0; color: #111; }
.filter-pill.active, .tool-button.active { color: #fff; background: #111; border-color: #111; }
.toolbar-spacer { flex: 1; }
.sort-select { height: 29px; padding: 0 8px; border-radius: 5px; font-size: 11px; }
.compact { width: 30px; height: 29px; display: grid; place-items: center; padding: 0; border-radius: 2px; }
.result-meta { display: flex; align-items: center; gap: 10px; min-height: 34px; padding: 0 clamp(16px, 8vw, 140px) 10px; color: #555; font-size: 11px; }
.meta-primary { color: #111; }
.meta-button { display: inline-flex; align-items: center; gap: 5px; padding-inline: 7px; }
.results-layout { flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr); gap: 12px; padding: 0 clamp(16px, 8vw, 140px) 18px; }
.gallery-panel { min-width: 0; min-height: 0; overflow: hidden; border-top: 1px solid #111; padding-top: 10px; }
.similar-note { margin: 0 0 8px; color: #444; font-size: 11px; }
@media (max-width: 860px) { .results-layout { padding-inline: 16px; } .searched .hero-search { padding-inline: 16px; } .toolbar, .result-meta { padding-inline: 16px; } }
@media (max-width: 640px) { h1 { font-size: 28px; }.results-layout { overflow: auto; } .toolbar { overflow-x: auto; flex-wrap: nowrap; } .filter-group { flex: none; } }
</style>
