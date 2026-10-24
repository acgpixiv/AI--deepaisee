// 全局共享状态：应用信息、统计、模型、提示条。
import { reactive } from "vue";
import { api, type AppInfo, type ModelInfo, type StatsOut } from "./api";

export interface Toast {
  id: number;
  text: string;
  level: "info" | "ok" | "warn" | "error";
}

export const store = reactive({
  info: null as AppInfo | null,
  stats: null as StatsOut | null,
  model: null as ModelInfo | null,
  toasts: [] as Toast[],
  indexBusy: false,
  scanBusy: false,
});

let toastSeq = 0;

export function toast(text: string, level: Toast["level"] = "info", ms = 3600) {
  const id = ++toastSeq;
  store.toasts.push({ id, text, level });
  if (store.toasts.length > 5) store.toasts.shift();
  window.setTimeout(() => {
    const i = store.toasts.findIndex((t) => t.id === id);
    if (i >= 0) store.toasts.splice(i, 1);
  }, ms);
}

export async function refreshStats() {
  try {
    store.stats = await api.stats();
  } catch {
    /* 后端未就绪时静默 */
  }
}

export async function refreshModel() {
  try {
    store.model = await api.modelInfo();
  } catch {
    /* ignore */
  }
}

export async function refreshInfo() {
  try {
    store.info = await api.appInfo();
  } catch {
    /* ignore */
  }
}

/** 人类可读的文件大小 */
export function humanSize(n: number): string {
  if (!n || n < 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

export function fmtTime(ts: number): string {
  if (!ts) return "";
  const d = new Date(ts * 1000);
  const p = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 类型 → 颜色（键与后端 util::ext_kind 返回值一致） */
export const KIND_COLORS: Record<string, string> = {
  image: "#38bdf8",
  video: "#a78bfa",
  audio: "#34d399",
  document: "#f59e0b",
  text: "#94a3b8",
  archive: "#fb923c",
  app: "#f472b6",
  other: "#64748b",
};

export function kindColor(k: string): string {
  return KIND_COLORS[k] || KIND_COLORS.other;
}
