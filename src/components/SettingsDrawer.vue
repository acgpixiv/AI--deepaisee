<script setup lang="ts">
// 右侧抽屉：规则 / 局域网 / 日志 / 设置（模型另有独立的「模型」页）
import { onMounted, ref, watch } from "vue";
import { api, type Activity, type AppConfig, type Rule, type ServerStatus, type WatchStatus } from "../api";
import { refreshStats, toast } from "../store";

const props = defineProps<{ open: boolean; tab: string }>();
const emit = defineEmits<{ (e: "close"): void; (e: "tab", t: string): void }>();

const TABS = [
  { k: "rules", t: "分类规则" },
  { k: "lan", t: "局域网" },
  { k: "log", t: "日志" },
  { k: "settings", t: "设置" },
];

const cfg = ref<AppConfig | null>(null);
const rules = ref<Rule[]>([]);
const logs = ref<Activity[]>([]);
const logKind = ref("");
const srv = ref<ServerStatus | null>(null);
const watchState = ref<WatchStatus | null>(null);
const dataDir = ref("");

const newRule = ref<Rule>({
  id: 0,
  name: "",
  field: "name",
  op: "contains",
  value: "",
  category: "",
  priority: 10,
  enabled: true,
});

async function loadAll() {
  try {
    cfg.value = await api.getConfig();
  } catch {}
  try {
    rules.value = await api.rulesList();
  } catch {}
  try {
    logs.value = await api.activityList(logKind.value || undefined, 300);
  } catch {}
  try {
    srv.value = await api.serverStatus();
  } catch {}
  try {
    watchState.value = await api.watchStatus();
  } catch {}
  try {
    dataDir.value = await api.dataDir();
  } catch {}
}

watch(
  () => props.open,
  (v) => {
    if (v) loadAll();
  }
);
watch(
  () => props.tab,
  (t) => {
    if (t === "log") refreshLogs();
  }
);

async function refreshLogs() {
  try {
    logs.value = await api.activityList(logKind.value || undefined, 300);
  } catch {}
}

// ---------------------------------------------------------------- 规则
async function addRule() {
  if (!newRule.value.value.trim() || !newRule.value.category.trim())
    return toast("请填写匹配内容与分类名", "warn");
  const r: Rule = { ...newRule.value, id: 0 };
  rules.value.push(r);
  await saveRules();
  newRule.value.value = "";
  newRule.value.category = "";
}

async function saveRules() {
  try {
    await api.rulesSave(rules.value.map((r, i) => ({ ...r, priority: r.priority ?? i })));
    toast("规则已保存", "ok");
  } catch (e) {
    toast(`保存失败：${e}`, "error");
  }
}

async function applyRules() {
  try {
    const n = await api.rulesApply();
    toast(`已按规则重新分类 ${n} 个文件`, "ok");
    await refreshStats();
  } catch (e) {
    toast(`应用失败：${e}`, "error");
  }
}

async function delRule(i: number) {
  rules.value.splice(i, 1);
  await saveRules();
}

// ---------------------------------------------------------------- 局域网
async function startServer() {
  try {
    const url = await api.serverStart();
    srv.value = { running: true, url };
    toast(`局域网服务已启动：${url}`, "ok");
  } catch (e) {
    toast(`启动失败：${e}`, "error");
  }
}
async function stopServer() {
  await api.serverStop();
  srv.value = { running: false, url: "" };
  toast("局域网服务已停止", "warn");
}

async function toggleWatch() {
  try {
    if (watchState.value?.running) {
      await api.watchStop();
      toast("实时监控已停止", "warn");
    } else {
      const n = await api.watchStart();
      toast(`实时监控已开启，监视 ${n} 个目录`, "ok");
    }
    watchState.value = await api.watchStatus();
  } catch (e) {
    toast(`${e}`, "error");
  }
}

// ---------------------------------------------------------------- 设置
async function saveCfg() {
  if (!cfg.value) return;
  try {
    await api.saveConfig(cfg.value);
    toast("设置已保存", "ok");
  } catch (e) {
    toast(`保存失败：${e}`, "error");
  }
}

async function vacuum() {
  try {
    await api.dbVacuum();
    toast("数据库已压缩", "ok");
    await refreshStats();
  } catch (e) {
    toast(`${e}`, "error");
  }
}

async function clearAll() {
  if (!confirm("确定清空全部索引？文件本身不会被删除。")) return;
  try {
    await api.dbClear();
    toast("索引已清空", "ok");
    await refreshStats();
  } catch (e) {
    toast(`${e}`, "error");
  }
}

onMounted(loadAll);
</script>

<template>
  <transition name="slide">
    <aside v-if="open" class="drawer">
      <div class="dh">
        <span class="dt">设置与工具</span>
        <button class="x" @click="emit('close')">×</button>
      </div>
      <div class="tabs">
        <button
          v-for="t in TABS"
          :key="t.k"
          class="tab"
          :class="{ active: tab === t.k }"
          @click="emit('tab', t.k)"
        >
          {{ t.t }}
        </button>
      </div>

      <div class="dc">
        <!-- ------------------------------------------------ 规则 -->
        <template v-if="tab === 'rules'">
          <div class="sec-t">新增规则</div>
          <div class="ruleform">
            <select v-model="newRule.field">
              <option value="name">文件名</option>
              <option value="ext">扩展名</option>
              <option value="path">完整路径</option>
              <option value="kind">文件类型</option>
            </select>
            <select v-model="newRule.op">
              <option value="contains">包含</option>
              <option value="equals">等于</option>
              <option value="startswith">开头是</option>
              <option value="endswith">结尾是</option>
              <option value="regex">正则</option>
            </select>
            <input v-model="newRule.value" placeholder="匹配内容，如 合同 / .jpg" />
            <input v-model="newRule.category" placeholder="归为分类" />
            <button class="primary" @click="addRule">添加</button>
          </div>

          <div class="sec-t mt">已有规则</div>
          <div class="rules">
            <div v-for="(r, i) in rules" :key="i" class="rule">
              <input type="checkbox" v-model="r.enabled" />
              <span class="mono tiny">{{ r.field }} {{ r.op }} "{{ r.value }}"</span>
              <span class="arrow">→</span>
              <span class="cat">{{ r.category }}</span>
              <button class="mini" @click="delRule(i)">删</button>
            </div>
            <div v-if="!rules.length" class="none">还没有规则</div>
          </div>
          <div class="row">
            <button class="primary" @click="applyRules">按规则重新分类</button>
            <button class="ghost" @click="saveRules">保存</button>
          </div>
        </template>

        <!-- ------------------------------------------------ 局域网 -->
        <template v-else-if="tab === 'lan'">
          <div class="sec-t">手机 / 平板访问</div>
          <div class="row">
            <span class="badge" :class="srv?.running ? 'ok' : 'muted'">
              {{ srv?.running ? "运行中" : "已停止" }}
            </span>
          </div>
          <div v-if="srv?.running" class="url mono">{{ srv.url }}</div>
          <div class="row">
            <button class="primary" v-if="!srv?.running" @click="startServer">启动服务</button>
            <button class="ghost" v-else @click="stopServer">停止服务</button>
          </div>
          <p class="muted tiny">
            同一局域网内的手机浏览器打开上面的地址即可搜索、预览、下载。
            端口在「设置」里改（默认 8770）。
          </p>

          <div class="sec-t mt">实时监控</div>
          <div class="row">
            <span class="badge" :class="watchState?.running ? 'ok' : 'muted'">
              {{ watchState?.running ? `监控 ${watchState.roots} 个目录` : "未开启" }}
            </span>
            <button @click="toggleWatch">{{ watchState?.running ? "停止" : "开启" }}</button>
          </div>
          <p class="muted tiny">
            开启后新增/修改/删除文件会自动更新索引，无需手动重扫。
          </p>
        </template>

        <!-- ------------------------------------------------ 日志 -->
        <template v-else-if="tab === 'log'">
          <div class="row">
            <select v-model="logKind" @change="refreshLogs">
              <option value="">全部</option>
              <option value="scan">扫描</option>
              <option value="index">索引</option>
              <option value="model">模型</option>
              <option value="op">文件操作</option>
              <option value="watch">监控</option>
              <option value="server">局域网</option>
              <option value="error">错误</option>
            </select>
            <button class="ghost" @click="refreshLogs">刷新</button>
          </div>
          <div class="logs">
            <div v-for="l in logs" :key="l.id" class="logline">
              <span class="lt mono">{{ new Date(l.ts * 1000).toLocaleTimeString() }}</span>
              <span class="lk" :class="l.kind">{{ l.kind }}</span>
              <span class="lm">{{ l.msg }}</span>
            </div>
            <div v-if="!logs.length" class="none">暂无日志</div>
          </div>
        </template>

        <!-- ------------------------------------------------ 设置 -->
        <template v-else>
          <template v-if="cfg">
            <div class="sec-t">扫描</div>
            <div class="field">
              <label>最大文件（MB，0=不限）</label>
              <input type="number" v-model.number="cfg.scan.max_size_mb" />
            </div>
            <div class="field">
              <label>最小文件（KB）</label>
              <input type="number" v-model.number="cfg.scan.min_size_kb" />
            </div>

            <div class="sec-t mt">索引</div>
            <div class="field">
              <label>正文最大字符数</label>
              <input type="number" v-model.number="cfg.index.max_text_chars" />
            </div>
            <div class="field">
              <label>视频抽帧数</label>
              <input type="number" v-model.number="cfg.index.video_frames" />
            </div>

            <div class="sec-t mt">搜索</div>
            <div class="field">
              <label>语义权重 {{ Number(cfg.search.semantic_weight).toFixed(2) }}</label>
              <input
                type="range"
                min="0"
                max="1"
                step="0.05"
                v-model.number="cfg.search.semantic_weight"
              />
            </div>
            <div class="field">
              <label>每页条数</label>
              <input type="number" v-model.number="cfg.search.page_size" />
            </div>

            <div class="sec-t mt">局域网</div>
            <div class="field">
              <label>端口</label>
              <input type="number" v-model.number="cfg.server.port" />
            </div>
            <div class="field">
              <label>访问令牌（留空不校验）</label>
              <input v-model="cfg.server.token" />
            </div>

            <div class="row">
              <button class="primary" @click="saveCfg">保存设置</button>
            </div>

            <div class="sec-t mt">数据</div>
            <div class="pathbox mono">{{ dataDir }}</div>
            <div class="row">
              <button class="ghost" @click="vacuum">压缩数据库</button>
              <button class="ghost danger" @click="clearAll">清空索引</button>
            </div>
          </template>
          <div v-else class="none">读取配置中…</div>
        </template>
      </div>
    </aside>
  </transition>
</template>

<style scoped>
.drawer {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  width: 460px;
  background: var(--panel);
  border-left: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  z-index: 40;
  box-shadow: -18px 0 40px rgba(0, 0, 0, .45);
}
.slide-enter-active, .slide-leave-active { transition: transform .18s ease; }
.slide-enter-from, .slide-leave-to { transform: translateX(100%); }
.dh {
  display: flex; align-items: center; justify-content: space-between;
  padding: 12px 14px; border-bottom: 1px solid var(--border);
}
.dt { font-size: 14px; font-weight: 600; }
.x { background: transparent; border: none; font-size: 20px; color: var(--muted); padding: 0 6px; }
.tabs { display: flex; gap: 4px; padding: 10px 12px; flex-wrap: wrap; }
.dc { flex: 1; overflow: auto; padding: 4px 14px 20px; min-height: 0; }
.sec-t { font-size: 12px; color: var(--muted); margin: 10px 0 8px; }
.sec-t.mt { margin-top: 20px; }
.row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.field { display: flex; flex-direction: column; gap: 5px; margin-bottom: 10px; }
.field label { font-size: 12px; color: var(--muted); }
.field input, .field select { width: 100%; }
.badge {
  display: inline-block; font-size: 11px; padding: 3px 9px; border-radius: 999px;
  border: 1px solid var(--border); color: var(--muted);
}
.badge.ok { color: #22c55e; border-color: rgba(34, 197, 94, .45); background: rgba(34, 197, 94, .12); }
.badge.warn { color: #f59e0b; border-color: rgba(245, 158, 11, .45); background: rgba(245, 158, 11, .12); }
.caps { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 8px; }
.caps li { display: flex; align-items: center; gap: 8px; font-size: 12px; }
.caps b { width: 96px; font-weight: 500; }
.ok { color: #22c55e; }
.warn { color: #f59e0b; }
.ruleform { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.ruleform input, .ruleform select { min-width: 0; }
.rules { display: flex; flex-direction: column; gap: 5px; max-height: 240px; overflow: auto; }
.rule {
  display: flex; align-items: center; gap: 6px; font-size: 12px;
  background: var(--panel-2); border: 1px solid var(--border); border-radius: 7px; padding: 5px 8px;
}
.arrow { color: var(--muted); }
.cat { color: var(--accent); flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mini { padding: 1px 7px; font-size: 11px; }
.url {
  margin: 8px 0; padding: 8px 10px; background: var(--panel-2);
  border: 1px solid var(--border); border-radius: 7px; font-size: 12px; word-break: break-all;
}
.logs { display: flex; flex-direction: column; gap: 3px; }
.logline { display: grid; grid-template-columns: 66px 58px 1fr; gap: 6px; font-size: 11px; padding: 3px 0; border-bottom: 1px solid rgba(36,48,74,.4); }
.lt { color: #64748b; }
.lk { color: var(--accent); }
.lk.error { color: var(--danger); }
.lm { color: #c4d0e3; word-break: break-all; }
.pathbox {
  font-size: 11px; color: var(--muted); background: var(--panel-2);
  border: 1px solid var(--border); border-radius: 7px; padding: 7px 9px; margin-bottom: 8px;
  word-break: break-all;
}
.danger { color: #f87171; border-color: rgba(239, 68, 68, .4); }
.none { color: var(--muted); font-size: 12px; padding: 10px 0; }
.tiny { font-size: 11px; line-height: 1.6; }
</style>
