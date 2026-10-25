<script setup lang="ts">
// 树状图视图：面包屑 + 下钻 + 目录列表
import { computed, onMounted, ref } from "vue";
import { api, type TreeNode } from "../api";
import { humanSize, kindColor, toast } from "../store";
import Treemap from "../components/Treemap.vue";

const depth = ref(3);
const root = ref("");
const node = ref<TreeNode | null>(null);
const trail = ref<TreeNode[]>([]);
const hovered = ref<TreeNode | null>(null);
const loading = ref(false);

async function load(path = "", push = false) {
  loading.value = true;
  try {
    const n = await api.treeData(path || undefined, depth.value);
    node.value = n;
    if (push) trail.value.push(n);
    else if (!path) trail.value = [n];
  } catch (e) {
    toast(`读取目录树失败：${e}`, "error");
  } finally {
    loading.value = false;
  }
}

function drill(n: TreeNode) {
  if (!n.children?.length) return;
  load(n.path, true);
}

function up() {
  if (trail.value.length <= 1) return;
  trail.value.pop();
  const prev = trail.value[trail.value.length - 1];
  node.value = prev;
}

const children = computed(() => {
  const list = [...(node.value?.children || [])];
  list.sort((a, b) => b.size - a.size);
  return list;
});

const totalSize = computed(() => children.value.reduce((a, c) => a + c.size, 0));
const totalCount = computed(() => children.value.reduce((a, c) => a + c.count, 0));
const kindRows = computed(() => {
  const source = hovered.value || node.value;
  return Object.entries(source?.kinds || {})
    .map(([kind, count]) => ({ kind, count }))
    .sort((a, b) => b.count - a.count);
});

function share(s: number): string {
  if (!totalSize.value) return "0%";
  return ((s / totalSize.value) * 100).toFixed(1) + "%";
}

function mainKind(k: Record<string, number>): string {
  const e = Object.entries(k || {}).sort((a, b) => b[1] - a[1]);
  return e.length ? e[0][0] : "other";
}

onMounted(() => load());
</script>

<template>
  <div class="view">
    <div class="bar">
      <button class="ghost" :disabled="trail.length <= 1" @click="up">↑ 上一级</button>
      <div class="crumb">
        <template v-for="(t, i) in trail" :key="t.path + i">
          <span class="sep" v-if="i">/</span>
          <span class="c" :class="{ last: i === trail.length - 1 }">{{ t.name || "全部" }}</span>
        </template>
      </div>
      <span class="grow"></span>
      <span class="muted tiny">{{ totalCount.toLocaleString() }} 个文件 · {{ humanSize(totalSize) }}</span>
      <select v-model.number="depth" class="sel" @change="load(node?.path || '', false)">
        <option :value="2">2 层</option>
        <option :value="3">3 层</option>
        <option :value="4">4 层</option>
        <option :value="5">5 层</option>
      </select>
      <button @click="load(node?.path || '', false)" :disabled="loading">刷新</button>
    </div>

    <div class="body">
      <div class="chart card">
        <Treemap :node="node" @drill="drill" @hover="hovered = $event" />
      </div>
      <div class="side card">
        <div class="sh">
          {{ hovered ? hovered.name : "目录占比" }}
        </div>
        <div v-if="kindRows.length" class="kind-cards">
          <div v-for="row in kindRows" :key="row.kind" class="kind-card">
            <span class="dot" :style="{ background: kindColor(row.kind) }"></span>
            <span>{{ row.kind }}</span>
            <b class="mono">{{ row.count }}</b>
          </div>
        </div>
        <div class="list">
          <div
            v-for="c in children.slice(0, 60)"
            :key="c.path"
            class="item"
            @dblclick="drill(c)"
            :title="c.path"
          >
            <span class="dot" :style="{ background: kindColor(mainKind(c.kinds)) }"></span>
            <span class="nm">{{ c.name }}</span>
            <span class="num mono">{{ humanSize(c.size) }}</span>
            <span class="pct mono">{{ share(c.size) }}</span>
          </div>
          <div v-if="!children.length" class="none">暂无数据</div>
        </div>
        <div class="foot muted tiny">
          单击方块下钻 · 双击右侧目录进入 · 面积 = 体积
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.view { display: flex; flex-direction: column; height: 100%; gap: 8px; min-height: 0; }
.bar { display: flex; align-items: center; gap: 8px; }
.crumb { display: flex; align-items: center; gap: 6px; font-size: 13px; overflow: hidden; }
.crumb .c { color: var(--muted); max-width: 220px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.crumb .c.last { color: var(--text); font-weight: 600; }
.sep { color: #3b4a66; }
.grow { flex: 1; }
.sel { height: 34px; padding: 0 8px; }
.body { flex: 1; display: grid; grid-template-columns: minmax(0, 1fr) 330px; gap: 8px; min-height: 0; }
.chart { padding: 6px; min-width: 0; overflow: hidden; }
.side { display: flex; flex-direction: column; overflow: hidden; }
.sh { padding: 10px 12px; font-size: 13px; color: #b7c4da; border-bottom: 1px solid var(--border); }
.list { flex: 1; overflow: auto; min-height: 0; }
.kind-cards { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 5px; padding: 8px; border-bottom: 1px solid #111; }
.kind-card { min-width: 0; display: flex; align-items: center; gap: 5px; padding: 6px 7px; border: 1px solid #aaa; background: #fafafa; font-size: 10px; }
.kind-card > span:nth-child(2) { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kind-card b { margin-left: auto; font-weight: 500; }
.item {
  display: grid;
  grid-template-columns: 10px 1fr 78px 52px;
  gap: 7px;
  align-items: center;
  padding: 6px 12px;
  font-size: 12px;
  border-bottom: 1px solid rgba(36, 48, 74, .5);
  cursor: default;
}
.item:hover { background: rgba(79, 140, 255, .07); }
.dot { width: 8px; height: 8px; border-radius: 2px; }
.nm { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.num, .pct { color: var(--muted); text-align: right; font-size: 11px; }
.none { padding: 20px; text-align: center; color: var(--muted); font-size: 12px; }
.foot { padding: 8px 12px; border-top: 1px solid var(--border); }
.tiny { font-size: 11px; }
/* Flat white/black tree surface. */
.view { color: #111; }
.bar { border-bottom: 1px solid #111; padding-bottom: 8px; }
.crumb .c, .crumb .c.last, .sh, .num, .pct { color: #111; }
.sep { color: #777; }
.chart, .side { border: 1px solid #111; border-radius: 2px; background: #fff; }
.sh { border-color: #111; }
.item { border-bottom-color: #ddd; }
.item:hover { background: #f0f0f0; }
.sel { border-color: #111; border-radius: 2px; background: #fff; color: #111; }
</style>
