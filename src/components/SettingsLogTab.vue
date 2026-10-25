<script setup lang="ts">
import { computed } from "vue";
import { RefreshCw } from "lucide-vue-next";
import type { Activity } from "../api";

const props = defineProps<{
  logs: Activity[];
  logKind: string;
  refreshLogs: () => Promise<void>;
}>();

const emit = defineEmits<{
  (event: "update-log-kind", value: string): void;
}>();

const kind = computed({
  get: () => props.logKind,
  set: (value: string) => emit("update-log-kind", value),
});
</script>

<template>
  <section class="settings-section log-section settings-log-tab">
    <div class="section-head">
      <div><h2>活动日志</h2><p>记录扫描、索引、模型和服务状态。</p></div>
      <button class="secondary-button" @click="props.refreshLogs"><RefreshCw :size="14" />刷新</button>
    </div>
    <div class="log-toolbar">
      <select v-model="kind" @change="props.refreshLogs">
        <option value="">全部</option>
        <option value="scan">扫描</option>
        <option value="index">索引</option>
        <option value="model">模型</option>
        <option value="op">文件操作</option>
        <option value="watch">监控</option>
        <option value="server">局域网</option>
        <option value="error">错误</option>
      </select>
    </div>
    <div class="logs">
      <div v-for="entry in props.logs" :key="entry.id" class="log-row">
        <span class="log-time mono">{{ new Date(entry.ts * 1000).toLocaleTimeString() }}</span>
        <span class="log-kind" :class="entry.kind">{{ entry.kind }}</span>
        <span>{{ entry.msg }}</span>
      </div>
      <div v-if="!props.logs.length" class="empty-line">暂无日志</div>
    </div>
  </section>
</template>

