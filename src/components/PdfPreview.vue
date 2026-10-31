<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

const props = defineProps<{ path: string }>();
const emit = defineEmits<{
  (event: "page-count", count: number): void;
  (event: "page-change", page: number): void;
}>();

let pdfJsModule: Promise<typeof import("pdfjs-dist")> | undefined;
async function loadPdfJs() {
  pdfJsModule ??= import("pdfjs-dist");
  const pdfjs = await pdfJsModule;
  pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
  return pdfjs;
}

const host = ref<HTMLElement | null>(null);
const canvas = ref<HTMLCanvasElement | null>(null);
const loading = ref(true);
const failed = ref(false);
const page = ref(1);
const pageCount = ref(0);
let disposed = false;
let pdf: any;
let loadingTask: { destroy: () => Promise<void> } | undefined;
let renderTask: { cancel: () => void; promise: Promise<void> } | undefined;
let wheelLock = false;

async function renderPage() {
  if (disposed || !pdf || !canvas.value || !host.value) return;
  renderTask?.cancel();
  const pdfPage = await pdf.getPage(page.value);
  if (disposed) return;
  const base = pdfPage.getViewport({ scale: 1 });
  const width = Math.max(1, host.value.clientWidth || 430);
  const outputScale = Math.min(window.devicePixelRatio || 1, 2);
  const viewport = pdfPage.getViewport({ scale: (width / base.width) * outputScale });
  canvas.value.width = Math.max(1, Math.floor(viewport.width));
  canvas.value.height = Math.max(1, Math.floor(viewport.height));
  canvas.value.style.width = `${width}px`;
  canvas.value.style.height = `${Math.ceil(viewport.height / outputScale)}px`;
  const context = canvas.value.getContext("2d");
  if (!context) return;
  renderTask = pdfPage.render({ canvas: canvas.value, canvasContext: context, viewport });
  try { await renderTask.promise; } catch (error) {
    if ((error as Error)?.name !== "RenderingCancelledException") throw error;
  }
  pdfPage.cleanup();
  loading.value = false;
  emit("page-change", page.value);
}

async function openPdf() {
  loading.value = true;
  failed.value = false;
  const { getDocument } = await loadPdfJs();
  const url = convertFileSrc(props.path);
  loadingTask = getDocument({ url, isEvalSupported: false });
  try {
    pdf = await loadingTask.promise;
  } catch {
    try { await loadingTask.destroy(); } catch { /* already failed */ }
    if (disposed) return;
    loadingTask = getDocument({ url, disableWorker: true, useWorkerFetch: false, isEvalSupported: false });
    pdf = await loadingTask.promise;
  }
  if (disposed) return;
  pageCount.value = pdf.numPages || 1;
  emit("page-count", pageCount.value);
  await renderPage();
}

function movePage(delta: number) {
  if (!pageCount.value || wheelLock) return;
  const next = Math.max(1, Math.min(pageCount.value, page.value + delta));
  if (next === page.value) return;
  page.value = next;
  wheelLock = true;
  window.setTimeout(() => { wheelLock = false; }, 120);
  void renderPage().catch(() => { failed.value = true; });
}

function onWheel(event: WheelEvent) {
  event.preventDefault();
  if (Math.abs(event.deltaY) >= 2) movePage(event.deltaY > 0 ? 1 : -1);
}

onMounted(() => { void openPdf().catch(() => { loading.value = false; failed.value = true; }); });
watch(() => props.path, () => {
  page.value = 1;
  pageCount.value = 0;
  void openPdf().catch(() => { loading.value = false; failed.value = true; });
});
onBeforeUnmount(() => {
  disposed = true;
  renderTask?.cancel();
  if (loadingTask) void loadingTask.destroy();
  if (pdf) void pdf.destroy();
});
</script>

<template>
  <div ref="host" class="pdf-preview" @wheel="onWheel">
    <canvas v-show="!failed" ref="canvas"></canvas>
    <span v-if="loading && !failed" class="loading"></span>
    <span v-if="failed" class="failed">PDF</span>
    <span v-if="pageCount" class="page-indicator">{{ page }} / {{ pageCount }}</span>
  </div>
</template>

<style scoped>
.pdf-preview { position: relative; width: 100%; height: 100%; overflow: hidden; background: #fff; }
canvas { display: block; width: 100%; max-width: 100%; height: auto; background: #fff; }
.loading { position: absolute; left: 50%; top: 50%; width: 18px; height: 18px; margin: -9px; border: 2px solid #bbb; border-top-color: #111; border-radius: 50%; animation: spin .8s linear infinite; }
.failed { position: absolute; inset: 0; display: grid; place-items: center; color: #555; font-size: 12px; }
.page-indicator { position: absolute; left: 50%; bottom: 12px; transform: translateX(-50%); padding: 4px 10px; border-radius: 2px; background: rgba(0,0,0,.62); color: #fff; font: 12px Consolas, monospace; pointer-events: none; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
