// 与 Rust 后端通信的唯一入口。
// 所有命令名必须与 src-tauri/src/cmd.rs 里的 #[tauri::command] 一致。
import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------- 类型

export interface AppInfo {
  name: string;
  version: string;
  display_version: string;
  provider: string;
  files: number;
}

export interface Hit {
  id: number;
  path: string;
  dir: string;
  name: string;
  ext: string;
  kind: string;
  kind_label: string;
  size: number;
  size_label: string;
  mtime: number;
  mtime_label: string;
  category: string;
  /** 是否有缩略图（文件名固定为 `<id>.jpg`） */
  thumb: boolean;
  score: number;
  reason: string;
  snippet: string;
  /** 视觉标签（图片/视频自动生成）：蓝天、草地、雪景、夕阳… */
  tags?: string | null;
}

export interface SearchQuery {
  text: string;
  kinds: string[];
  exts: string[];
  roots: string[];
  categories: string[];
  size_min?: number | null;
  size_max?: number | null;
  mtime_after?: number | null;
  mtime_before?: number | null;
  limit?: number;
  offset?: number;
  sort?: string;
  desc?: boolean;
  use_fts?: boolean;
  use_pinyin?: boolean;
  use_fuzzy?: boolean;
  use_semantic?: boolean;
  /** 用文字描述找图片/声音（需要装了 CLIP/CLAP 模型），三态：不传=跟随界面开关 */
  cross_modal?: boolean | null;
}

export interface StatsOut {
  files: number;
  bytes: number;
  bytes_label: string;
  kinds: { kind: string; label: string; count: number; bytes: number }[];
  text_ok: number;
  embed_ok: number;
  missing: number;
  db_bytes: number;
  vector_count: number;
  dim: number;
}

export interface ScanProgress {
  running: boolean;
  dirs: number;
  files: number;
  added: number;
  updated: number;
  skipped: number;
  errors: number;
  missing: number;
  bytes: number;
  done: boolean;
  cancelled: boolean;
  current: string;
  seconds: number;
}

export interface IndexStats {
  queued: number;
  running: boolean;
  text_done: number;
  text_fail: number;
  emb_done: number;
  emb_fail: number;
  thumb_done: number;
  current: string;
}

export interface ModelInfo {
  backend: string;
  model: string;
  dim: number;
  device: string;
  modalities: string[];
  ready: boolean;
  detail: string;
  /** 跨模态图片模型（中文 CLIP）状态，空串 = 未安装 */
  clip: string;
  /** 跨模态音频模型（CLAP）状态，空串 = 未安装 */
  clap: string;
}

/** 模型目录里的一个 GGUF 文件 */
export interface ModelEntry {
  path: string;
  /** 展示名：GGUF 头里的 general.name，读不到就是文件名 */
  name: string;
  file: string;
  size: number;
  size_label: string;
  mtime: number;
  mtime_label: string;
  /** embedding | chat | projector | unknown */
  kind: string;
  kind_label: string;
  /** 是否当前正在使用 */
  active: boolean;
}

export interface TreeNode {
  name: string;
  path: string;
  size: number;
  count: number;
  kinds: Record<string, number>;
  children: TreeNode[];
}

export interface WatchStatus {
  running: boolean;
  roots: number;
  events: number;
}

export interface ServerStatus {
  running: boolean;
  url: string;
}

export interface Activity {
  id: number;
  ts: number;
  kind: string;
  msg: string;
}

export interface Rule {
  id: number;
  name: string;
  field: string;
  op: string;
  value: string;
  category: string;
  priority: number;
  enabled: boolean;
}

export interface AppConfig {
  scan: any;
  index: any;
  model: any;
  search: any;
  watch: any;
  ui: any;
  server: any;
}

/** 部分配置（深合并用）：后端会把传入的字段并入当前配置，其余保持不变。 */
export type PartialConfig = DeepPartial<AppConfig>;
export type DeepPartial<T> = T extends object
  ? { [K in keyof T]?: DeepPartial<T[K]> }
  : T;

// ---------------------------------------------------------------- 调用

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  getConfig: () => invoke<AppConfig>("get_config"),
  saveConfig: (cfg: PartialConfig) => invoke<void>("save_config", { cfg }),

  search: (q: SearchQuery) => invoke<Hit[]>("search", { q }),
  searchSemantic: (q: SearchQuery) => invoke<Hit[]>("search_semantic", { q }),
  similar: (fileId: number, kind?: string, top?: number) =>
    invoke<Hit[]>("similar", { fileId, kind, top }),
  stats: () => invoke<StatsOut>("stats"),
  recent: (limit = 50) => invoke<Hit[]>("recent", { limit }),

  scanStart: (roots?: string[]) => invoke<void>("scan_start", { roots }),
  scanCancel: () => invoke<void>("scan_cancel"),
  scanProgress: () => invoke<ScanProgress>("scan_progress"),

  indexStart: (what?: string, force?: boolean) =>
    invoke<number>("index_start", { what, force }),
  indexStats: () => invoke<IndexStats>("index_stats"),

  modelInfo: () => invoke<ModelInfo>("model_info"),
  modelLoad: (backend: string, path?: string) =>
    invoke<ModelInfo>("model_load", { backend, path }),
  modelList: () => invoke<ModelEntry[]>("model_list"),
  modelDir: () => invoke<string>("model_dir_path"),
  modelDelete: (path: string) => invoke<void>("model_delete", { path }),
  modelImport: (src: string) => invoke<string>("model_import", { src }),

  openPath: (path: string) => invoke<void>("open_path", { path }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),

  opExport: (paths: string[], out: string, fmt?: string) =>
    invoke<number>("op_export", { paths, out, fmt }),

  rulesList: () => invoke<Rule[]>("rules_list"),
  rulesSave: (rules: Rule[]) => invoke<void>("rules_save", { rules }),
  rulesApply: () => invoke<number>("rules_apply"),

  activityList: (kind?: string, limit = 300) =>
    invoke<Activity[]>("activity_list", { kind, limit }),

  treeData: (root?: string, depth = 3) =>
    invoke<TreeNode>("tree_data", { root, depth }),

  watchStart: () => invoke<number>("watch_start"),
  watchStop: () => invoke<void>("watch_stop"),
  watchStatus: () => invoke<WatchStatus>("watch_status"),

  serverStart: () => invoke<string>("server_start"),
  serverStop: () => invoke<void>("server_stop"),
  serverStatus: () => invoke<ServerStatus>("server_status"),

  dbVacuum: () => invoke<void>("db_vacuum"),
  dbClear: () => invoke<void>("db_clear"),
  dataDir: () => invoke<string>("data_dir_path"),
};

/** Tauri 未注入时（比如直接用浏览器打开 vite dev）给一个明确的错误。 */
export function isTauri(): boolean {
  return typeof (window as any).__TAURI_INTERNALS__ !== "undefined";
}
