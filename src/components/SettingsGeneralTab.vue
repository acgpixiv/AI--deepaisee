<script setup lang="ts">
import { computed } from "vue";
import { FolderPlus, Languages, Rocket, Save, X } from "lucide-vue-next";
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
.plain-label { justify-content: flex-start; }
.inline-check { display: inline-flex; align-items: center; gap: 6px; color: #222; }
.inline-check input { width: auto; accent-color: #111; }
</style>
