<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { FileText } from "lucide-vue-next";
import { convertFileSrc } from "@tauri-apps/api/core";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { api } from "../api";

const props = defineProps<{ fileId: number; path: string }>();
const emit = defineEmits<{ (event: "ready"): void }>();

let pdfJsModule: Promise<typeof import("pdfjs-dist")> | undefined;
async function loadPdfJs() {
  pdfJsModule ??= import("pdfjs-dist");
  const pdfjs = await pdfJsModule;
  pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
  return pdfjs;
}

let activeRenders = 0;
const renderWaiters: Array<() => void> = [];
async function acquireRenderSlot() {
  if (activeRenders >= 2) await new Promise<void>((resolve) => renderWaiters.push(resolve));
  activeRenders++;
}
function releaseRenderSlot() {
  activeRenders--;
  renderWaiters.shift()?.();
}

const host = ref<HTMLElement | null>(null);
const canvas = ref<HTMLCanvasElement | null>(null);
const rendered = ref(false);
const failed = ref(false);
let disposed = false;
let observer: IntersectionObserver | undefined;
let loadingTask: { destroy: () => Promise<void> } | undefined;

async function renderCover() {
  await acquireRenderSlot();
  try {
    if (disposed || !canvas.value) return;
    const canvasElement = canvas.value;
    const { getDocument } = await loadPdfJs();
    if (disposed) return;
    const url = convertFileSrc(props.path);
    // WebView2 can reject a module worker for local asset URLs. Try the
    // normal worker path first, then render in the current WebView thread so
    // one unsupported worker does not leave every PDF card blank.
    let loading = getDocument({ url, isEvalSupported: false });
    loadingTask = loading;
    let pdf;
    try {
      pdf = await loading.promise;
    } catch (workerError) {
      try { await loading.destroy(); } catch { /* already failed */ }
      if (disposed) return;
      loading = getDocument({ url, disableWorker: true, useWorkerFetch: false, isEvalSupported: false });
      loadingTask = loading;
      pdf = await loading.promise;
    }
    if (disposed) return;
    const page = await pdf.getPage(1);
    const base = page.getViewport({ scale: 1 });
    // Fill the card by width for both portrait and landscape pages. A tall
    // portrait page may be cropped vertically, but its text stays readable.
    const displayWidth = Math.max(1, host.value?.clientWidth || 320);
    const scale = displayWidth / base.width;
    const outputScale = Math.min(window.devicePixelRatio || 1, 2);
    const viewport = page.getViewport({ scale: scale * outputScale });
    canvasElement.width = Math.max(1, Math.floor(viewport.width));
    canvasElement.height = Math.max(1, Math.floor(viewport.height));
    canvasElement.style.width = `${displayWidth}px`;
    canvasElement.style.height = `${Math.ceil(viewport.height / outputScale)}px`;
    await page.render({ canvas: canvasElement, canvasContext: canvasElement.getContext("2d")!, viewport }).promise;
    if (disposed) return;
    rendered.value = true;
    try {
      const dataUrl = canvasElement.toDataURL("image/jpeg", 0.84);
      await api.savePdfThumbnail(props.fileId, props.path, dataUrl);
      emit("ready");
    } catch {
      // Keep the rendered canvas visible when the persistent cache cannot be written.
    }
    page.cleanup();
    await pdf.destroy();
  } catch {
    if (!disposed) failed.value = true;
  } finally {
    releaseRenderSlot();
  }
}

onMounted(() => {
  if (!host.value || !("IntersectionObserver" in window)) {
    void renderCover();
    return;
  }
  observer = new IntersectionObserver((entries) => {
    if (!entries.some((entry) => entry.isIntersecting)) return;
    observer?.disconnect();
    void renderCover();
  }, { rootMargin: "180px" });
  observer.observe(host.value);
});

onBeforeUnmount(() => {
  disposed = true;
  observer?.disconnect();
  if (loadingTask) void loadingTask.destroy();
});
</script>

<template>
  <div ref="host" class="pdf-thumb">
    <canvas v-show="rendered" ref="canvas"></canvas>
    <FileText v-if="!rendered && failed" :size="30" stroke-width="1.5" />
    <span v-else-if="!rendered" class="pdf-loading"></span>
  </div>
</template>

<style scoped>
.pdf-thumb { position: absolute; inset: 0; display: flex; align-items: flex-start; justify-content: flex-start; overflow: hidden; background: #fff; }
canvas { display: block; width: 100%; max-width: none; height: auto; }
.pdf-loading { width: 18px; height: 18px; border: 2px solid #bbb; border-top-color: #111; border-radius: 50%; animation: turn .8s linear infinite; }
@keyframes turn { to { transform: rotate(360deg); } }
</style>
