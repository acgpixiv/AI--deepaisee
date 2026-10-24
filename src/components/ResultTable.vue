<script setup lang="ts">
// 搜索结果表格：名称 / 类型 / 大小 / 修改时间 / 位置 / 匹配度
import { computed } from "vue";
import type { Hit } from "../api";
import { fmtTime, humanSize, kindColor } from "../store";

const props = defineProps<{
  hits: Hit[];
  selectedId: number | null;
  loading?: boolean;
  /** 当前查询词，用于在正文片段里高亮 */
  query?: string;
}>();

const emit = defineEmits<{
  (e: "select", hit: Hit): void;
  (e: "open", hit: Hit): void;
  (e: "reveal", hit: Hit): void;
}>();

const rows = computed(() => props.hits);

function pct(h: Hit): string {
  return (h.score > 0 ? h.score : 0).toFixed(2);
}

// 视觉标签很多（图片约 5~8 个），表格里只展示前 3 个最"有用"的，
// 其余放 title 里。优先展示和当前查询相关的那个。
function tagList(h: Hit): string[] {
  if (!h.tags) return [];
  return h.tags.split(",").filter(Boolean);
}

function shownTags(h: Hit): string[] {
  const all = tagList(h);
  if (!all.length) return [];
  return all.slice(0, 3);
}

// ------------------------------------------------------------ 正文片段高亮
function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function rxEsc(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** 片段里的查询词加 <mark>。先转义再做替换，避免正文里的尖括号变成标签。 */
function highlight(s: string): string {
  const out = esc(s);
  const q = (props.query || "").trim();
  if (!q) return out;
  const terms = Array.from(
    new Set(
      [q, ...q.split(/[\s,，、;；]+/)]
        .map((t) => t.trim())
        .filter((t) => t.length >= 1)
    )
  ).sort((a, b) => b.length - a.length);
  return out.replace(new RegExp(terms.map(rxEsc).join("|"), "gi"), (m) => `<mark>${m}</mark>`);
}
</script>

<template>
  <div class="wrap">
    <div class="head">
      <div class="c name">名称</div>
      <div class="c kind">类型</div>
      <div class="c size">大小</div>
      <div class="c time">修改时间</div>
      <div class="c score">匹配</div>
      <div class="c dir">所在位置</div>
    </div>

    <div v-if="loading" class="empty">正在检索…</div>
    <div v-else-if="!rows.length" class="empty">
      <div class="big">没有找到匹配的文件</div>
      <div class="hint">
        换个说法试试；或到「扫描」页把要检索的目录加进索引。
      </div>
    </div>

    <div v-else class="body">
      <div
        v-for="h in rows"
        :key="h.id"
        class="row"
        :class="{ active: h.id === selectedId }"
        @click="emit('select', h)"
        @dblclick="emit('open', h)"
      >
        <div class="c name">
          <span class="dot" :style="{ background: kindColor(h.kind) }"></span>
          <div class="nwrap">
            <div class="nline">
              <span class="nm" :title="h.name">{{ h.name }}</span>
              <span v-if="h.reason" class="reason">{{ h.reason }}</span>
              <span
                v-for="t in shownTags(h)"
                :key="t"
                class="tag"
                :title="tagList(h).join(' · ')"
                >{{ t }}</span
              >
            </div>
            <div
              v-if="h.snippet"
              class="snippet"
              :title="h.snippet"
              v-html="highlight(h.snippet)"
            ></div>
          </div>
        </div>
        <div class="c kind">{{ h.kind_label }}</div>
        <div class="c size mono">{{ h.size_label || humanSize(h.size) }}</div>
        <div class="c time mono">{{ h.mtime_label || fmtTime(h.mtime) }}</div>
        <div class="c score">
          <div class="bar">
            <div
              class="fill"
              :style="{
                width: Math.min(100, (h.score || 0) * 100) + '%',
                background: kindColor(h.kind),
              }"
            ></div>
          </div>
          <span class="num mono">{{ pct(h) }}</span>
        </div>
        <div class="c dir" :title="h.dir">{{ h.dir }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.wrap {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}
.head,
.row {
  display: grid;
  grid-template-columns: minmax(200px, 2.2fr) 74px 92px 132px 104px minmax(140px, 1.6fr);
  align-items: center;
  gap: 8px;
}
.head {
  padding: 8px 12px;
  font-size: 12px;
  color: var(--muted);
  border-bottom: 1px solid var(--border);
  background: var(--panel-2);
  border-radius: 8px 8px 0 0;
}
.body {
  flex: 1;
  overflow: auto;
  min-height: 0;
}
.row {
  padding: 7px 12px;
  border-bottom: 1px solid rgba(36, 48, 74, 0.55);
  cursor: default;
  font-size: 13px;
}
.row:hover { background: rgba(79, 140, 255, 0.07); }
.row.active { background: rgba(79, 140, 255, 0.16); }
.c { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.name { display: flex; align-items: center; gap: 7px; min-width: 0; }
.nwrap { min-width: 0; flex: 1; }
.nline { display: flex; align-items: center; gap: 7px; min-width: 0; }
.dot { width: 8px; height: 8px; border-radius: 2px; flex: none; }
.nm { overflow: hidden; text-overflow: ellipsis; }
/* 正文命中片段：比文件名小一号，两行截断，关键词高亮 */
.snippet {
  margin-top: 2px;
  font-size: 11px;
  line-height: 1.5;
  color: #7f8da8;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  white-space: normal;
}
.snippet :deep(mark) {
  background: rgba(245, 158, 11, 0.25);
  color: #ffd48a;
  border-radius: 3px;
  padding: 0 1px;
}
.reason {
  flex: none;
  font-size: 11px;
  color: var(--muted);
  border: 1px solid var(--border);
  border-radius: 5px;
  padding: 0 5px;
}
/* 视觉标签：区别于"匹配原因"（描边灰），用淡青色底，一眼看出是画面描述 */
.tag {
  flex: none;
  font-size: 11px;
  color: #7fd1c0;
  background: rgba(34, 197, 94, 0.1);
  border: 1px solid rgba(34, 197, 94, 0.25);
  border-radius: 5px;
  padding: 0 5px;
}
.kind { color: var(--muted); }.size, .time { color: var(--muted); font-size: 12px; }
.dir { color: var(--muted); font-size: 12px; }
.score { display: flex; align-items: center; gap: 6px; }
.bar {
  flex: 1;
  height: 4px;
  background: #1b2333;
  border-radius: 3px;
  overflow: hidden;
}
.fill { height: 100%; border-radius: 3px; }
.num { font-size: 11px; color: var(--muted); width: 30px; text-align: right; }
.empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--muted);
}
.empty .big { font-size: 15px; color: #b7c4da; }
.empty .hint { font-size: 12px; }
</style>
