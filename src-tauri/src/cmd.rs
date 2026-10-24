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
    let vec = state
        .embedder
        .read()
        .embed_text(&text, true)
        .map_err(|e| e.to_string())?;
    q.query_vector = Some(vec);
    q.use_semantic = Some(true);
    Ok(state.search(&mut q))
}

#[tauri::command]
pub fn similar(state: State<'_, Arc<AppState>>, file_id: i64, kind: Option<String>, top: Option<usize>) -> Vec<Hit> {
    let vecs = Arc::clone(&state.vectors.read());
    let media = Arc::clone(&state.media_vectors.read());
    let clip = Arc::clone(&state.clip_vectors.read());
    let audio = Arc::clone(&state.audio_vectors.read());
    crate::search::similar(
        &state.db,
        &vecs,
        &media,
        &clip,
        &audio,
        file_id,
        kind.as_deref(),
        top.unwrap_or(30),
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
}

#[tauri::command]
pub fn scan_start(state: State<'_, Arc<AppState>>, roots: Option<Vec<String>>) -> Result<(), String> {
    if state.scan_busy.load(Ordering::Relaxed) {
        return Err("扫描已在进行中".into());
    }
    let cfg = state.cfg.read().clone();
    let mut sc = cfg.scan.clone();
    if let Some(r) = roots {
        if !r.is_empty() {
            sc.roots = r;
        }
    }
    if sc.roots.is_empty() {
        return Err("请先选择要扫描的目录".into());
    }
    state.scan_cancel.store(false, Ordering::Relaxed);
    state.scan_busy.store(true, Ordering::Relaxed);
    let db = Arc::clone(&state.db);
    let cancel = Arc::clone(&state.scan_cancel);
    let busy = Arc::clone(&state.scan_busy);
    let stats_shared = Arc::clone(&state.scan_stats);
    let auto_text = cfg.index.auto_fulltext;
    let auto_embed = cfg.index.auto_embed;
    std::thread::spawn(move || {
        let scanner = crate::scan::Scanner::with_shared(Arc::clone(&stats_shared), Arc::clone(&cancel));
        let stats = scanner.run(&db, &sc, |batch| {
            for row in batch {
                let _ = db.upsert_file(row);
            }
        });
        let _ = stats;
        busy.store(false, Ordering::Relaxed);
        let _ = auto_text;
        let _ = auto_embed;
    });
    Ok(())
}

#[tauri::command]
pub fn scan_cancel(state: State<'_, Arc<AppState>>) {
    state.scan_cancel.store(true, Ordering::Relaxed);
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
    }
}

// ---------------------------------------------------------------- 索引
#[tauri::command]
pub fn index_start(
    state: State<'_, Arc<AppState>>,
    what: Option<String>,
    force: Option<bool>,
) -> Result<usize, String> {
    let idx = match state.indexer() {
        Some(i) => i,
        None => state.rebuild_indexer(),
    };
    let what = what.unwrap_or_else(|| "auto".into());
    // force=true 时把所有文件标回 pending —— 换模型（维度变了）后
    // 旧向量会被清空，但 embed_state 还是 ok，不加这一步就永远重建不了。
    if force.unwrap_or(false) {
        let _ = state.db.reset_states(&what);
    }
    let n = idx.enqueue_all_pending(&what);
    Ok(n)
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
    if let Some(p) = &path {
        cfg.model.embed_model = p.clone();
    }
    let emb: Arc<dyn crate::ai::Embedder> = match backend.as_str() {
        "llama" => {
            #[cfg(feature = "llama")]
            {
                let model = path.unwrap_or_else(|| cfg.model.embed_model.clone());
                if model.trim().is_empty() {
                    return Err("请先在设置里指定 GGUF 模型路径".into());
                }
                let e = crate::ai::llama::LlamaEmbedder::load(
                    &model,
                    cfg.model.n_gpu_layers as i32,
                    cfg.model.context_size as i32,
                    cfg.model.n_threads as i32,
                )?;
                Arc::new(e)
            }
            #[cfg(not(feature = "llama"))]
            {
                return Err("当前构建未启用 llama.cpp（用 --features llama 重新编译）".into());
            }
        }
        _ => Arc::new(crate::ai::hash::HashEmbedder::new()),
    };
    let dim = emb.dim();
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
    build_tree(&rows, &root, depth)
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

fn build_tree(rows: &[FileRow], root: &str, depth: usize) -> TreeNode {
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
    let mut buckets: std::collections::HashMap<String, Vec<FileRow>> = std::collections::HashMap::new();
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
                buckets.entry(key).or_default().push(r.clone());
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
