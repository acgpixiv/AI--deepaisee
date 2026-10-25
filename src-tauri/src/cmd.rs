//! Tauri 命令层：前端通过 invoke 调用。
use crate::config::AppConfig;
use crate::db::{Activity, FileRow, Rule};
use crate::fileops;
use crate::search::{Hit, SearchQuery};
use crate::state::AppState;
use crate::util::human_size;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::State;

#[derive(Serialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub display_version: String,
    pub provider: String,
    pub files: i64,
    pub data_dir: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, Arc<AppState>>) -> AppInfo {
    AppInfo {
        name: "deepaisee".into(),
        version: crate::APP_VERSION.into(),
        display_version: crate::APP_DISPLAY_VERSION.into(),
        provider: state.embedder.read().info().backend,
        files: state.db.count().unwrap_or(0),
        data_dir: state.data_dir.to_string_lossy().to_string(),
    }
}

#[tauri::command]
pub fn get_config(state: State<'_, Arc<AppState>>) -> AppConfig {
    state.cfg.read().clone()
}

#[tauri::command]
pub fn save_config(
    state: State<'_, Arc<AppState>>,
    cfg: serde_json::Value,
) -> Result<(), String> {
    // 采用 JSON 深合并而不是整体覆盖：前端只改一个字段时
    // （例如只传 {"scan":{"roots":[...]}}）不能把模型路径等其他配置清空。
    let mut merged = serde_json::to_value(state.cfg.read().clone())
        .map_err(|e| format!("读取配置失败：{e}"))?;
    merge_json(&mut merged, &cfg);
    let new_cfg: AppConfig =
        serde_json::from_value(merged).map_err(|e| format!("配置格式错误：{e}"))?;

    // 注意：save() 要的是**配置文件路径**，不是目录。
    // 传目录会得到 "拒绝访问 (os error 5)"，而界面只看到一句看不懂的错误。
    let path = crate::config::config_path(Some(&state.app_dir));
    new_cfg
        .save(&path)
        .map_err(|e| format!("保存配置失败：{e}"))?;
    *state.cfg.write() = new_cfg;
    if let Some(indexer) = state.indexer() {
        let cfg = state.cfg.read();
        let batch_size = match cfg.model.parallel_mode.as_str() {
            "eco" => 1,
            "turbo" | "狂暴" => cfg.index.batch_size.max(96).min(128),
            _ => cfg.index.batch_size.max(48).min(64),
        };
        indexer.set_batch_size(batch_size);
    }
    Ok(())
}

/// 递归深合并：`src` 里的对象逐键并入 `dst`，数组与标量直接覆盖。
fn merge_json(dst: &mut serde_json::Value, src: &serde_json::Value) {
    match (dst, src) {
        (serde_json::Value::Object(d), serde_json::Value::Object(s)) => {
            for (k, v) in s {
                match d.get_mut(k) {
                    Some(slot) => merge_json(slot, v),
                    None => {
                        d.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (d, s) => *d = s.clone(),
    }
}

#[tauri::command]
pub fn search(state: State<'_, Arc<AppState>>, mut q: SearchQuery) -> Vec<Hit> {
    state.search(&mut q)
}

#[tauri::command]
pub fn search_semantic(state: State<'_, Arc<AppState>>, mut q: SearchQuery) -> Result<Vec<Hit>, String> {
    let text = q.text.clone();
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    let embedder = state.embedder.read();
    let info = embedder.info();
    let vec = embedder.embed_text(&text, true).map_err(|e| e.to_string())?;
    q.query_vector = Some(vec);
    if info.backend == "candle"
        && info.modalities.iter().any(|m| matches!(m.as_str(), "image" | "audio" | "video"))
    {
        q.vector_space = Some("shared".into());
    }
    q.use_semantic = Some(true);
    Ok(state.search(&mut q))
}

#[tauri::command]
pub fn similar(state: State<'_, Arc<AppState>>, file_id: i64, kind: Option<String>, top: Option<usize>) -> Vec<Hit> {
    let vecs = Arc::clone(&state.vectors.read());
    let media = Arc::clone(&state.media_vectors.read());
    let clip = Arc::clone(&state.clip_vectors.read());
    let audio = Arc::clone(&state.audio_vectors.read());
    let scan = state.cfg.read().scan.clone();
    let enabled_roots: Vec<String> = scan.roots.iter()
        .filter(|root| !scan.disabled_roots.iter().any(|disabled| {
            crate::util::norm_path(disabled).eq_ignore_ascii_case(&crate::util::norm_path(root))
        }))
        .cloned()
        .collect();
    crate::search::similar_scoped(
        &state.db,
        &vecs,
        &media,
        &clip,
        &audio,
        file_id,
        kind.as_deref(),
        top.unwrap_or(30),
        if scan.roots.is_empty() { None } else { Some(&enabled_roots) },
    )
}

#[derive(Serialize)]
pub struct KindStat {
    pub kind: String,
    pub label: String,
    pub count: i64,
    pub bytes: i64,
}

#[derive(Serialize)]
pub struct StatsOut {
    pub files: i64,
    pub bytes: i64,
    pub bytes_label: String,
    pub kinds: Vec<KindStat>,
    pub text_ok: i64,
    pub embed_ok: i64,
    pub missing: i64,
    pub db_bytes: i64,
    pub vector_count: i64,
    pub dim: i64,
}

#[derive(Serialize)]
pub struct IndexRootOut {
    pub root: String,
    pub enabled: bool,
    pub last_scan: i64,
    pub index_progress: f32,
    pub kinds_label: String,
    pub files: i64,
    pub bytes: i64,
    pub bytes_label: String,
    pub added: i64,
    pub updated: i64,
    pub skipped: i64,
    pub errors: i64,
    pub missing: i64,
    pub text_ok: i64,
    pub embed_ok: i64,
    pub completed: i64,
    pub text_pending: i64,
    pub embed_pending: i64,
    pub kinds: Vec<IndexKindStatOut>,
    pub last_error: String,
}

#[derive(Serialize)]
pub struct IndexKindStatOut {
    pub kind: String,
    pub label: String,
    pub count: i64,
    pub bytes: i64,
}

#[tauri::command]
pub fn index_roots(state: State<'_, Arc<AppState>>) -> Vec<IndexRootOut> {
    let scan = state.cfg.read().scan.clone();
    let _ = state.db.sync_index_roots(&scan.roots, &scan.disabled_roots);
    state.db.index_roots().unwrap_or_default().into_iter().map(|r| IndexRootOut {
        index_progress: if r.files > 0 {
            (r.completed as f32 * 100.0 / r.files as f32).clamp(0.0, 100.0)
        } else { 0.0 },
        kinds_label: r.kinds.iter().map(|k| format!("{} {}", crate::util::kind_label(&k.kind), k.count)).collect::<Vec<_>>().join(" / "),
        root: r.root,
        enabled: r.enabled,
        last_scan: r.last_scan,
        files: r.files,
        bytes: r.bytes,
        bytes_label: human_size(r.bytes),
        added: r.added,
        updated: r.updated,
        skipped: r.skipped,
        errors: r.errors,
        missing: r.missing,
        text_ok: r.text_ok,
        embed_ok: r.embed_ok,
        completed: r.completed,
        text_pending: r.text_pending,
        embed_pending: r.embed_pending,
        last_error: r.last_error,
        kinds: r.kinds.into_iter().map(|k| IndexKindStatOut {
            label: crate::util::kind_label(&k.kind).to_string(),
            kind: k.kind,
            count: k.count,
            bytes: k.bytes,
        }).collect(),
    }).collect()
}

#[tauri::command]
pub fn stats(state: State<'_, Arc<AppState>>) -> StatsOut {
    let db = &state.db;
    let files = db.count().unwrap_or(0);
    let bytes = db.total_bytes().unwrap_or(0);
    let kinds = db
        .kind_stats()
        .unwrap_or_default()
        .into_iter()
        .map(|(k, c, b)| KindStat {
            label: crate::util::kind_label(&k).to_string(),
            kind: k,
            count: c,
            bytes: b,
        })
        .collect();
    let missing = db
        .query_rows("SELECT id,path,dir,name,ext,kind,size,mtime,ctime,text_len,text_state,embed_state,thumb_key,category,tags FROM files WHERE missing=1", vec![])
        .len() as i64;
    let db_bytes = std::fs::metadata(state.data_dir.join("index.db"))
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let store = state.vectors.read();
    StatsOut {
        files,
        bytes,
        bytes_label: human_size(bytes),
        kinds,
        text_ok: db.count_state("text", "ok").unwrap_or(0),
        embed_ok: db.count_state("embed", "ok").unwrap_or(0),
        missing,
        db_bytes,
        vector_count: store.len() as i64,
        dim: store.dim() as i64,
    }
}

#[tauri::command]
pub fn recent(state: State<'_, Arc<AppState>>, limit: Option<usize>) -> Vec<Hit> {
    let mut q = SearchQuery::default();
    q.limit = Some(limit.unwrap_or(40));
    q.sort = Some("mtime".into());
    q.desc = true;
    state.search(&mut q)
}

// ---------------------------------------------------------------- 扫描
#[derive(Serialize, Clone)]
pub struct ScanProgress {
    pub running: bool,
    pub dirs: u64,
    pub files: u64,
    pub added: u64,
    pub updated: u64,
    pub skipped: u64,
    pub errors: u64,
    pub missing: u64,
    pub bytes: u64,
    pub current: String,
    pub seconds: f64,
    pub done: bool,
    pub cancelled: bool,
}

#[tauri::command]
pub fn scan_start(
    state: State<'_, Arc<AppState>>,
    roots: Option<Vec<String>>,
    index_what: Option<String>,
) -> Result<(), String> {
    if state.scan_busy.load(Ordering::Relaxed) {
        return Err("扫描已在进行中".into());
    }
    let cfg = state.cfg.read().clone();
    let mut sc = cfg.scan.clone();
    let requested: Option<std::collections::HashSet<String>> = roots.map(|items| {
        items.into_iter().map(|root| crate::util::norm_path(&root).to_ascii_lowercase()).collect()
    });
    sc.roots.retain(|root| {
        let normalized = crate::util::norm_path(root);
        let enabled = !sc.disabled_roots.iter().any(|disabled| {
            crate::util::norm_path(disabled).eq_ignore_ascii_case(&normalized)
        });
        enabled && requested.as_ref().map(|set| set.contains(&normalized.to_ascii_lowercase())).unwrap_or(true)
    });
    if sc.roots.is_empty() {
        return Err("请先选择要扫描的目录".into());
    }
    let _ = state.db.sync_index_roots(&cfg.scan.roots, &cfg.scan.disabled_roots);
    state.scan_cancel.store(false, Ordering::Relaxed);
    state.scan_busy.store(true, Ordering::Relaxed);
    let db = Arc::clone(&state.db);
    let app_state = Arc::clone(&state);
    let cancel = Arc::clone(&state.scan_cancel);
    let busy = Arc::clone(&state.scan_busy);
    let stats_shared = Arc::clone(&state.scan_stats);
    let auto_text = cfg.index.auto_fulltext;
    let auto_embed = cfg.index.auto_embed;
    let requested_index_what = index_what;
    std::thread::spawn(move || {
        let scanner = crate::scan::Scanner::with_shared(Arc::clone(&stats_shared), Arc::clone(&cancel));
        let stats = scanner.run(&db, &sc, |_| {});
        let scan_ts = crate::util::now_ts();
        for root in &stats.roots {
            let _ = db.upsert_index_root(
                &root.root,
                scan_ts,
                root.files as i64,
                root.bytes as i64,
                root.added as i64,
                root.updated as i64,
                root.skipped as i64,
                root.errors as i64,
                root.missing as i64,
                &root.last_error,
            );
            if root.errors > 0 {
                let _ = db.log("error", &format!("目录扫描 {}：{}；错误 {} 项：{}", root.root, if stats.cancelled { "已中断" } else { "完成" }, root.errors, root.last_error));
            } else {
                let _ = db.log("scan", &format!("目录扫描 {}：{} 个文件，新增 {}，更新 {}，跳过 {}，缺失 {}", root.root, root.files, root.added, root.updated, root.skipped, root.missing));
            }
        }
        if !stats.cancelled {
            let indexer = match app_state.indexer() {
                Some(i) => i,
                None => app_state.rebuild_indexer(),
            };
            let what = requested_index_what.as_deref().or(match (auto_text, auto_embed) {
                (true, true) => Some("auto"),
                (true, false) => Some("text"),
                (false, true) => Some("embed"),
                (false, false) => None,
            });
            if let Some(what) = what {
                indexer.set_progress_scope(Some(sc.roots.clone()));
                match db.pending_ids_in_roots(what, 100_000, &sc.roots) {
                    Ok(ids) => {
                        if !ids.is_empty() {
                            indexer.enqueue(ids, what);
                            if cancel.load(Ordering::Relaxed) {
                                indexer.cancel_pending();
                            }
                        }
                    }
                    Err(error) => {
                        let _ = db.log("error", &format!("查询待索引文件失败：{error}"));
                    }
                }
            }
        }
        busy.store(false, Ordering::Relaxed);
    });
    Ok(())
}

#[tauri::command]
pub fn scan_cancel(state: State<'_, Arc<AppState>>) {
    state.scan_cancel.store(true, Ordering::Relaxed);
    if let Some(indexer) = state.indexer() {
        indexer.cancel_pending();
    }
}

#[tauri::command]
pub fn scan_progress(state: State<'_, Arc<AppState>>) -> ScanProgress {
    let s = state.scan_stats.snapshot();
    ScanProgress {
        running: state.scan_busy.load(Ordering::Relaxed),
        dirs: s.dirs,
        files: s.files,
        added: s.added,
        updated: s.updated,
        skipped: s.skipped,
        errors: s.errors,
        missing: s.missing,
        bytes: s.bytes,
        current: s.current,
        seconds: s.seconds,
        done: s.done,
        cancelled: s.cancelled,
    }
}

// ---------------------------------------------------------------- 索引
#[tauri::command]
pub fn index_start(
    state: State<'_, Arc<AppState>>,
    what: Option<String>,
    force: Option<bool>,
    roots: Option<Vec<String>>,
) -> Result<usize, String> {
    let idx = match state.indexer() {
        Some(i) => i,
        None => state.rebuild_indexer(),
    };
    let what = what.unwrap_or_else(|| "auto".into());
    let scan = state.cfg.read().scan.clone();
    let configured = !scan.roots.is_empty();
    let enabled: Vec<String> = scan.roots.iter()
        .filter(|root| !scan.disabled_roots.iter().any(|disabled| {
            crate::util::norm_path(disabled).eq_ignore_ascii_case(&crate::util::norm_path(root))
        }))
        .cloned()
        .collect();
    let requested = roots.unwrap_or_else(|| enabled.clone());
    let scope: Vec<String> = if configured {
        requested.into_iter().filter(|root| enabled.iter().any(|active| {
            crate::util::norm_path(active).eq_ignore_ascii_case(&crate::util::norm_path(root))
        })).collect()
    } else {
        requested
    };
    if configured && scope.is_empty() {
        return Ok(0);
    }
    idx.set_progress_scope(if configured { Some(scope.clone()) } else { None });
    // force=true 时把所有文件标回 pending —— 换模型（维度变了）后
    // 旧向量会被清空，但 embed_state 还是 ok，不加这一步就永远重建不了。
    if force.unwrap_or(false) {
        idx.reset_progress();
        if configured {
            let _ = state.db.reset_states_in_roots(&what, &scope);
        } else {
            let _ = state.db.reset_states(&what);
        }
    }
    let ids = if configured {
        state.db.pending_ids_in_roots(&what, 100_000, &scope).map_err(|e| e.to_string())?
    } else {
        state.db.pending_ids(&what, 100_000).map_err(|e| e.to_string())?
    };
    let n = ids.len();
    if n > 0 {
        idx.enqueue(ids, &what);
    }
    Ok(n)
}

#[tauri::command]
pub fn index_cancel(state: State<'_, Arc<AppState>>) {
    if let Some(indexer) = state.indexer() {
        indexer.cancel_pending();
    }
}

#[tauri::command]
pub fn index_stats(state: State<'_, Arc<AppState>>) -> crate::index::IndexStats {
    match state.indexer() {
        Some(i) => i.snapshot(),
        None => crate::index::IndexStats::default(),
    }
}

// ---------------------------------------------------------------- 模型
#[derive(Serialize)]
pub struct ModelInfoOut {
    pub backend: String,
    pub model: String,
    pub dim: usize,
    pub device: String,
    pub modalities: Vec<String>,
    pub ready: bool,
    pub detail: String,
    /// 跨模态模型状态：图片（中文 CLIP）/ 声音（CLAP），没装就是空字符串
    pub clip: String,
    pub clap: String,
}

#[derive(Serialize)]
pub struct GpuDeviceOut {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub total_memory: u64,
    pub free_memory: u64,
    pub integrated: bool,
}

#[tauri::command]
pub fn gpu_devices() -> Vec<GpuDeviceOut> {
    #[cfg(feature = "llama")]
    {
        return crate::ai::llama::gpu_devices()
            .into_iter()
            .map(|d| GpuDeviceOut {
                id: d.id,
                name: d.name,
                backend: d.backend,
                total_memory: d.total_memory,
                free_memory: d.free_memory,
                integrated: d.integrated,
            })
            .collect();
    }
    #[cfg(not(feature = "llama"))]
    Vec::new()
}

#[tauri::command]
pub fn model_info(state: State<'_, Arc<AppState>>) -> ModelInfoOut {
    let i = state.embedder.read().info();
    let clip = state.clip.read().as_ref().map(|m| m.describe()).unwrap_or_default();
    let clap = state.clap.read().as_ref().map(|m| m.describe()).unwrap_or_default();
    ModelInfoOut {
        backend: i.backend,
        model: i.model,
        dim: i.dim,
        device: i.device,
        modalities: i.modalities,
        ready: i.ready,
        detail: i.detail,
        clip,
        clap,
    }
}

#[tauri::command]
pub fn model_load(state: State<'_, Arc<AppState>>, backend: String, path: Option<String>) -> Result<ModelInfoOut, String> {
    let mut cfg = state.cfg.read().clone();
    // The frontend sends a model path, never an implementation choice. Keep
    // the old backend argument as a compatibility alias for older clients,
    // but select the adapter from the path and its contents here.
    let requested = path.unwrap_or_else(|| cfg.model.embed_model.clone()).trim().to_string();
    let resolved = if backend.eq_ignore_ascii_case("hash") || requested.is_empty() {
        None
    } else {
        Some(crate::ai::resolve_model_path(&requested)?)
    };
    let resolved_s = resolved.as_ref().map(|p| p.to_string_lossy().to_string());
    let mode = if backend.eq_ignore_ascii_case("hash") || requested.is_empty() {
        "hash"
    } else if resolved.as_ref().map(|p| p.is_dir()).unwrap_or(false)
        || resolved.as_ref().and_then(|p| p.extension()).map(|x| x.eq_ignore_ascii_case("safetensors")).unwrap_or(false)
    {
        "candle"
    } else if resolved.as_ref().and_then(|p| p.extension()).map(|x| x.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
        "llama"
    } else {
        return Err(format!("无法识别模型格式：{}（需要 GGUF 或 safetensors）", requested));
    };
    cfg.model.embed_model = if mode == "hash" { String::new() } else { resolved_s.clone().unwrap_or(requested.clone()) };
    let emb: Arc<dyn crate::ai::Embedder> = match mode {
        "llama" => {
            #[cfg(feature = "llama")]
            {
                let e = crate::ai::llama::LlamaEmbedder::load(
                    resolved_s.as_deref().unwrap_or(&requested),
                    cfg.model.context_size as i32,
                    cfg.model.n_threads as i32,
                    &cfg.model.gpu_device,
                )?;
                Arc::new(e)
            }
            #[cfg(not(feature = "llama"))]
            {
                return Err("当前构建未启用 llama.cpp（用 --features llama 重新编译）".into());
            }
        }
        "candle" => {
            #[cfg(feature = "candle")]
            {
                let model_path = std::path::Path::new(resolved_s.as_deref().unwrap_or(&requested));
                let dir = if model_path.is_dir() {
                    model_path.to_path_buf()
                } else {
                    model_path
                        .parent()
                        .ok_or_else(|| "safetensors 模型路径没有父目录".to_string())?
                        .to_path_buf()
                };
                let e = crate::ai::candle_backend::CandleEmbedder::load(
                    &dir,
                    &cfg.model.gpu_device,
                    crate::ai::effective_batch_size(&cfg.model, cfg.index.batch_size),
                )?;
                Arc::new(e)
            }
            #[cfg(not(feature = "candle"))]
            {
                return Err("当前构建未启用 Candle；请使用包含 candle 的构建版本".into());
            }
        }
        _ => Arc::new(crate::ai::hash::HashEmbedder::new()),
    };
    let emb = crate::ai::maybe_replicate(emb, &cfg.model, cfg.index.batch_size);
    let info = emb.info();
    let dim = emb.dim();
    // Candle's multimodal model maps text and media into the same 768-D
    // space. Keep the legacy 256-D hash media space for llama/hash models so
    // existing indexes remain compatible with their backend.
    let shared_media = info.backend == "candle"
        && info.modalities.iter().any(|m| matches!(m.as_str(), "image" | "audio" | "video"));
    let media_dim = if shared_media { dim } else { 256 };
    let media_embedder: Arc<dyn crate::ai::Embedder> = if shared_media {
        Arc::clone(&emb)
    } else {
        Arc::new(crate::ai::hash::HashEmbedder::new())
    };
    cfg.model.embedding_dim = dim;
    let _ = cfg.save(&crate::config::config_path(Some(&state.app_dir)));
    *state.cfg.write() = cfg;
    *state.embedder.write() = emb;
    // 维度变了，向量库需要重建
    let vec_dir = state.data_dir.join("vectors");
    let old_count = state.vectors.read().len();
    if let Ok(v) = crate::vector::VectorStore::open(&vec_dir, dim) {
        // 换模型后维度变化会让旧的向量文件被丢弃。此时 embed_state 还是 ok，
        // 不重置就会「重建索引排队 0 个」——用户以为重建了，其实一条都没跑。
        let new_count = v.len();
        if old_count > 0 && new_count == 0 {
            let _ = state.db.reset_states("embed");
            let _ = state.db.log("model", "嵌入维度变化，已把全部文件标记为待重新向量化");
        }
        *state.vectors.write() = Arc::new(v);
    }
    let media_dir = state.data_dir.join("vectors_media");
    let old_media_count = state.media_vectors.read().len();
    if let Ok(v) = crate::vector::VectorStore::open(&media_dir, media_dim) {
        if old_media_count > 0 && v.len() == 0 {
            let _ = state.db.reset_states("embed");
            let _ = state.db.log("model", "媒体向量空间已切换，已把文件标记为待重新向量化");
        }
        *state.media_vectors.write() = Arc::new(v);
    }
    *state.media_embedder.write() = media_embedder;
    let _ = state.rebuild_indexer();
    let i = state.embedder.read().info();
    let clip = state.clip.read().as_ref().map(|m| m.describe()).unwrap_or_default();
    let clap = state.clap.read().as_ref().map(|m| m.describe()).unwrap_or_default();
    Ok(ModelInfoOut {
        backend: i.backend,
        model: i.model,
        dim: i.dim,
        device: i.device,
        modalities: i.modalities,
        ready: i.ready,
        detail: i.detail,
        clip,
        clap,
    })
}

// ---------------------------------------------------------------- 模型管理
/// Disable the active embedding model and restore the deterministic hash
/// baseline. Clearing the persisted path is part of the operation; otherwise
/// the previous model remains marked as active after the next refresh.
#[tauri::command]
pub fn model_deactivate(state: State<'_, Arc<AppState>>) -> Result<ModelInfoOut, String> {
    let mut cfg = state.cfg.read().clone();
    cfg.model.embed_model.clear();
    cfg.model.embedding_dim = 256;

    let emb: Arc<dyn crate::ai::Embedder> = Arc::new(crate::ai::hash::HashEmbedder::new());
    let media_emb: Arc<dyn crate::ai::Embedder> = Arc::new(crate::ai::hash::HashEmbedder::new());
    let old_count = state.vectors.read().len();
    let vectors = crate::vector::VectorStore::open(&state.data_dir.join("vectors"), 256)
        .map_err(|e| format!("打开基础向量库失败: {e}"))?;
    let mut needs_reindex = old_count > 0 && vectors.is_empty();
    if needs_reindex {
        let _ = state.db.reset_states("embed");
        let _ = state.db.log("model", "已停用嵌入模型，等待重新建立基础向量");
    }
    let old_media_count = state.media_vectors.read().len();
    let media_vectors = crate::vector::VectorStore::open(&state.data_dir.join("vectors_media"), 256)
        .map_err(|e| format!("打开媒体向量库失败: {e}"))?;
    if old_media_count > 0 && media_vectors.is_empty() {
        let _ = state.db.reset_states("embed");
        needs_reindex = true;
    }

    cfg.save(&crate::config::config_path(Some(&state.app_dir)))
        .map_err(|e| format!("保存模型配置失败: {e}"))?;
    *state.cfg.write() = cfg;
    *state.embedder.write() = emb;
    *state.media_embedder.write() = media_emb;
    *state.vectors.write() = Arc::new(vectors);
    *state.media_vectors.write() = Arc::new(media_vectors);
    let _ = state.rebuild_indexer();
    if needs_reindex {
        let _ = state.db.log("model", "模型已切换；待处理向量会在你手动开始索引后重建");
    }

    let i = state.embedder.read().info();
    let clip = state.clip.read().as_ref().map(|m| m.describe()).unwrap_or_default();
    let clap = state.clap.read().as_ref().map(|m| m.describe()).unwrap_or_default();
    Ok(ModelInfoOut {
        backend: i.backend,
        model: i.model,
        dim: i.dim,
        device: i.device,
        modalities: i.modalities,
        ready: i.ready,
        detail: i.detail,
        clip,
        clap,
    })
}

#[tauri::command]
pub fn model_list(state: State<'_, Arc<AppState>>) -> Vec<crate::models::ModelEntry> {
    let cfg = state.cfg.read().model.clone();
    let dir = crate::models::model_dir(&state.app_dir, &state.data_dir);
    crate::models::scan(&cfg, &[dir])
}

#[tauri::command]
pub fn model_dir_path(state: State<'_, Arc<AppState>>) -> String {
    crate::models::model_dir(&state.app_dir, &state.data_dir)
        .to_string_lossy()
        .to_string()
}

#[tauri::command]
pub fn model_delete(state: State<'_, Arc<AppState>>, path: String) -> Result<(), String> {
    let cfg = state.cfg.read().model.clone();
    crate::models::remove(&path, &cfg)?;
    let _ = state.db.log("model", &format!("已删除模型文件：{path}"));
    Ok(())
}

#[tauri::command]
pub fn model_import(
    state: State<'_, Arc<AppState>>,
    src: String,
) -> Result<String, String> {
    let dir = crate::models::model_dir(&state.app_dir, &state.data_dir);
    let dest = crate::models::import(&src, &dir)?;
    let _ = state.db.log("model", &format!("已导入模型：{dest}"));
    Ok(dest)
}

/// 导入一个模型文件夹，自动识别 GGUF / safetensors 及 tokenizer、配置等配套文件。
#[tauri::command]
pub fn model_import_folder(
    state: State<'_, Arc<AppState>>,
    src: String,
) -> Result<crate::models::ModelImportResult, String> {
    let dir = crate::models::model_dir(&state.app_dir, &state.data_dir);
    let result = crate::models::import_folder(&src, &dir)?;
    let msg = format!("已导入模型目录：{}（{} 个文件，{}）", result.destination, result.copied_files, crate::util::human_size(result.total_bytes));
    let _ = state.db.log("model", &msg);
    Ok(result)
}

// ---------------------------------------------------------------- 文件操作
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    open::that(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let p = Path::new(&path);
        open::that(p.parent().unwrap_or(p)).map_err(|e| e.to_string())
    }
}

/// Rename an indexed file and keep the SQLite/FTS metadata in sync.
/// Only a filename is accepted; directory traversal and cross-directory
/// moves are intentionally rejected.
#[tauri::command]
pub fn rename_path(
    state: State<'_, Arc<AppState>>,
    path: String,
    name: String,
) -> Result<String, String> {
    let old_path = crate::util::norm_path(&path);
    let source = Path::new(&old_path);
    if !source.is_file() {
        return Err("源文件不存在或不可访问".into());
    }
    let new_name = name.trim();
    if new_name.is_empty()
        || new_name == "."
        || new_name == ".."
        || new_name.chars().any(|c| c.is_control())
        || new_name.contains('/')
        || new_name.contains('\\')
    {
        return Err("新名称必须是有效的文件名".into());
    }
    // Windows rejects these names; checking here gives a consistent error on
    // every platform and prevents accidental device-path handling.
    if cfg!(windows) && ["CON", "PRN", "AUX", "NUL"].iter().any(|v| new_name.eq_ignore_ascii_case(v)) {
        return Err("新名称不能使用系统保留名称".into());
    }
    let row = state
        .db
        .get_by_path(&old_path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "文件不在当前索引中".to_string())?;
    let destination = source
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(new_name);
    let new_path = crate::util::norm_path(&destination.to_string_lossy());
    if crate::util::norm_path(&row.path) != old_path {
        return Err("索引路径不一致，请重新扫描".into());
    }
    if crate::util::norm_path(&destination.to_string_lossy()) == old_path {
        return Ok(old_path);
    }
    if destination.exists() {
        return Err("目标文件名已存在".into());
    }
    std::fs::rename(source, &destination).map_err(|e| format!("重命名失败：{e}"))?;
    if let Err(error) = state.db.rename_file(&old_path, &new_path) {
        // Keep the filesystem and index consistent if the database write
        // fails. The rollback is best effort and never deletes user data.
        let _ = std::fs::rename(&destination, source);
        return Err(format!("更新索引失败：{error}"));
    }
    let _ = state.db.log("file", &format!("文件已重命名：{old_path} -> {new_path}"));
    Ok(new_path)
}

/// Lazily create a gallery thumbnail for document formats whose preview is
/// expensive. The file id is checked against the database before writing so
/// a stale UI result cannot make a thumbnail for an unrelated path.
#[tauri::command]
pub fn ensure_preview(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    path: String,
) -> Result<bool, String> {
    let row = state
        .db
        .get_by_id(file_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "文件不存在".to_string())?;
    if crate::util::norm_path(&row.path) != crate::util::norm_path(&path) {
        return Err("预览路径与索引记录不一致".into());
    }
    let source = Path::new(&row.path);
    if !source.is_file() {
        return Ok(false);
    }
    let thumb_dir = state.data_dir.join("thumbs");
    std::fs::create_dir_all(&thumb_dir).map_err(|error| error.to_string())?;
    let dest = thumb_dir.join(format!("{file_id}.jpg"));
    if !dest.is_file() {
        let made = if matches!(row.kind.as_str(), "image" | "video") {
            crate::media::make_thumb(source, &dest, state.cfg.read().index.image_max_side)
        } else {
            crate::media::make_document_thumb(source, &dest, state.cfg.read().index.image_max_side)
        };
        if !made {
            return Ok(false);
        }
    }
    state
        .db
        .set_thumb(file_id, &format!("{file_id}.jpg"))
        .map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn op_export(paths: Vec<String>, out: String, fmt: Option<String>) -> Result<usize, String> {
    fileops::export_listing(&paths, Path::new(&out), &fmt.unwrap_or_else(|| "csv".into()))
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- 规则
#[tauri::command]
pub fn rules_list(state: State<'_, Arc<AppState>>) -> Vec<Rule> {
    state.db.list_rules().unwrap_or_default()
}

#[tauri::command]
pub fn rules_save(state: State<'_, Arc<AppState>>, rules: Vec<Rule>) -> Result<(), String> {
    state.db.replace_rules(&rules).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rules_apply(state: State<'_, Arc<AppState>>) -> usize {
    fileops::apply_rules_to_db(&state.db)
}

// ---------------------------------------------------------------- 日志
#[tauri::command]
pub fn activity_list(state: State<'_, Arc<AppState>>, kind: Option<String>, limit: Option<usize>) -> Vec<Activity> {
    state.db.recent_activity(limit.unwrap_or(200), kind.as_deref()).unwrap_or_default()
}

// ---------------------------------------------------------------- 树状图
#[derive(Serialize)]
pub struct TreeNode {
    pub path: String,
    pub name: String,
    pub size: i64,
    pub count: i64,
    pub kinds: std::collections::HashMap<String, i64>,
    pub children: Vec<TreeNode>,
}

#[tauri::command]
pub fn tree_data(state: State<'_, Arc<AppState>>, root: Option<String>, depth: Option<usize>) -> TreeNode {
    let depth = depth.unwrap_or(3).clamp(1, 8);
    let rows = state
        .db
        .query_rows(
            "SELECT id,path,dir,name,ext,kind,size,mtime,ctime,text_len,text_state,embed_state,thumb_key,category,tags FROM files WHERE missing=0",
            vec![],
        );
    // 不指定根目录时，从「所有已索引文件的公共父目录」开始，
    // 而不是从盘符（C:\）开始 —— 否则用户要连点五六层才能看到自己的文件夹。
    let root = match root {
        Some(r) if !r.trim().is_empty() => r,
        _ => smart_root(&rows).unwrap_or_default(),
    };
    let row_refs: Vec<&FileRow> = rows.iter().collect();
    build_tree(&row_refs, &root, depth)
}

/// 所有文件所在目录的最长公共前缀（按路径分隔符对齐）。
fn common_root(rows: &[FileRow]) -> Option<String> {
    let mut iter = rows.iter().filter(|r| !r.dir.is_empty());
    let first = iter.next()?.dir.clone();
    let mut prefix: Vec<String> = first.split('\\').map(|s| s.to_string()).collect();
    for r in iter {
        let parts: Vec<&str> = r.dir.split('\\').collect();
        let mut n = 0;
        while n < prefix.len()
            && n < parts.len()
            && prefix[n].eq_ignore_ascii_case(parts[n])
        {
            n += 1;
        }
        prefix.truncate(n);
        if prefix.is_empty() {
            return None;
        }
    }
    Some(prefix.join("\\"))
}

/// 如果公共前缀浅到只剩盘符（`C:`），树状图会退化成一整块盘，用户要点五六层
/// 才能看到自己的目录。这时改取「文件数最多的那条路径分支」作为根。
fn smart_root(rows: &[FileRow]) -> Option<String> {
    let common = common_root(rows)?;
    let parts: Vec<&str> = common.split('\\').filter(|s| !s.is_empty()).collect();
    if parts.len() > 1 {
        return Some(common);
    }
    let mut tally: std::collections::HashMap<String, (usize, u64)> = std::collections::HashMap::new();
    for r in rows.iter().filter(|r| !r.dir.is_empty()) {
        let ps: Vec<&str> = r.dir.split('\\').filter(|s| !s.is_empty()).collect();
        // 盘符 + 一级目录（`C:\Users`）；没有一级目录就只用盘符
        let key = if ps.len() >= 2 {
            format!("{}\\{}", ps[0], ps[1])
        } else {
            ps[0].to_string()
        };
        let e = tally.entry(key).or_insert((0, 0));
        e.0 += 1;
        e.1 += r.size.max(0) as u64;
    }
    tally
        .into_iter()
        .max_by_key(|(_, (n, bytes))| (*n, *bytes))
        .map(|(k, _)| k)
        .or(Some(common))
}

fn build_tree(rows: &[&FileRow], root: &str, depth: usize) -> TreeNode {
    let root_norm = root.trim_end_matches('\\').to_lowercase();
    let mut node = TreeNode {
        path: root.to_string(),
        name: if root.is_empty() {
            "全部".into()
        } else {
            Path::new(root)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| root.to_string())
        },
        size: 0,
        count: 0,
        kinds: std::collections::HashMap::new(),
        children: vec![],
    };
    let mut buckets: std::collections::HashMap<String, Vec<&FileRow>> = std::collections::HashMap::new();
    for r in rows {
        if !root_norm.is_empty() && !r.path.to_lowercase().starts_with(&root_norm) {
            continue;
        }
        node.size += r.size;
        node.count += 1;
        *node.kinds.entry(r.kind.clone()).or_insert(0) += 1;
        if depth > 1 {
            // 取 root 之后的第一层目录
            let rest = if root_norm.is_empty() {
                r.path.as_str()
            } else {
                &r.path[root_norm.len().min(r.path.len())..]
            };
            let rest = rest.trim_start_matches('\\');
            if let Some(pos) = rest.find('\\') {
                let seg = &rest[..pos];
                let key = if root_norm.is_empty() {
                    seg.to_string()
                } else {
                    format!("{}\\{}", root.trim_end_matches('\\'), seg)
                };
                buckets.entry(key).or_default().push(*r);
            }
        }
    }
    if depth > 1 {
        let mut keys: Vec<String> = buckets.keys().cloned().collect();
        keys.sort();
        for k in keys {
            if let Some(children) = buckets.get(&k) {
                node.children.push(build_tree(children, &k, depth - 1));
            }
        }
    }
    node
}

// ---------------------------------------------------------------- 监控
#[derive(Serialize)]
pub struct WatchStatus {
    pub running: bool,
    pub roots: Vec<String>,
}

#[tauri::command]
pub fn watch_start(state: State<'_, Arc<AppState>>) -> Result<usize, String> {
    let cfg = state.cfg.read().clone();
    let roots = if cfg.watch.roots.is_empty() {
        cfg.scan.roots.clone()
    } else {
        cfg.watch.roots.clone()
    };
    if roots.is_empty() {
        return Err("请先设置监控目录".into());
    }
    let n = state.watcher.start(&roots, cfg.watch.debounce_secs as f64, Arc::clone(&state.db))?;
    let _ = state.db.log("watch", &format!("开始监控 {} 个目录", n));
    Ok(n)
}

#[tauri::command]
pub fn watch_stop(state: State<'_, Arc<AppState>>) {
    state.watcher.stop();
    let _ = state.db.log("watch", "停止监控");
}

#[tauri::command]
pub fn watch_status(state: State<'_, Arc<AppState>>) -> WatchStatus {
    let cfg = state.cfg.read().clone();
    WatchStatus {
        running: state.watcher.running(),
        roots: if cfg.watch.roots.is_empty() { cfg.scan.roots } else { cfg.watch.roots },
    }
}

// ---------------------------------------------------------------- 局域网
#[derive(Serialize)]
pub struct ServerStatus {
    pub running: bool,
    pub url: Option<String>,
    pub port: u16,
}

#[tauri::command]
pub fn server_start(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let cfg = state.cfg.read().clone();
    let p = state.search_engine();
    let st = crate::server::ServerState {
        db: p.db.clone(),
        vectors: p.vectors.clone(),
        media_vectors: p.media.clone(),
        clip_vectors: p.clip.clone(),
        audio_vectors: p.audio.clone(),
        cfg: Arc::new(p.cfg.clone()),
        scan_roots: cfg.scan.roots.clone(),
        disabled_roots: cfg.scan.disabled_roots.clone(),
        token: cfg.server.token.clone(),
        allow_write: cfg.server.allow_write,
        data_dir: state.data_dir.clone(),
    };
    let url = state.server.start(&cfg.server, st)?;
    let _ = state.db.log("server", &format!("局域网服务启动 {url}"));
    Ok(url)
}

#[tauri::command]
pub fn server_stop(state: State<'_, Arc<AppState>>) {
    state.server.stop();
    let _ = state.db.log("server", "局域网服务停止");
}

#[tauri::command]
pub fn server_status(state: State<'_, Arc<AppState>>) -> ServerStatus {
    let cfg = state.cfg.read().clone();
    ServerStatus { running: state.server.running(), url: state.server.url(), port: cfg.server.port }
}

// ---------------------------------------------------------------- 杂项
#[tauri::command]
pub fn db_vacuum(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.db.vacuum().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn db_clear(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.db.clear_all().map_err(|e| e.to_string())?;
    let vec_dir = state.data_dir.join("vectors");
    let dim = state.cfg.read().model.embedding_dim;
    if let Ok(v) = crate::vector::VectorStore::open(&vec_dir, dim) {
        let _ = v.clear();
        *state.vectors.write() = Arc::new(v);
    }
    Ok(())
}

#[tauri::command]
pub fn data_dir_path(state: State<'_, Arc<AppState>>) -> String {
    state.data_dir.to_string_lossy().to_string()
}

#[derive(Serialize)]
pub struct ExtensionStatusOut {
    pub onnxruntime: bool,
    pub onnxruntime_path: String,
    pub python: bool,
    pub python_path: String,
    pub root: String,
}

fn extension_root(state: &AppState) -> std::path::PathBuf {
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    candidates.push(state.app_dir.clone());
    candidates.push(state.app_dir.join("resources"));
    let mut parent = state.app_dir.clone();
    for _ in 0..4 {
        if let Some(next) = parent.parent() {
            parent = next.to_path_buf();
            candidates.push(parent.clone());
        } else {
            break;
        }
    }
    candidates
        .into_iter()
        .find(|p| p.join(".tools").exists() || p.join("scripts").exists())
        .unwrap_or_else(|| state.app_dir.clone())
}

#[tauri::command]
pub fn extension_status(state: State<'_, Arc<AppState>>) -> ExtensionStatusOut {
    extension_status_for(&state)
}

fn extension_status_for(state: &AppState) -> ExtensionStatusOut {
    let root = extension_root(state);
    let ort = root.join(".tools").join("onnxruntime").join("onnxruntime.dll");
    let python = root.join(".tools").join("python").join("python.exe");
    ExtensionStatusOut {
        onnxruntime: ort.is_file(),
        onnxruntime_path: ort.to_string_lossy().replace('/', "\\"),
        python: python.is_file(),
        python_path: python.to_string_lossy().replace('/', "\\"),
        root: root.to_string_lossy().replace('/', "\\"),
    }
}

/// Install an optional extension into the project-local `.tools` directory.
/// Only the two fixed script names are accepted; no user-provided command is
/// executed and existing files are never removed.
#[tauri::command]
pub fn extension_install(state: State<'_, Arc<AppState>>, kind: String) -> Result<ExtensionStatusOut, String> {
    let script_name = match kind.as_str() {
        "onnxruntime" => "install-onnxruntime.ps1",
        "python" => "install-python-sidecar.ps1",
        _ => return Err("未知扩展类型".into()),
    };
    let root = extension_root(&state);
    let mut script = root.join("scripts").join(script_name);
    if !script.is_file() {
        script = state.app_dir.join("resources").join("scripts").join(script_name);
    }
    if !script.is_file() {
        return Err(format!("找不到扩展安装脚本：{}", script.display()));
    }
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .current_dir(&root)
        .output()
        .map_err(|e| format!("启动 PowerShell 失败：{e}"))?;
    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if msg.is_empty() { format!("扩展安装失败（退出码 {:?}）", output.status.code()) } else { msg });
    }
    let _ = state.db.log("extension", &format!("已安装项目扩展：{kind}"));
    Ok(extension_status_for(&state))
}
