<script setup lang="ts">
import { computed } from "vue";
import { Power, Save, Server, Square, Wifi } from "lucide-vue-next";
import type { AppConfig, ServerStatus, WatchStatus } from "../api";
import { t } from "../i18n";

const props = defineProps<{
  cfg: AppConfig;
  srv: ServerStatus | null;
  watcher: WatchStatus | null;
  watchRootsText: string;
  saveLanConfig: () => Promise<boolean>;
  toggleServer: () => Promise<void>;
  toggleWatcher: () => Promise<void>;
}>();

const emit = defineEmits<{
  (event: "update-watch-roots", value: string): void;
}>();

const watchRoots = computed({
  get: () => props.watchRootsText,
  set: (value: string) => emit("update-watch-roots", value),
});
</script>

<template>
  <div class="settings-lan-tab">
    <section class="settings-section">
      <div class="section-head">
        <div><h2><Server :size="17" />{{ t("LAN 服务") }}</h2><p>{{ t("在同一局域网内访问索引和搜索。") }}</p></div>
        <span class="status" :class="{ on: props.srv?.running }"><span></span>{{ props.srv?.running ? t("运行中") : t("已停止") }}</span>
      </div>
      <div v-if="props.srv?.running" class="server-url mono">{{ props.srv.url }}</div>
      <div class="form-grid three">
        <label>{{ t("监听地址") }}<input v-model="props.cfg.server.bind" /></label>
        <label>{{ t("端口") }}<input type="number" v-model.number="props.cfg.server.port" /></label>
        <label>{{ t("访问令牌") }}<input v-model="props.cfg.server.token" :placeholder="t('留空不校验')" /></label>
      </div>
      <div class="switch-row">
        <label><input type="checkbox" v-model="props.cfg.server.enabled" />{{ t("允许启动局域网服务") }}</label>
        <label><input type="checkbox" v-model="props.cfg.server.allow_write" />{{ t("允许远程写操作") }}</label>
      </div>
      <div class="action-row">
        <button class="save-button" @click="props.saveLanConfig"><Save :size="14" />{{ t("保存配置") }}</button>
        <button class="secondary-button" @click="props.toggleServer"><Power :size="14" />{{ props.srv?.running ? t("停止服务") : t("启动服务") }}</button>
      </div>
    </section>

    <section class="settings-section">
      <div class="section-head">
        <div><h2><Wifi :size="17" />{{ t("实时监控") }}</h2><p>{{ t("文件变化后自动更新索引。") }}</p></div>
        <span class="status" :class="{ on: props.watcher?.running }"><span></span>{{ props.watcher?.running ? `${props.watcher.roots} ${t("个目录")}` : t("未开启") }}</span>
      </div>
      <label class="wide-label">{{ t("监控目录（每行一个，留空则沿用扫描目录）") }}<textarea v-model="watchRoots" rows="3" placeholder="D:\\Documents"></textarea></label>
      <div class="form-grid three"><label>{{ t("防抖秒数") }}<input type="number" min="0" v-model.number="props.cfg.watch.debounce_secs" /></label></div>
      <div class="switch-row">
        <label><input type="checkbox" v-model="props.cfg.watch.enabled" />{{ t("允许实时监控") }}</label>
        <label><input type="checkbox" v-model="props.cfg.watch.auto_index" />{{ t("变化后自动索引") }}</label>
      </div>
      <div class="action-row">
        <button class="save-button" @click="props.saveLanConfig"><Save :size="14" />{{ t("保存配置") }}</button>
        <button class="secondary-button" @click="props.toggleWatcher"><Square v-if="props.watcher?.running" :size="14" /><Power v-else :size="14" />{{ props.watcher?.running ? t("停止监控") : t("开启监控") }}</button>
      </div>
    </section>
  </div>
</template>

