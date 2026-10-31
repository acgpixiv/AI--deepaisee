<script setup lang="ts">
import { computed, ref } from "vue";
import { AlertTriangle, FolderPlus, Languages, Rocket, Save, X } from "lucide-vue-next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { AppConfig } from "../api";
import { t } from "../i18n";
import { toast } from "../store";

const props = defineProps<{
  cfg: AppConfig;
  saveGeneralConfig: () => Promise<boolean>;
}>();

/** 工作区目录列表（后端 cfg.workspace.dirs） */
const workspaceDirs = computed<string[]>(() => (props.cfg.workspace?.dirs as string[]) ?? []);
const recentPaths = computed<string[]>(() => (props.cfg.workspace?.recent_paths as string[]) ?? []);
const pinnedPaths = computed<string[]>(() => (props.cfg.workspace?.pinned_paths as string[]) ?? []);
const savedSearchPaths = computed(() => [...new Set([...pinnedPaths.value, ...recentPaths.value])]);
const professionalWarningOpen = ref(false);

async function addWorkspaceDir() {
  try {
    const picked = await openDialog({ directory: true, multiple: true, title: t("选择工作区目录") });
    if (!picked) return;
    const list = Array.isArray(picked) ? picked : [picked];
    const current = [...workspaceDirs.value];
    for (const dir of list) {
      const normalized = dir.replace(/[\\/]+$/, "");
      if (normalized && !current.some((item) => item.toLowerCase() === normalized.toLowerCase())) {
        current.push(normalized);
      }
    }
    props.cfg.workspace.dirs = current;
  } catch (error) {
    toast(`${t("选择目录失败：")}${error}`, "error");
  }
}

function removeWorkspaceDir(index: number) {
  const list = [...workspaceDirs.value];
  list.splice(index, 1);
  props.cfg.workspace.dirs = list;
}

async function addPinnedSearchPath() {
  try {
    const picked = await openDialog({ directory: true, multiple: true, title: t("添加固定搜索路径") });
    if (!picked) return;
    const list = Array.isArray(picked) ? picked : [picked];
    const current = [...pinnedPaths.value];
    for (const path of list) {
      const normalized = path.replace(/[\\/]+$/, "") || path;
      if (normalized && !current.some((item) => item.toLowerCase() === normalized.toLowerCase())) current.push(normalized);
    }
    props.cfg.workspace.pinned_paths = current;
  } catch (error) {
    toast(t("选择目录失败：") + String(error), "error");
  }
}

function removeSavedSearchPath(path: string) {
  props.cfg.workspace.pinned_paths = pinnedPaths.value.filter((item) => item.toLowerCase() !== path.toLowerCase());
  props.cfg.workspace.recent_paths = recentPaths.value.filter((item) => item.toLowerCase() !== path.toLowerCase());
  if (String(props.cfg.workspace.last_path ?? "").toLowerCase() === path.toLowerCase()) {
    props.cfg.workspace.last_path = "";
    props.cfg.workspace.last_scope = "indexed";
  }
  if (String(props.cfg.workspace.startup_path ?? "").toLowerCase() === path.toLowerCase()) {
    props.cfg.workspace.startup_path = "";
    if (props.cfg.workspace.startup_scope === "fixed") props.cfg.workspace.startup_scope = "indexed";
  }
}

function toggleProfessionalMode() {
  if (props.cfg.ui.professional_mode) {
    props.cfg.ui.professional_mode = false;
    if (props.cfg.model.parallel_mode === "custom") props.cfg.model.parallel_mode = "auto";
    return;
  }
  professionalWarningOpen.value = true;
}

function enableProfessionalMode() {
  props.cfg.ui.professional_mode = true;
  professionalWarningOpen.value = false;
}
</script>

<template>
  <div class="settings-general-tab">
    <section class="settings-section">
      <div class="section-head">
        <div><h2><Languages :size="17" />{{ t("语言与主题") }}</h2><p>{{ t("界面语言和配色方案，修改后立即生效。") }}</p></div>
      </div>
      <div class="form-grid three">
        <label>
          {{ t("界面语言") }}
          <select v-model="props.cfg.ui.language">
            <option value="zh">{{ t("简体中文") }}</option>
            <option value="en">English</option>
          </select>
        </label>
        <label>
          {{ t("主题") }}
          <select v-model="props.cfg.ui.theme">
            <option value="light">{{ t("明亮") }}</option>
            <option value="dark">{{ t("暗色") }}</option>
            <option value="system">{{ t("跟随系统") }}</option>
          </select>
        </label>
        <label>
          {{ t("默认搜索模式") }}
          <select v-model="props.cfg.ui.default_search_mode">
            <option value="fulltext">{{ t("全文搜索") }}</option>
            <option value="semantic">{{ t("语义搜索") }}</option>
            <option value="hybrid">{{ t("综合（全文 + 语义）") }}</option>
          </select>
        </label>
      </div>
      <div class="switch-row">
        <label><input type="checkbox" v-model="props.cfg.ui.remember_window" />{{ t("记住窗口大小和位置") }}</label>
      </div>
    </section>

    <section class="settings-section professional-section">
      <div class="professional-control">
        <h2>{{ t("专业配置") }}</h2>
        <button
          type="button"
          class="boolean-switch"
          :class="{ active: props.cfg.ui.professional_mode }"
          role="switch"
          :aria-checked="!!props.cfg.ui.professional_mode"
          @click="toggleProfessionalMode"
        ><span>{{ t("开启") }}</span><span>{{ t("关闭") }}</span></button>
      </div>
    </section>

    <div v-if="professionalWarningOpen" class="modal-backdrop" role="presentation" @click.self="professionalWarningOpen = false">
      <section class="warning-dialog" role="dialog" aria-modal="true" :aria-label="t('专业配置')">
        <AlertTriangle :size="22" aria-hidden="true" />
        <h3>{{ t("专业配置") }}</h3>
        <p>{{ t("专业配置可能导致部分功能运作不正常。确定开启吗？") }}</p>
        <div class="dialog-actions">
          <button type="button" class="secondary-button" @click="professionalWarningOpen = false">{{ t("取消") }}</button>
          <button type="button" class="save-button" @click="enableProfessionalMode">{{ t("确定") }}</button>
        </div>
      </section>
    </div>

    <section class="settings-section">
      <div class="section-head">
        <div><h2><Rocket :size="17" />{{ t("启动行为") }}</h2><p>{{ t("开机自启与启动时的索引维护策略。") }}</p></div>
      </div>
      <div class="switch-row">
        <label><input type="checkbox" v-model="props.cfg.ui.autostart" />{{ t("开机自动启动") }}</label>
        <label><input type="checkbox" v-model="props.cfg.ui.auto_scan_on_start" />{{ t("启动时自动扫描索引目录") }}</label>
        <label><input type="checkbox" v-model="props.cfg.ui.notify_file_changes" />{{ t("发现文件变化时给出提示") }}</label>
      </div>
      <p class="field-hint">{{ t("启动扫描是增量的：只处理新增和修改过的文件，并把已删除的文件标记为缺失，不会重建整个索引。") }}</p>
    </section>

    <section class="settings-section">
      <div class="section-head">
        <div><h2><FolderPlus :size="17" />{{ t("启动搜索范围") }}</h2><p>{{ t("设置应用启动后使用的搜索范围；记忆会恢复上次关闭时选择的路径。") }}</p></div>
      </div>
      <div class="form-grid two workspace-options">
        <label>
          {{ t("默认搜索范围") }}
          <select v-model="props.cfg.workspace.startup_scope">
            <option value="remember">{{ t("记忆：上次关闭时的路径") }}</option>
            <option value="indexed">{{ t("仅索引") }}</option>
            <option value="all">{{ t("全盘") }}</option>
            <option value="fixed">{{ t("固定路径") }}</option>
          </select>
        </label>
        <label v-if="props.cfg.workspace.startup_scope === 'fixed'">
          {{ t("固定路径") }}
          <select v-model="props.cfg.workspace.startup_path">
            <option v-for="path in savedSearchPaths" :key="path" :value="path">{{ path }}</option>
          </select>
        </label>
      </div>
      <div class="workspace-list search-scope-list">
        <div v-for="path in savedSearchPaths" :key="path" class="workspace-row">
          <span class="mono" :title="path">{{ path }}</span>
          <span class="path-kind">{{ pinnedPaths.some((item) => item.toLowerCase() === path.toLowerCase()) ? t("固定") : t("最近") }}</span>
          <button class="tiny-button" :title="t('删除')" @click="removeSavedSearchPath(path)"><X :size="13" /></button>
        </div>
        <p v-if="!savedSearchPaths.length" class="empty-line">{{ t("暂无保存的搜索路径") }}</p>
      </div>
      <div class="action-row">
        <button class="secondary-button" @click="addPinnedSearchPath"><FolderPlus :size="14" />{{ t("添加固定路径") }}</button>
      </div>
    </section>

    <section class="settings-section">
      <div class="section-head">
        <div>
          <h2><FolderPlus :size="17" />{{ t("工作区") }}</h2>
          <p>{{ t("这里添加的目录不必加入索引，搜索时会实时抽取正文一起参与匹配。") }}</p>
        </div>
        <span class="status" :class="{ on: props.cfg.workspace?.enabled && workspaceDirs.length > 0 }">
          <span></span>{{ props.cfg.workspace?.enabled ? `${workspaceDirs.length} ${t("个目录")}` : t("已关闭") }}
        </span>
      </div>

      <div class="workspace-list">
        <div v-for="(dir, index) in workspaceDirs" :key="dir + index" class="workspace-row">
          <span class="mono" :title="dir">{{ dir }}</span>
          <button class="tiny-button" :title="t('移除')" @click="removeWorkspaceDir(index)"><X :size="13" /></button>
        </div>
        <p v-if="workspaceDirs.length === 0" class="empty-line">{{ t("还没有工作区目录，添加后主页搜索会自动包含这些目录里的文件。") }}</p>
      </div>

      <div class="action-row">
        <button class="secondary-button" @click="addWorkspaceDir"><FolderPlus :size="14" />{{ t("添加目录") }}</button>
      </div>

      <div class="form-grid two workspace-options">
        <label>{{ t("单文件大小上限（MB）") }}<input type="number" min="1" v-model.number="props.cfg.workspace.max_file_mb" /></label>
        <label>{{ t("递归深度（0 = 不递归）") }}<input type="number" min="0" v-model.number="props.cfg.workspace.depth" /></label>
      </div>
      <div class="switch-row">
        <label><input type="checkbox" v-model="props.cfg.workspace.live_extract" />{{ t("对未入库文件实时抽取正文") }}</label>
        <label><input type="checkbox" v-model="props.cfg.workspace.enabled" />{{ t("启用工作区实时检索") }}</label>
      </div>
    </section>

    <div class="action-row">
      <button class="save-button" @click="props.saveGeneralConfig"><Save :size="14" />{{ t("保存通用设置") }}</button>
    </div>
  </div>
</template>

<style scoped>
.field-hint { margin: 12px 0 0; color: #555; font-size: 11px; line-height: 1.5; }
.workspace-list { margin-top: 14px; border-top: 1px solid #111; }
.workspace-row { display: flex; align-items: center; gap: 10px; min-height: 34px; border-bottom: 1px solid #ddd; font-size: 11px; }
.workspace-row .mono { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: #222; }
.workspace-row .tiny-button { width: 26px; height: 26px; display: inline-grid; place-items: center; padding: 0; background: #fff; border: 1px solid #111; border-radius: 2px; }
.workspace-row .tiny-button:hover { background: #f0f0f0; }
.empty-line { padding: 14px 0; color: #555; font-size: 11px; }
.workspace-options { margin-top: 16px; }
.workspace-options.two { grid-template-columns: repeat(2, minmax(0, 1fr)); }
.path-kind { flex: none; color: #666; font-size: 10px; }
.plain-label { justify-content: flex-start; }
.professional-section { padding: 12px 14px; }
.professional-control { display: flex; align-items: center; justify-content: space-between; gap: 14px; width: 100%; }
.professional-control h2 { margin: 0; font-size: 13px; text-align: left; }
.boolean-switch { display: inline-grid; grid-template-columns: 1fr 1fr; min-width: 112px; padding: 0; border: 1px solid #111; border-radius: 2px; color: #111; background: #fff; cursor: pointer; }
.boolean-switch span { padding: 5px 10px; font-size: 11px; text-align: center; }
.boolean-switch span:first-child { background: #fff; color: #111; }
.boolean-switch span:last-child { background: #111; color: #fff; }
.boolean-switch.active span:first-child { background: #111; color: #fff; }
.boolean-switch.active span:last-child { background: #fff; color: #111; }
.inline-check { display: inline-flex; align-items: center; gap: 6px; color: #222; }
.inline-check input { width: auto; accent-color: #111; }
.modal-backdrop { position: fixed; inset: 0; z-index: 200; display: grid; place-items: center; padding: 16px; background: rgba(0,0,0,.42); }
.warning-dialog { width: min(420px, 100%); padding: 22px; border: 1px solid #111; background: #fff; color: #111; box-shadow: 0 18px 50px rgba(0,0,0,.22); }
.warning-dialog > svg { color: #8c1d24; }
.warning-dialog h3 { margin: 10px 0 6px; font-size: 16px; }
.warning-dialog p { margin: 0; color: #444; font-size: 12px; line-height: 1.6; }
.dialog-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 20px; }
</style>
