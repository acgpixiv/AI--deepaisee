//! 目录扫描：遍历、过滤、写入 files 表。
use crate::config::ScanConfig;
use crate::db::{Db, FileRow};
use crate::util::{ext_kind, now_ts};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanStats {
    pub dirs: u64,
    pub files: u64,
    pub added: u64,
    pub updated: u64,
    pub skipped: u64,
    pub errors: u64,
    pub missing: u64,
    pub bytes: u64,
    pub done: bool,
    pub cancelled: bool,
    pub current: String,
    pub seconds: f64,
}

pub struct Scanner {
    pub stats: Arc<ScanStatsShared>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct ScanStatsShared {
    pub dirs: AtomicU64,
    pub files: AtomicU64,
    pub added: AtomicU64,
    pub updated: AtomicU64,
    pub skipped: AtomicU64,
    pub errors: AtomicU64,
    pub missing: AtomicU64,
    pub bytes: AtomicU64,
    pub done: AtomicBool,
    pub cancelled: AtomicBool,
}

impl ScanStatsShared {
    pub fn new() -> Self {
        Self::default()
    }

    /// 每次开始新扫描前清零，否则进度条上的数字会一轮轮累加
    /// （实测连扫两次后 files 变成 31→46，用户会以为文件变多了）。
    pub fn reset(&self) {
        for c in [
            &self.dirs,
            &self.files,
            &self.added,
            &self.updated,
            &self.skipped,
            &self.errors,
            &self.missing,
            &self.bytes,
        ] {
            c.store(0, Ordering::Relaxed);
        }
        self.done.store(false, Ordering::Relaxed);
        self.cancelled.store(false, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ScanStats {
        ScanStats {
            dirs: self.dirs.load(Ordering::Relaxed),
            files: self.files.load(Ordering::Relaxed),
            added: self.added.load(Ordering::Relaxed),
            updated: self.updated.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            missing: self.missing.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            current: String::new(),
            seconds: 0.0,
        }
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            stats: Arc::new(ScanStatsShared::default()),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// 用外部共享的统计/取消标志构造（供 AppState 轮询进度）。
    pub fn with_shared(stats: Arc<ScanStatsShared>, cancel: Arc<AtomicBool>) -> Self {
        Self { stats, cancel }
    }

    pub fn snapshot(&self) -> ScanStats {
        self.stats.snapshot()
    }

    /// 同步扫描（在调用方线程或任务里运行）。
    pub fn run<F>(&self, db: &Db, cfg: &ScanConfig, mut on_batch: F) -> ScanStats
    where
        F: FnMut(&[FileRow]),
    {
        let started = Instant::now();
        self.cancel.store(false, Ordering::SeqCst);
        self.stats.reset();
        let excludes: Vec<String> = cfg.exclude_dirs.iter().map(|s| s.to_lowercase()).collect();
        let mut batch: Vec<FileRow> = Vec::with_capacity(400);
        // 本次扫描实际遇到的 id：扫描结束后，根目录下没出现在这里的旧记录就是已被删除的
        let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let now = now_ts();

        for root in &cfg.roots {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }
            let root_path = Path::new(root);
            if !root_path.exists() {
                // 整个根都没了：它底下的记录全部标记为缺失，否则会永远显示在列表里
                let mut prefix = crate::util::norm_path(root);
                if !prefix.ends_with("\\") {
                    prefix.push('\\');
                }
                if let Ok(n) = db.mark_missing_prefix(&prefix) {
                    if n > 0 {
                        self.stats.missing.fetch_add(n as u64, Ordering::Relaxed);
                    }
                }
                self.stats.errors.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let walker = walkdir::WalkDir::new(root_path)
                .follow_links(cfg.follow_links)
                .into_iter()
                .filter_entry(|e| {
                    if e.depth() == 0 {
                        return true;
                    }
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if e.file_type().is_dir() && excludes.iter().any(|x| name == *x || name.contains(x.as_str())) {
                        return false;
                    }
                    !name.starts_with("$recycle.bin")
                });
            for entry in walker {
                if self.cancel.load(Ordering::SeqCst) {
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        self.stats.errors.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                let ft = entry.file_type();
                if ft.is_dir() {
                    self.stats.dirs.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if !ft.is_file() {
                    continue;
                }
                let path = entry.path();
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let kind = ext_kind(&ext);
                let accepted = match kind {
                    "image" => cfg.images,
                    "video" => cfg.videos,
                    "audio" => cfg.audios,
                    "document" | "text" => cfg.documents,
                    "archive" => cfg.archives,
                    _ => cfg.others,
                };
                if !accepted {
                    self.stats.skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => {
                        self.stats.errors.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                let size = meta.len();
                if size < cfg.min_size_kb * 1024 || size > cfg.max_size_mb * 1024 * 1024 {
                    self.stats.skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(now);
                let mut row = FileRow::new(&path.to_string_lossy());
                row.kind = kind.to_string();
                row.size = size as i64;
                row.mtime = mtime;
                row.ctime = now;

                let existed = db.get_by_path(&row.path).ok().flatten();
                match existed {
                    Some(old) => {
                        if old.size == row.size && old.mtime == row.mtime && !old.path.is_empty() {
                            seen.insert(old.id);
                            self.stats.files.fetch_add(1, Ordering::Relaxed);
                            self.stats.bytes.fetch_add(size, Ordering::Relaxed);
                            continue;
                        }
                        if let Err(_) = db.upsert_file(&row) {
                            self.stats.errors.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        self.stats.updated.fetch_add(1, Ordering::Relaxed);
                    }
                    None => {
                        if let Err(_) = db.upsert_file(&row) {
                            self.stats.errors.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        self.stats.added.fetch_add(1, Ordering::Relaxed);
                    }
                }
                self.stats.files.fetch_add(1, Ordering::Relaxed);
                self.stats.bytes.fetch_add(size, Ordering::Relaxed);
                if let Ok(Some(saved)) = db.get_by_path(&row.path) {
                    seen.insert(saved.id);
                    batch.push(saved);
                }
                if batch.len() >= 400 {
                    on_batch(&batch);
                    batch.clear();
                }
            }
        }
        if !batch.is_empty() {
            on_batch(&batch);
        }
        // 把「扫描根下、这次没遇到」的旧记录标成 missing。
        // 不这么做，删掉的文件会永远留在库里，统计越扫越大（用户看到「35 个文件」实际只有 16 个）。
        if !self.cancel.load(Ordering::SeqCst) {
            for root in &cfg.roots {
                let prefix = crate::util::norm_path(root);
                let prefix = if prefix.ends_with('\\') {
                    prefix
                } else {
                    format!("{prefix}\\")
                };
                if let Ok(stale) = db.mark_missing_except(&prefix, &seen) {
                    if stale > 0 {
                        self.stats.missing.fetch_add(stale as u64, Ordering::Relaxed);
                    }
                }
            }
        }
        self.stats.done.store(true, Ordering::SeqCst);
        self.stats.cancelled.store(self.cancel.load(Ordering::SeqCst), Ordering::SeqCst);
        let mut snap = self.snapshot();
        snap.seconds = started.elapsed().as_secs_f64();
        snap.current = String::new();
        snap
    }
}

/// 快速统计（不写库），用于预估。
pub fn quick_count(roots: &[String], cfg: &ScanConfig) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let excludes: Vec<String> = cfg.exclude_dirs.iter().map(|s| s.to_lowercase()).collect();
    for root in roots {
        for entry in walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            !(e.file_type().is_dir() && excludes.iter().any(|x| name == *x))
        }) {
            if let Ok(e) = entry {
                if e.file_type().is_file() {
                    files += 1;
                    if let Ok(m) = e.metadata() {
                        bytes += m.len();
                    }
                }
            }
        }
    }
    (files, bytes)
}
