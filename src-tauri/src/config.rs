//! 配置管理：读写 data/config.json，提供默认值。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
// 缺字段时用 Default 补齐，避免用户手改配置时整份失效
#[serde(default)]
pub struct ScanConfig {
    pub roots: Vec<String>,
    /// Roots kept in the directory list but excluded from indexing.
    #[serde(default)]
    pub disabled_roots: Vec<String>,
    pub exclude_dirs: Vec<String>,
    pub follow_links: bool,
    pub max_size_mb: u64,
    pub min_size_kb: u64,
    pub images: bool,
    pub videos: bool,
    pub audios: bool,
    /// Whether PDF files are included. Kept separate because PDFs may contain
    /// both extracted text and embedded images.
    #[serde(default = "default_true")]
    pub pdf: bool,
    pub documents: bool,
    pub archives: bool,
    pub others: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            roots: vec![],
            disabled_roots: vec![],
            exclude_dirs: vec![
                "$RECYCLE.BIN".into(),
                "System Volume Information".into(),
                "node_modules".into(),
                ".git".into(),
                "AppData".into(),
                "Windows".into(),
                "Program Files".into(),
                "Program Files (x86)".into(),
                "ProgramData".into(),
            ],
            follow_links: false,
            max_size_mb: 4096,
            min_size_kb: 0,
            images: true,
            videos: true,
            audios: true,
            pdf: true,
            documents: true,
            archives: true,
            others: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// 缺字段时用 Default 补齐，避免用户手改配置时整份失效
#[serde(default)]
pub struct IndexConfig {
    pub max_text_chars: usize,
    pub auto_fulltext: bool,
    pub auto_embed: bool,
    pub image_max_side: u32,
    pub video_frames: u32,
    /// 视频抽帧的最小时间间隔（秒）。间隔越小越密，下限 4 秒，
    /// 避免长视频抽出成千上万张几乎相同的画面。
    pub video_frame_interval: f32,
    /// 单个视频最多保留多少帧（安全上限，超过时自动放稀采样）。
    pub video_max_frames: u32,
    /// 相邻两帧的图像指纹（dHash）汉明距离小于该值即视为重复画面丢弃。
    pub video_dedup_distance: u32,
    /// 是否对视频音轨做语义（CLAP）索引，用于「用文字描述找声音」。
    pub video_index_audio: bool,
    pub audio_seconds: f32,
    pub batch_size: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            max_text_chars: 200_000,
            auto_fulltext: true,
            auto_embed: true,
            image_max_side: 448,
            video_frames: 16,
            video_frame_interval: 4.0,
            video_max_frames: 64,
            video_dedup_distance: 6,
            video_index_audio: true,
            audio_seconds: 8.0,
            batch_size: 8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// 缺字段时用 Default 补齐，避免用户手改配置时整份失效
#[serde(default)]
pub struct ModelConfig {
    /// GGUF 嵌入模型路径
    pub embed_model: String,
    /// GGUF 生成模型路径（可选，用于 RAG 问答）
    pub chat_model: String,
    pub n_threads: i32,
    /// Selected accelerator device id/name. `auto` chooses the largest available GPU.
    #[serde(default = "default_gpu_device")]
    pub gpu_device: String,
    pub context_size: u32,
    pub auto_load: bool,
    pub embedding_dim: usize,
    /// 跨模态模型（中文 CLIP）目录，空 = 自动在 data/models/clip 与 exe/models/clip 找
    pub clip_dir: String,
    /// 关掉后不做「用文字描述找图片」，省内存
    pub clip_enabled: bool,
    /// 跨模态音频模型（CLAP）目录，空 = 自动在 data/models/clap 与 exe/models/clap 找
    pub clap_dir: String,
    /// 关掉后不做「用文字描述找声音」
    pub clap_enabled: bool,
    /// Runtime model concurrency policy. Compatible replicas are distributed
    /// across accelerators subject to memory limits and the selected cap.
    #[serde(default = "default_parallel_mode")]
    pub parallel_mode: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            embed_model: String::new(),
            chat_model: String::new(),
            n_threads: 0,
            // A llama model load requires a selected accelerator and always
            // offloads all model layers; it never falls back to CPU inference.
            gpu_device: "auto".into(),
            context_size: 4096,
            auto_load: true,
            embedding_dim: 768,
            clip_dir: String::new(),
            clip_enabled: true,
            clap_dir: String::new(),
            clap_enabled: true,
            parallel_mode: "auto".into(),
        }
    }
}

fn default_gpu_device() -> String {
    "auto".into()
}

fn default_parallel_mode() -> String {
    "auto".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// 缺字段时用 Default 补齐，避免用户手改配置时整份失效
#[serde(default)]
pub struct SearchConfig {
    pub page_size: usize,
    pub debounce_ms: u64,
    pub use_pinyin: bool,
    pub use_fuzzy: bool,
    pub use_semantic: bool,
    pub use_fulltext: bool,
    pub semantic_weight: f32,
    pub semantic_floor: f32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            // Keep the default useful for large local indexes. Individual
            // callers can still request a smaller page explicitly.
            page_size: 1000,
            debounce_ms: 200,
            use_pinyin: true,
            use_fuzzy: true,
            use_semantic: true,
            use_fulltext: true,
            semantic_weight: 0.55,
            semantic_floor: 0.45,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// 缺字段时用 Default 补齐，避免用户手改配置时整份失效
#[serde(default)]
pub struct WatchConfig {
    pub enabled: bool,
    pub auto_index: bool,
    pub debounce_secs: u64,
    #[serde(default)]
    pub roots: Vec<String>,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self { enabled: false, auto_index: true, debounce_secs: 2, roots: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// 缺字段时用 Default 补齐，避免用户手改配置时整份失效
#[serde(default)]
pub struct UiConfig {
    pub font_size: u32,
    /// light | dark | system
    pub theme: String,
    /// zh | en
    pub language: String,
    pub remember_window: bool,
    /// 开机自启（写入当前用户的 Run 注册表项）
    pub autostart: bool,
    /// 启动时自动扫描索引根目录的增量变化
    pub auto_scan_on_start: bool,
    /// 启动扫描发现变化时是否在底部给出提示条
    pub notify_file_changes: bool,
    /// 默认搜索模式：fulltext | semantic | hybrid
    pub default_search_mode: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            font_size: 14,
            theme: "system".into(),
            language: "zh".into(),
            remember_window: true,
            autostart: false,
            auto_scan_on_start: true,
            notify_file_changes: true,
            default_search_mode: "hybrid".into(),
        }
    }
}

/// 工作区：主页可以直接指定一个目录，搜索时把该目录里**尚未入库**的文件
/// 实时抽取正文一起参与检索，不影响索引数据库。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    /// 工作区根目录列表，空表示未启用（可同时指定多个目录）
    pub dirs: Vec<String>,
    pub enabled: bool,
    /// 是否对未入库文件做实时正文抽取（否则只按文件名匹配）
    pub live_extract: bool,
    /// 实时抽取时跳过的超大文件（MB）
    pub max_file_mb: u64,
    /// 递归深度上限，0 = 不递归
    pub depth: u32,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self { dirs: vec![], enabled: true, live_extract: true, max_file_mb: 64, depth: 4 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// 缺字段时用 Default 补齐，避免用户手改配置时整份失效
#[serde(default)]
pub struct ServerConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    pub token: String,
    #[serde(default)]
    pub allow_write: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { enabled: false, bind: "0.0.0.0".into(), port: 8770, token: String::new(), allow_write: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
// 允许用户手写只含部分字段的 config.json（缺的段用默认值），
// 否则少一个字段整份配置都会静默回退到默认值。
#[serde(default)]
pub struct AppConfig {
    pub scan: ScanConfig,
    pub index: IndexConfig,
    pub model: ModelConfig,
    pub search: SearchConfig,
    pub watch: WatchConfig,
    pub ui: UiConfig,
    pub workspace: WorkspaceConfig,
    pub server: ServerConfig,
}

impl AppConfig {
    /// 从磁盘加载；文件不存在或损坏时回退默认值。
    pub fn load(path: &Path) -> Self {
        let config: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        config
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, text)
    }
}

/// 数据目录解析：优先 exe 同级 data/（便携模式）；如果该位置不可写
/// （典型场景：NSIS 安装到 Program Files），回退到用户可写目录
/// %LOCALAPPDATA%\deepaisee\data，保证索引库/配置/模型缓存总能落盘。
static DATA_DIR_CACHE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn data_dir(app_dir: Option<&Path>) -> PathBuf {
    DATA_DIR_CACHE.get_or_init(|| resolve_data_dir(app_dir)).clone()
}

fn resolve_data_dir(app_dir: Option<&Path>) -> PathBuf {
    let portable = match app_dir {
        Some(d) => d.join("data"),
        None => PathBuf::from("data"),
    };
    if portable.is_dir() {
        return portable;
    }
    // 目录还不存在：试着建一次并写探针，判断这个位置是否可写。
    if std::fs::create_dir_all(&portable).is_ok() {
        let probe = portable.join(".write-probe");
        let ok = std::fs::write(&probe, b"ok").is_ok();
        let _ = std::fs::remove_file(&probe);
        if ok {
            return portable;
        }
    }
    let fallback = std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("deepaisee").join("data"))
        .or_else(|| std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("deepaisee").join("data")))
        .unwrap_or_else(|| std::env::temp_dir().join("deepaisee").join("data"));
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

pub fn config_path(app_dir: Option<&Path>) -> PathBuf {
    data_dir(app_dir).join("config.json")
}

/// 插件扩展预留目录：exe 同级 plugins/。安装版卸载时会保留该目录，
/// 后续插件只需把目录（或 zip）放进去即可被发现。
pub fn plugins_dir(app_dir: &Path) -> PathBuf {
    let dir = app_dir.join("plugins");
    let _ = std::fs::create_dir_all(&dir);
    dir
}