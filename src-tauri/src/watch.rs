//! 实时文件监控：notify 6 + 去抖合并。
use crate::db::Db;
use crate::util::ext_kind;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, serde::Serialize)]
pub struct WatchEvent {
    pub kind: String, // created | modified | deleted | renamed
    pub path: String,
    pub dest: Option<String>,
}

pub struct FileWatcher {
    inner: Mutex<Option<notify::RecommendedWatcher>>,
    pending: Arc<Mutex<HashMap<String, WatchEvent>>>,
    last_flush: Arc<Mutex<Instant>>,
    running: AtomicBool,
    pub on_batch: Arc<Mutex<Option<Box<dyn FnMut(Vec<WatchEvent>) + Send>>>>,
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FileWatcher {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            last_flush: Arc::new(Mutex::new(Instant::now())),
            running: AtomicBool::new(false),
            on_batch: Arc::new(Mutex::new(None)),
        }
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn start(&self, roots: &[String], debounce_secs: f64, db: Arc<Db>) -> Result<usize, String> {
        self.stop();
        let pending = Arc::clone(&self.pending);
        let last = Arc::clone(&self.last_flush);
        let on_batch = Arc::clone(&self.on_batch);
        let mut w = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(ev) = res else { return };
            let kind = match ev.kind {
                EventKind::Create(_) => "created",
                EventKind::Remove(_) => "deleted",
                EventKind::Modify(notify::event::ModifyKind::Name(_)) => "renamed",
                EventKind::Modify(_) => "modified",
                _ => return,
            };
            for p in ev.paths {
                let path = p.to_string_lossy().to_string();
                let ext = PathBuf::from(&path)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
                    .unwrap_or_default();
                let k = ext_kind(&ext);
                if k == "other" {
                    continue;
                }
                let mut pend = pending.lock();
                pend.insert(
                    path.clone(),
                    WatchEvent { kind: kind.to_string(), path, dest: None },
                );
                let mut lf = last.lock();
                if lf.elapsed() >= Duration::from_millis((debounce_secs * 1000.0) as u64) {
                    *lf = Instant::now();
                    let batch: Vec<WatchEvent> = pend.drain().map(|(_, v)| v).collect();
                    drop(pend);
                    if !batch.is_empty() {
                        if let Some(cb) = on_batch.lock().as_mut() {
                            cb(batch);
                        }
                    }
                }
            }
        })
        .map_err(|e| format!("监控初始化失败: {e}"))?;

        let mut n = 0;
        for r in roots {
            if std::path::Path::new(r).exists() {
                if w.watch(std::path::Path::new(r), RecursiveMode::Recursive).is_ok() {
                    n += 1;
                }
            }
        }
        *self.inner.lock() = Some(w);
        self.running.store(true, Ordering::Relaxed);
        let _ = db;
        Ok(n)
    }

    pub fn stop(&self) {
        *self.inner.lock() = None;
        self.running.store(false, Ordering::Relaxed);
    }

    pub fn flush_now(&self) -> Vec<WatchEvent> {
        let mut pend = self.pending.lock();
        let batch: Vec<WatchEvent> = pend.drain().map(|(_, v)| v).collect();
        batch
    }
}
