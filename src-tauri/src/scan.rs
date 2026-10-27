//! 目录扫描：遍历、过滤、写入 files 表。
use crate::config::ScanConfig;
use crate::db::{Db, FileRow};
use crate::util::{ext_kind, now_ts};
use parking_lot::Mutex;
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
    pub roots: Vec<RootScanStats>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RootScanStats {
    pub root: String,
    pub dirs: u64,
    pub files: u64,
    pub added: u64,
    pub updated: u64,
    pub skipped: u64,
    pub errors: u64,
    pub missing: u64,
    pub bytes: u64,
    pub last_error: String,
}

impl RootScanStats {
    fn record_error(&mut self, message: impl AsRef<str>) {
        if self.last_error.len() < 8_000 {
            if !self.last_error.is_empty() {
                self.last_error.push_str("; ");
            }
            self.last_error.push_str(message.as_ref());
            self.last_error.truncate(8_000);
        }
    }
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
    pub roots: Mutex<Vec<RootScanStats>>,
    pub current: Mutex<String>,
    pub started_at: Mutex<Option<Instant>>,
    pub elapsed_millis: AtomicU64,
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
        self.roots.lock().clear();
        self.current.lock().clear();
        *self.started_at.lock() = Some(Instant::now());
        self.elapsed_millis.store(0, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ScanStats {
        let done = self.done.load(Ordering::Relaxed);
        let seconds = if done {
            self.elapsed_millis.load(Ordering::Relaxed) as f64 / 1000.0
        } else {
            self.started_at
                .lock()
                .as_ref()
                .map(|started| started.elapsed().as_secs_f64())
                .unwrap_or(0.0)
        };
        ScanStats {
            dirs: self.dirs.load(Ordering::Relaxed),
            files: self.files.load(Ordering::Relaxed),
            added: self.added.load(Ordering::Relaxed),
            updated: self.updated.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            missing: self.missing.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            done,
            cancelled: self.cancelled.load(Ordering::Relaxed),
            current: if done { String::new() } else { self.current.lock().clone() },
            seconds,
            roots: self.roots.lock().clone(),
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
        self.stats.reset();
        let excludes: Vec<String> = cfg.exclude_dirs.iter().map(|s| s.to_lowercase()).collect();
        let mut batch: Vec<FileRow> = Vec::with_capacity(400);
        // 本次扫描实际遇到的 id：扫描结束后，根目录下没出现在这里的旧记录就是已被删除的
        let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut complete_roots = std::collections::HashSet::new();
        let now = now_ts();

        for root in cfg.roots.iter().filter(|root| !root_is_disabled(root, cfg)) {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }
            let root_path = Path::new(root);
            let mut root_stats = RootScanStats {
                root: crate::util::norm_path(root),
                ..RootScanStats::default()
            };
            if let Err(error) = std::fs::metadata(root_path) {
                if error.kind() == std::io::ErrorKind::NotFound {
                    // A confirmed missing root can no longer contain any indexed files.
                    self.stats.errors.fetch_add(1, Ordering::Relaxed);
                    root_stats.errors += 1;
                    root_stats.record_error("目录不存在");
                    let mut prefix = crate::util::norm_path(root);
                    if !prefix.ends_with("\\") {
                        prefix.push('\\');
                    }
                    match db.mark_missing_prefix(&prefix) {
                        Ok(n) => {
                            if n > 0 {
                                self.stats.missing.fetch_add(n as u64, Ordering::Relaxed);
                                root_stats.missing += n as u64;
                            }
                        }
                        Err(error) => {
                            self.stats.errors.fetch_add(1, Ordering::Relaxed);
                            root_stats.errors += 1;
                            root_stats.record_error(error.to_string());
                        }
                    }
                } else {
                    self.stats.errors.fetch_add(1, Ordering::Relaxed);
                    root_stats.errors += 1;
                    root_stats.record_error(error.to_string());
                }
                self.stats.roots.lock().push(root_stats);
                continue;
            }
            let root_incomplete = AtomicBool::new(false);
            let walker = walkdir::WalkDir::new(root_path)
                .follow_links(cfg.follow_links)
                .into_iter()
                .filter_entry(|e| {
                    if e.depth() == 0 {
                        return true;
                    }
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if e.file_type().is_dir() && excludes.iter().any(|x| name == *x || name.contains(x.as_str())) {
                        root_incomplete.store(true, Ordering::Relaxed);
                        return false;
                    }
                    let include = !name.starts_with("$recycle.bin");
                    if !include {
                        root_incomplete.store(true, Ordering::Relaxed);
                    }
                    include
                });
            for entry in walker {
                if self.cancel.load(Ordering::SeqCst) {
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(error) => {
                        root_incomplete.store(true, Ordering::Relaxed);
                        self.stats.errors.fetch_add(1, Ordering::Relaxed);
                        root_stats.errors += 1;
                        root_stats.record_error(error.to_string());
                        continue;
                    }
                };
                let ft = entry.file_type();
                if ft.is_dir() {
                    self.stats.dirs.fetch_add(1, Ordering::Relaxed);
                    root_stats.dirs += 1;
                    continue;
                }
                if !ft.is_file() {
                    continue;
                }
                let path = entry.path();
                *self.stats.current.lock() = path.to_string_lossy().to_string();
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let kind = ext_kind(&ext);
                let accepted = match kind {
                    "image" => cfg.images,
                    "video" => cfg.videos,
                    "audio" => cfg.audios,
                    "document" => if ext == "pdf" { cfg.pdf } else { cfg.documents },
                    "text" => cfg.documents,
                    "archive" => cfg.archives,
                    _ => cfg.others,
                };
                if !accepted {
                    let indexed_path = crate::util::norm_path(&path.to_string_lossy());
                    if let Err(error) = mark_existing_seen(db, &indexed_path, &mut seen) {
                        root_incomplete.store(true, Ordering::Relaxed);
                        self.stats.errors.fetch_add(1, Ordering::Relaxed);
                        root_stats.errors += 1;
                        root_stats.record_error(format!("{}: {error}", path.display()));
                    }
                    self.stats.skipped.fetch_add(1, Ordering::Relaxed);
                    root_stats.skipped += 1;
                    continue;
                }
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(error) => {
                        root_incomplete.store(true, Ordering::Relaxed);
                        self.stats.errors.fetch_add(1, Ordering::Relaxed);
                        root_stats.errors += 1;
                        root_stats.record_error(format!("{}: {error}", entry.path().display()));
                        continue;
                    }
                };
                let size = meta.len();
                if size < cfg.min_size_kb * 1024 || size > cfg.max_size_mb * 1024 * 1024 {
                    let indexed_path = crate::util::norm_path(&path.to_string_lossy());
                    if let Err(error) = mark_existing_seen(db, &indexed_path, &mut seen) {
                        root_incomplete.store(true, Ordering::Relaxed);
                        self.stats.errors.fetch_add(1, Ordering::Relaxed);
                        root_stats.errors += 1;
                        root_stats.record_error(format!("{}: {error}", path.display()));
                    }
                    self.stats.skipped.fetch_add(1, Ordering::Relaxed);
                    root_stats.skipped += 1;
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

                let existed = match db.get_by_path(&row.path) {
                    Ok(existed) => existed,
                    Err(error) => {
                        root_incomplete.store(true, Ordering::Relaxed);
                        self.stats.errors.fetch_add(1, Ordering::Relaxed);
                        root_stats.errors += 1;
                        root_stats.record_error(format!("{}: {error}", row.path));
                        continue;
                    }
                };
                match existed {
                    Some(old) => {
                        if old.size == row.size && old.mtime == row.mtime && !old.path.is_empty() {
                            seen.insert(old.id);
                            if let Err(error) = db.update_fields(&old.path, &[("missing", "0".into())]) {
                                root_incomplete.store(true, Ordering::Relaxed);
                                self.stats.errors.fetch_add(1, Ordering::Relaxed);
                                root_stats.errors += 1;
                                root_stats.record_error(format!("{}: {error}", row.path));
                            }
                            self.stats.files.fetch_add(1, Ordering::Relaxed);
                            self.stats.bytes.fetch_add(size, Ordering::Relaxed);
                            root_stats.files += 1;
                            root_stats.bytes += size;
                            continue;
                        }
                        if let Err(error) = db.upsert_file(&row) {
                            root_incomplete.store(true, Ordering::Relaxed);
                            self.stats.errors.fetch_add(1, Ordering::Relaxed);
                            root_stats.errors += 1;
                            root_stats.record_error(format!("{}: {error}", row.path));
                            continue;
                        }
                        self.stats.updated.fetch_add(1, Ordering::Relaxed);
                        root_stats.updated += 1;
                    }
                    None => {
                        if let Err(error) = db.upsert_file(&row) {
                            root_incomplete.store(true, Ordering::Relaxed);
                            self.stats.errors.fetch_add(1, Ordering::Relaxed);
                            root_stats.errors += 1;
                            root_stats.record_error(format!("{}: {error}", row.path));
                            continue;
                        }
                        self.stats.added.fetch_add(1, Ordering::Relaxed);
                        root_stats.added += 1;
                    }
                }
                self.stats.files.fetch_add(1, Ordering::Relaxed);
                self.stats.bytes.fetch_add(size, Ordering::Relaxed);
                root_stats.files += 1;
                root_stats.bytes += size;
                match db.get_by_path(&row.path) {
                    Ok(Some(saved)) => {
                        seen.insert(saved.id);
                        batch.push(saved);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        root_incomplete.store(true, Ordering::Relaxed);
                        self.stats.errors.fetch_add(1, Ordering::Relaxed);
                        root_stats.errors += 1;
                        root_stats.record_error(format!("{}: {error}", row.path));
                    }
                }
                if batch.len() >= 400 {
                    on_batch(&batch);
                    batch.clear();
                }
            }
            let root_complete = !root_incomplete.load(Ordering::Relaxed)
                && !self.cancel.load(Ordering::SeqCst);
            if root_complete {
                complete_roots.insert(crate::util::norm_path(root).to_ascii_lowercase());
            }
            self.stats.roots.lock().push(root_stats);
        }
        if !batch.is_empty() {
            on_batch(&batch);
        }
        // 把「扫描根下、这次没遇到」的旧记录标成 missing。
        // 不这么做，删掉的文件会永远留在库里，统计越扫越大（用户看到「35 个文件」实际只有 16 个）。
        if !self.cancel.load(Ordering::SeqCst) {
            for root in cfg.roots.iter().filter(|root| !root_is_disabled(root, cfg)) {
                if !complete_roots.contains(&crate::util::norm_path(root).to_ascii_lowercase()) {
                    continue;
                }
                let prefix = crate::util::norm_path(root);
                let prefix = if prefix.ends_with('\\') {
                    prefix
                } else {
                    format!("{prefix}\\")
                };
                if let Ok(stale) = db.mark_missing_except(&prefix, &seen) {
                    if stale > 0 {
                        self.stats.missing.fetch_add(stale as u64, Ordering::Relaxed);
                        if let Some(root_stats) = self.stats.roots.lock().iter_mut().find(|stats| {
                            stats.root.eq_ignore_ascii_case(&crate::util::norm_path(root))
                        }) {
                            root_stats.missing += stale as u64;
                        }
                    }
                }
            }
        }
        self.stats
            .elapsed_millis
            .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        self.stats.current.lock().clear();
        self.stats.done.store(true, Ordering::SeqCst);
        self.stats.cancelled.store(self.cancel.load(Ordering::SeqCst), Ordering::SeqCst);
        let mut snap = self.snapshot();
        snap.seconds = started.elapsed().as_secs_f64();
        snap.current = String::new();
        snap
    }
}

fn mark_existing_seen(
    db: &Db,
    path: &str,
    seen: &mut std::collections::HashSet<i64>,
) -> Result<(), String> {
    if let Some(row) = db.get_by_path(path).map_err(|error| error.to_string())? {
        seen.insert(row.id);
        db.update_fields(&row.path, &[("missing", "0".into())])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn root_is_disabled(root: &str, cfg: &ScanConfig) -> bool {
    let root = crate::util::norm_path(root);
    cfg.disabled_roots
        .iter()
        .any(|disabled| crate::util::norm_path(disabled).eq_ignore_ascii_case(&root))
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::types::Value;
    use std::path::PathBuf;

    struct TestWorkspace(PathBuf);

    impl TestWorkspace {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "deepdark-oracle-scan-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn root(&self) -> PathBuf {
            let path = self.0.join("root");
            std::fs::create_dir_all(&path).unwrap();
            path
        }

        fn db(&self) -> Db {
            Db::open(&self.0.join("data").join("index.db")).unwrap()
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn config(root: &Path) -> ScanConfig {
        ScanConfig {
            roots: vec![root.to_string_lossy().to_string()],
            exclude_dirs: Vec::new(),
            ..ScanConfig::default()
        }
    }

    fn scan(db: &Db, cfg: &ScanConfig) -> ScanStats {
        Scanner::new().run(db, cfg, |_| {})
    }

    fn is_active(db: &Db, path: &Path) -> bool {
        let path = crate::util::norm_path(&path.to_string_lossy());
        !db.query_rows(
            "SELECT id,path,dir,name,ext,kind,size,mtime,ctime,text_len,text_state,embed_state,thumb_key,category,tags FROM files WHERE path=?1 AND missing=0",
            vec![Value::from(path)],
        )
        .is_empty()
    }

    #[test]
    fn filtered_files_remain_indexed() {
        let workspace = TestWorkspace::new();
        let root = workspace.root();
        let image = root.join("photo.png");
        std::fs::write(&image, b"image").unwrap();
        let db = workspace.db();
        let mut cfg = config(&root);
        scan(&db, &cfg);
        assert!(is_active(&db, &image));

        cfg.images = false;
        scan(&db, &cfg);
        assert!(is_active(&db, &image));
    }

    #[test]
    fn excluded_subtree_is_not_pruned() {
        let workspace = TestWorkspace::new();
        let root = workspace.root();
        let private_dir = root.join("private");
        std::fs::create_dir_all(&private_dir).unwrap();
        let image = private_dir.join("photo.png");
        std::fs::write(&image, b"image").unwrap();
        let db = workspace.db();
        let mut cfg = config(&root);
        scan(&db, &cfg);
        assert!(is_active(&db, &image));

        cfg.exclude_dirs.push("private".into());
        scan(&db, &cfg);
        assert!(is_active(&db, &image));
    }

    #[test]
    fn restored_unchanged_file_is_marked_present() {
        let workspace = TestWorkspace::new();
        let root = workspace.root();
        let image = root.join("photo.png");
        std::fs::write(&image, b"image").unwrap();
        let db = workspace.db();
        let cfg = config(&root);
        scan(&db, &cfg);
        assert!(is_active(&db, &image));

        db.mark_missing(&crate::util::norm_path(&image.to_string_lossy()))
            .unwrap();
        assert!(!is_active(&db, &image));
        scan(&db, &cfg);
        assert!(is_active(&db, &image));
    }

    #[test]
    fn removed_file_is_pruned_after_complete_scan() {
        let workspace = TestWorkspace::new();
        let root = workspace.root();
        let image = root.join("photo.png");
        std::fs::write(&image, b"image").unwrap();
        let db = workspace.db();
        let cfg = config(&root);
        scan(&db, &cfg);
        std::fs::remove_file(&image).unwrap();

        let stats = scan(&db, &cfg);
        assert!(!is_active(&db, &image));
        assert_eq!(stats.missing, 1);
    }

    #[test]
    fn cancellation_before_run_is_preserved() {
        let workspace = TestWorkspace::new();
        let root = workspace.root();
        std::fs::write(root.join("photo.png"), b"image").unwrap();
        let db = workspace.db();
        let scanner = Scanner::new();
        scanner.cancel();

        let stats = scanner.run(&db, &config(&root), |_| {});
        assert!(stats.cancelled);
        assert_eq!(stats.files, 0);
    }
}
