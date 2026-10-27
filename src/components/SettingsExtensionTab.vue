<script setup lang="ts">
import { onMounted, ref } from "vue";
import { Download, FolderOpen, RefreshCw } from "lucide-vue-next";
import { api, type ExtensionStatus } from "../api";
import { toast } from "../store";
import { t } from "../i18n";

const status = ref<ExtensionStatus | null>(null);
const loading = ref(false);
const installing = ref("");

async function refresh() {
  loading.value = true;
  try {
    status.value = await api.extensionStatus();
  } catch (error) {
    toast(`${t("读取扩展状态失败：")}${error}`, "error");
  } finally {
    loading.value = false;
  }
}

async function install(kind: "onnxruntime" | "python") {
  installing.value = kind;
  try {
    status.value = await api.extensionInstall(kind);
    toast(kind === "onnxruntime" ? t("ONNX Runtime 已安装到项目目录") : t("Python sidecar 运行时已安装到项目目录"), "ok");
  } catch (error) {
    toast(`${t("扩展安装失败：")}${error}`, "error");
  } finally {
    installing.value = "";
  }
}

async function openRoot() {
  if (status.value?.root) await api.openPath(status.value.root);
}

onMounted(refresh);
</script>

<template>
  <section class="settings-section extension-section">
    <div class="section-head">
      <div>
        <h2>{{ t("可选扩展") }}</h2>
        <p>{{ t("内置模型支持保持轻量；按需安装兼容性扩展，文件保存在项目目录。") }}</p>
      </div>
      <div class="extension-actions">
        <button class="secondary-button" :disabled="loading || !!installing" @click="refresh"><RefreshCw :size="14" />{{ t("刷新") }}</button>
        <button class="secondary-button" :disabled="!status?.root" @click="openRoot"><FolderOpen :size="14" />{{ t("打开扩展目录") }}</button>
      </div>
    </div>

    <div class="extension-grid">
      <article class="extension-card">
        <div class="extension-card-head"><strong>ONNX Runtime</strong><span class="extension-state" :class="{ on: status?.onnxruntime }">{{ status?.onnxruntime ? t("已安装") : t("未安装") }}</span></div>
        <p>{{ t("用于运行标准 ONNX 模型，例如 CLIP、CLAP。不能直接读取 safetensors。") }}</p>
        <div class="extension-path mono">{{ status?.onnxruntime_path || t("项目/.tools/onnxruntime/onnxruntime.dll") }}</div>
        <button class="primary" :disabled="!!installing" @click="install('onnxruntime')"><Download :size="14" />{{ installing === "onnxruntime" ? t("安装中…") : status?.onnxruntime ? t("重新检查") : t("下载并安装") }}</button>
      </article>

      <article class="extension-card">
        <div class="extension-card-head"><strong>Python sidecar</strong><span class="extension-state" :class="{ on: status?.python }">{{ status?.python ? t("已安装") : t("未安装") }}</span></div>
        <p>{{ t("可选的 Python 运行时入口，用于尚未被内置模型支持的格式；不会自动安装 PyTorch。") }}</p>
        <div class="extension-path mono">{{ status?.python_path || t("项目/.tools/python/python.exe") }}</div>
        <button class="primary" :disabled="!!installing" @click="install('python')"><Download :size="14" />{{ installing === "python" ? t("安装中…") : status?.python ? t("重新检查") : t("下载运行时") }}</button>
      </article>
    </div>
    <p class="extension-note">{{ t("扩展不会写入系统目录，也不会删除已有文件。Python 运行时本身不包含模型和 PyTorch，具体模型适配器可按需另行安装。") }}</p>
  </section>
</template>

<style scoped>
.extension-actions { display: flex; flex-wrap: wrap; gap: 7px; }
.extension-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; }
.extension-card { min-width: 0; padding: 13px; border: 1px solid #111; background: #fff; }
.extension-card-head { display: flex; justify-content: space-between; gap: 10px; align-items: center; }
.extension-card p, .extension-note { color: #555; font-size: 11px; line-height: 1.55; }
.extension-card p { min-height: 34px; margin: 8px 0; }
.extension-state { color: #777; font-size: 11px; }
.extension-state.on { color: #111; font-weight: 600; }
.extension-path { margin: 8px 0 11px; overflow: hidden; color: #555; font-size: 10px; text-overflow: ellipsis; white-space: nowrap; }
.extension-card button { display: inline-flex; align-items: center; gap: 6px; }
.extension-note { margin: 12px 0 0; }
@media (max-width: 720px) { .extension-grid { grid-template-columns: 1fr; } .extension-actions { margin-top: 8px; } }
</style>
