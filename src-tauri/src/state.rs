//! 应用全局状态。
use crate::ai::Embedder;
use crate::config::AppConfig;
use crate::db::Db;
use crate::index::Indexer;
use crate::search::SearchEngine;
use crate::server::LanServer;
use crate::vector::VectorStore;
use crate::watch::FileWatcher;
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

pub struct AppState {
    pub cfg: RwLock<AppConfig>,
    pub db: Arc<Db>,
    /// 文本向量空间（llama.cpp GGUF 512 维 / hash 256 维）
    pub vectors: RwLock<Arc<VectorStore>>,
    /// 图片/音频/视频向量空间（hash 特征 256 维，与文本空间独立）
    pub media_vectors: RwLock<Arc<VectorStore>>,
    /// 跨模态图片空间（中文 CLIP 512 维）：图片与视频关键帧，可用文字检索
    pub clip_vectors: RwLock<Arc<VectorStore>>,
    /// 跨模态音频空间（CLAP 512 维）：音频与视频音轨，可用文字检索
    pub audio_vectors: RwLock<Arc<VectorStore>>,
    pub embedder: RwLock<Arc<dyn Embedder>>,
    /// 媒体兜底编码器（始终是 hash）
    pub media_embedder: RwLock<Arc<dyn Embedder>>,
    /// 中文 CLIP（可选，缺模型时为 None）
    pub clip: RwLock<Option<Arc<crate::ai::clip::ClipModel>>>,
    /// CLAP 音频-文本对齐（可选）
    pub clap: RwLock<Option<Arc<crate::ai::clap::ClapModel>>>,
    pub indexer: RwLock<Option<Arc<Indexer>>>,
    pub watcher: Arc<FileWatcher>,
    pub server: Arc<LanServer>,
    pub data_dir: PathBuf,
    pub app_dir: PathBuf,
    pub scan_stats: Arc<crate::scan::ScanStatsShared>,
    pub scan_cancel: Arc<std::sync::atomic::AtomicBool>,
    pub scan_busy: Arc<std::sync::atomic::AtomicBool>,
    /// 工作区实时检索（未入库文件的现场抽取 + 缓存）
    pub workspace: crate::workspace::WorkspaceSearch,
}

pub struct EngineParts {
    pub db: Arc<Db>,
    pub vectors: Arc<VectorStore>,
    pub media: Arc<VectorStore>,
    pub clip: Arc<VectorStore>,
    pub audio: Arc<VectorStore>,
    pub cfg: crate::config::SearchConfig,
}

/// 加载跨模态模型（CLIP / CLAP）。
///
/// 两个模型都是可选的：没装就返回 None，检索时静默跳过跨模态段。
/// 加载失败也不能让主程序起不来——用户可能只想用文件名搜索。
pub fn load_cross_modal(
    cfg: &crate::config::ModelConfig,
    app_dir: &std::path::Path,
    data_dir: &std::path::Path,
) -> (
    Option<Arc<crate::ai::clip::ClipModel>>,
    Option<Arc<crate::ai::clap::ClapModel>>,
) {
    let threads = if cfg.n_threads > 0 { cfg.n_threads } else { 0 };
    let clip = crate::ai::clip::resolve_dir(cfg, app_dir, data_dir)
        .and_then(|d| crate::ai::clip::ClipModel::load(&d, threads))
        .map(Arc::new);
    let clap = crate::ai::clap::resolve_dir(cfg, app_dir, data_dir)
        .and_then(|d| crate::ai::clap::ClapModel::load(&d, threads))
        .map(Arc::new);
    (clip, clap)
}

impl AppState {
    pub fn new(app_dir: PathBuf) -> Result<Self, String> {
        // 注意：这里必须传 config.json 的完整路径，不是目录。
        // 传目录时 read_to_string 永远失败 → 整份配置静默回退默认值，
        // 表现为「配置文件里明明有模型路径，程序却当没配」。
        let cfg = AppConfig::load(&crate::config::config_path(Some(&app_dir)));
        let data_dir = crate::config::data_dir(Some(&app_dir));
        std::fs::create_dir_all(&data_dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
        let db = Arc::new(Db::open(&data_dir.join("index.db")).map_err(|e| format!("打开数据库失败: {e}"))?);
        db.migrate().map_err(|e| format!("数据库迁移失败: {e}"))?;
        db.sync_index_roots(&cfg.scan.roots, &cfg.scan.disabled_roots)
            .map_err(|e| format!("同步索引目录失败: {e}"))?;
        let vec_dir = data_dir.join("vectors");
        // 先用默认的 hash 后端建 embedder，向量库维度跟随后端实际输出，
        // 不能直接用 config 里的 embedding_dim（那可能是 llama 的 512 或旧的 768）。
        let embedder: Arc<dyn Embedder> = Arc::new(crate::ai::hash::HashEmbedder::new());
        let media_embedder: Arc<dyn Embedder> = Arc::new(crate::ai::hash::HashEmbedder::new());
        let dim = embedder.dim().max(16);
        let vectors = Arc::new(VectorStore::open(&vec_dir, dim).map_err(|e| format!("向量库打开失败: {e}"))?);
        // 媒体空间固定 256 维（HashEmbedder 的输出），与文本空间分开存
        let media_vectors = Arc::new(
            VectorStore::open(&data_dir.join("vectors_media"), 256)
                .map_err(|e| format!("媒体向量库打开失败: {e}"))?,
        );
        // 跨模态空间：CLIP 图片 512 维、CLAP 音频 512 维。
        // 与上面两个空间物理隔离——不同模型的向量放在一起会互相干扰。
        let clip_vectors = Arc::new(
            VectorStore::open(&data_dir.join("vectors_clip"), 512)
                .map_err(|e| format!("跨模态图片向量库打开失败: {e}"))?,
        );
        let audio_vectors = Arc::new(
            VectorStore::open(&data_dir.join("vectors_audio"), 512)
                .map_err(|e| format!("跨模态音频向量库打开失败: {e}"))?,
        );
        // Cross-modal encoders are loaded lazily with an explicit indexing
        // request. Opening the search UI must remain lightweight.
        let (clip, clap) = (None, None);
        Ok(Self {
            cfg: RwLock::new(cfg),
            db,
            vectors: RwLock::new(vectors),
            media_vectors: RwLock::new(media_vectors),
            clip_vectors: RwLock::new(clip_vectors),
            audio_vectors: RwLock::new(audio_vectors),
            embedder: RwLock::new(embedder),
            media_embedder: RwLock::new(media_embedder),
            clip: RwLock::new(clip),
            clap: RwLock::new(clap),
            indexer: RwLock::new(None),
            watcher: Arc::new(FileWatcher::new()),
            server: Arc::new(LanServer::new()),
            data_dir,
            app_dir,
            scan_stats: Arc::new(crate::scan::ScanStatsShared::new()),
            scan_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scan_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            workspace: crate::workspace::WorkspaceSearch::new(),
        })
    }

    pub fn search_engine(&self) -> EngineParts {
        let cfg = self.cfg.read().search.clone();
        EngineParts {
            db: Arc::clone(&self.db),
            vectors: Arc::clone(&self.vectors.read()),
            media: Arc::clone(&self.media_vectors.read()),
            clip: Arc::clone(&self.clip_vectors.read()),
            audio: Arc::clone(&self.audio_vectors.read()),
            cfg,
        }
    }

    pub fn search(&self, q: &mut crate::search::SearchQuery) -> Vec<crate::search::Hit> {
        // Own the scope string before mutating the query below. Borrowing it
        // directly from `q` would keep an immutable borrow alive across the
        // root-scope normalization and prevent the required mutable borrow.
        let scope = q.scope.as_deref().unwrap_or("legacy").to_owned();
        let scan = self.cfg.read().scan.clone();
        // `all` intentionally bypasses the enabled-root restriction. The
        // indexed database can safely search every row, while `indexed` keeps
        // the configured-root behaviour and `path` narrows to the selected
        // directory (including a directory that has not been indexed yet).
        if scope == "path" {
            if let Some(path) = q.scope_path.as_deref().map(crate::util::norm_path).filter(|p| !p.is_empty()) {
                q.roots = vec![path];
            }
        } else if scope == "indexed" || scope == "legacy" {
            crate::search::apply_enabled_root_scope(q, &scan.roots, &scan.disabled_roots);
        } else {
            q.roots.clear();
        }
        // 跨模态：把查询词编码成图片/声音空间里的向量。
        // 模型没装时静默跳过，用户仍然能靠文件名/标签搜到东西。
        if q.cross_modal == Some(true) && q.text.chars().count() >= 2 {
            if let Some(clip) = self.clip.read().as_ref() {
                if let Ok(v) = clip.text_vec(q.text.trim()) {
                    q.clip_query = Some(v);
                }
            }
            if let Some(clap) = self.clap.read().as_ref() {
                // CLAP 是英文模型：中文查询先映射成英文提示，否则几乎所有
                // 音频都落在同一片「弱相关」余弦区间里，排序等于随机。
                let prompt = crate::ai::clap::prompt_for(q.text.trim());
                if let Ok(v) = clap.text_vec(&prompt) {
                    q.audio_query = Some(v);
                }
            }
        }
        let p = self.search_engine();
        let engine = SearchEngine {
            db: &p.db,
            vectors: &p.vectors,
            media_vectors: &p.media,
            clip_vectors: &p.clip,
            audio_vectors: &p.audio,
            cfg: &p.cfg,
        };
        let mut hits = engine.search(q);

        // 工作区实时检索：把「工作区目录里有、索引库里没有」的文件也算进来。
        // 开关由前端传（默认开），只有首页搜索要，其他调用方（相似检索、服务端 API）不需要。
        if q.workspace.unwrap_or(true) && scope != "indexed" {
            let mut ws_cfg = self.cfg.read().workspace.clone();
            if scope == "legacy" && ws_cfg.dirs.is_empty() {
                ws_cfg.dirs = scan.roots.iter()
                    .filter(|root| !scan.disabled_roots.iter().any(|disabled| {
                        crate::util::norm_path(disabled).eq_ignore_ascii_case(&crate::util::norm_path(root))
                    }))
                    .cloned()
                    .collect();
            }
            if scope == "path" {
                if let Some(path) = q.scope_path.as_deref().map(crate::util::norm_path).filter(|p| !p.is_empty()) {
                    ws_cfg.dirs = vec![path];
                }
            } else if scope == "all" {
                // Full-disk scope also covers files that have not been
                // indexed yet. Keep the existing bounded workspace extractor
                // limits so selecting "all" cannot recursively read an
                // unbounded drive in the UI thread.
                ws_cfg.dirs = available_drive_roots();
            }
            if ws_cfg.enabled && !ws_cfg.dirs.is_empty() {
                let extra = self.workspace.search(&self.db, &ws_cfg, q, &hits);
                if !extra.is_empty() {
                    merge_workspace_hits(&mut hits, extra, q);
                }
            }
        }
        hits
    }

    pub fn rebuild_indexer(&self) -> Arc<Indexer> {
        let app_cfg = self.cfg.read().clone();
        let cfg = app_cfg.index.clone();
        let embedder = Arc::clone(&self.embedder.read());
        let vecs = Arc::clone(&self.vectors.read());
        let media_vecs = Arc::clone(&self.media_vectors.read());
        let clip_vecs = Arc::clone(&self.clip_vectors.read());
        let audio_vecs = Arc::clone(&self.audio_vectors.read());
        let configured_batch_size = cfg.batch_size;
        let idx = Arc::new(Indexer::new(
            Arc::clone(&self.db),
            vecs,
            media_vecs,
            clip_vecs,
            audio_vecs,
            embedder,
            Arc::clone(&self.media_embedder.read()),
            cfg,
            self.data_dir.join("thumbs"),
        ));
        idx.set_batch_size(crate::ai::effective_batch_size(&app_cfg.model, configured_batch_size));
        idx.set_worker_threads(crate::ai::effective_parallel_threads(&app_cfg.model));
        {
            idx.set_cross_modal(self.clip.read().clone(), self.clap.read().clone());
        }
        // Stop the previous worker before swapping in a new embedder.  The
        // worker owns an Arc<Indexer>, so simply replacing the slot would
        // leave it running with the old model (and potentially its GPU
        // allocation) until process exit.
        if let Some(previous) = self.indexer.write().take() {
            previous.stop();
        }
        *self.indexer.write() = Some(Arc::clone(&idx));
        idx.start();
        idx
    }

    pub fn indexer(&self) -> Option<Arc<Indexer>> {
        self.indexer.read().clone()
    }

}

fn available_drive_roots() -> Vec<String> {
    #[cfg(windows)]
    {
        ('A'..='Z')
            .map(|letter| format!("{letter}:\\"))
            .filter(|root| std::path::Path::new(root).is_dir())
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec!["/".into()]
    }
}

/// 把工作区命中并入索引结果：按分数降序，保留原来的分页语义。
///
/// 工作区命中没有真实 id（负数），排序键用分数而不是 id，避免两批结果
/// 混在一起时顺序错乱。offset 已经在 workspace 侧过滤过（只处理首页），
/// 这里只需要把总数截断到 limit。
fn merge_workspace_hits(
    hits: &mut Vec<crate::search::Hit>,
    extra: Vec<crate::search::Hit>,
    q: &crate::search::SearchQuery,
) {
    hits.extend(extra);
    let sort = q.sort.as_deref().unwrap_or("relevance");
    if sort == "relevance" || sort.is_empty() {
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    }
    let limit = q.limit.unwrap_or(50_000).max(1);
    let offset = q.offset.unwrap_or(0);
    if hits.len() > offset + limit {
        hits.truncate(offset + limit);
    }
}

