//! 后台索引器：文本抽取 → 缩略图 → 向量化。
use crate::ai::{embed_path, Embedder};
use crate::config::IndexConfig;
use crate::db::Db;
use crate::media;
use crate::vector::VectorStore;
use parking_lot::Mutex;
use rayon::prelude::*;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

const VECTOR_CHECKPOINT_ITEMS: usize = 512;
const ACTIVE_FILE_DISPLAY_LIMIT: usize = 5;

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct IndexStats {
    pub text_done: u64,
    pub text_fail: u64,
    pub emb_done: u64,
    pub emb_fail: u64,
    pub thumb_done: u64,
    /// 跨模态向量（CLIP 图片 / CLAP 音频）
    pub cross_done: u64,
    pub cross_fail: u64,
    pub queued: u64,
    /// Current indexing batch size and completed file count.
    pub total: u64,
    pub completed: u64,
    /// Set as soon as the user cancels, and cleared by the next enqueue.
    pub cancelled: bool,
    pub running: bool,
    pub current: Option<String>,
    /// Files currently included in the active model batch. The UI shows this
    /// list so multi-instance work is visible without exposing a counter.
    pub current_items: Vec<String>,
    pub last_error: Option<String>,
}

pub struct Indexer {
    db: Arc<Db>,
    /// 文本向量空间（llama.cpp GGUF 或 hash 基线）
    vectors: Arc<VectorStore>,
    /// 图片/音频/视频向量空间（hash 特征，维度与文本空间独立）
    media_vectors: Arc<VectorStore>,
    /// 跨模态图片空间（CLIP 512 维），图片与视频关键帧
    clip_vectors: Arc<VectorStore>,
    /// 跨模态音频空间（CLAP 512 维）
    audio_vectors: Arc<VectorStore>,
    /// 跨模态模型（缺模型时为 None，功能静默关闭）
    clip: parking_lot::RwLock<Option<Arc<crate::ai::clip::ClipModel>>>,
    clap: parking_lot::RwLock<Option<Arc<crate::ai::clap::ClapModel>>>,
    embedder: Arc<dyn Embedder>,
    /// 媒体兜底编码器（始终是 hash）
    media_embedder: Arc<dyn Embedder>,
    cfg: IndexConfig,
    batch_size: AtomicU64,
    cpu_pool: parking_lot::RwLock<Arc<rayon::ThreadPool>>,
    progress_what: Mutex<String>,
    /// Scope of the currently requested indexing operation. Keeping this on
    /// the worker prevents progress for one scan root from being reported as
    /// progress for every file in the database.
    progress_roots: Mutex<Option<Vec<String>>>,
    queue: Mutex<VecDeque<(i64, String)>>, // (file_id, what)
    stats: Mutex<IndexStats>,
    stop: AtomicBool,
    cancel_requested: AtomicBool,
    resume_pending: AtomicBool,
    cancel_checkpointed: AtomicBool,
    busy: AtomicBool,
    active: AtomicU64,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    thumb_dir: PathBuf,
    /// 距上次向量库落盘的插入条数（批量落盘，避免每条都全量写盘）
    dirty: AtomicU64,
    /// Vector states are acknowledged in SQLite only after the corresponding
    /// vector files have been durably replaced. This prevents an interrupted
    /// build from leaving `embed_state=ok` without a persisted vector.
    pending_states: Mutex<Vec<i64>>,
    last_checkpoint_error_log: AtomicU64,
}

impl Indexer {
    #[inline]
    fn is_cancelled(&self) -> bool {
        self.cancel_requested.load(Ordering::Relaxed) || self.stop.load(Ordering::Relaxed)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<Db>,
        vectors: Arc<VectorStore>,
        media_vectors: Arc<VectorStore>,
        clip_vectors: Arc<VectorStore>,
        audio_vectors: Arc<VectorStore>,
        embedder: Arc<dyn Embedder>,
        media_embedder: Arc<dyn Embedder>,
        cfg: IndexConfig,
        thumb_dir: PathBuf,
    ) -> Self {
        let configured_batch_size = cfg.batch_size.clamp(1, 128) as u64;
        let cpu_pool = Self::build_cpu_pool(configured_batch_size as usize);
        let progress_what = db
            .get_meta("index_progress_what")
            .ok()
            .flatten()
            .filter(|value| matches!(value.as_str(), "auto" | "text" | "embed"))
            .unwrap_or_else(|| "auto".into());
        Self {
            db,
            vectors,
            media_vectors,
            clip_vectors,
            audio_vectors,
            clip: parking_lot::RwLock::new(None),
            clap: parking_lot::RwLock::new(None),
            embedder,
            media_embedder,
            cfg,
            batch_size: AtomicU64::new(configured_batch_size),
            cpu_pool: parking_lot::RwLock::new(cpu_pool),
            progress_what: Mutex::new(progress_what),
            progress_roots: Mutex::new(None),
            queue: Mutex::new(VecDeque::new()),
            stats: Mutex::new(IndexStats::default()),
            stop: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            resume_pending: AtomicBool::new(false),
            cancel_checkpointed: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            active: AtomicU64::new(0),
            worker: Mutex::new(None),
            thumb_dir,
            dirty: AtomicU64::new(0),
            pending_states: Mutex::new(Vec::new()),
            last_checkpoint_error_log: AtomicU64::new(0),
        }
    }

    /// 挂上跨模态模型（换模型后要重新挂）。
    pub fn set_cross_modal(
        &self,
        clip: Option<Arc<crate::ai::clip::ClipModel>>,
        clap: Option<Arc<crate::ai::clap::ClapModel>>,
    ) {
        *self.clip.write() = clip;
        *self.clap.write() = clap;
    }

    pub fn snapshot(&self) -> IndexStats {
        let mut s = self.stats.lock().clone();
        s.queued = self.queue.lock().len() as u64;
        s.running = self.busy.load(Ordering::Relaxed);
        if !s.running && s.queued == 0 {
            let what = self.progress_what.lock().clone();
            if let Ok((total, completed)) = self.progress(&what) {
                s.total = total.max(0) as u64;
                let in_memory = if what == "text" {
                    0
                } else {
                    self.pending_states.lock().len() as i64
                };
                s.completed = completed
                    .saturating_add(in_memory)
                    .max(0)
                    .min(total.max(0)) as u64;
            }
        }
        s
    }

    pub fn set_progress_scope(&self, roots: Option<Vec<String>>) {
        let normalized = roots.map(|items| {
            items
                .into_iter()
                .map(|root| crate::util::norm_path(&root))
                .filter(|root| !root.is_empty())
                .collect::<Vec<_>>()
        }).filter(|items| !items.is_empty());
        *self.progress_roots.lock() = normalized;
    }

    fn progress(&self, what: &str) -> rusqlite::Result<(i64, i64)> {
        let roots = self.progress_roots.lock().clone();
        match roots {
            Some(roots) => self.db.index_progress_in_roots(what, &roots),
            None => self.db.index_progress(what),
        }
    }

    fn set_current_items(&self, names: &[String], lanes: usize) {
        if names.is_empty() {
            return;
        }
        // The list represents model lanes, not the whole native batch. A
        // single model may receive many documents in one call, but only one
        // lane should be shown; replicated models show one name per replica.
        let count = lanes.clamp(1, ACTIVE_FILE_DISPLAY_LIMIT).min(names.len());
        let active: Vec<String> = (0..count)
            .map(|lane| names[lane * names.len() / count].clone())
            .collect();
        let mut stats = self.stats.lock();
        stats.current = active.first().cloned();
        stats.current_items = active;
    }

    fn clear_current_batch(&self) {
        let mut stats = self.stats.lock();
        stats.current = None;
        stats.current_items.clear();
    }

    pub fn backlog(&self) -> usize {
        self.queue.lock().len()
    }

    pub fn set_batch_size(&self, batch_size: usize) {
        let batch_size = batch_size.clamp(1, 128);
        self.batch_size.store(batch_size as u64, Ordering::Relaxed);
        *self.cpu_pool.write() = Self::build_cpu_pool(batch_size);
    }

    fn build_cpu_pool(batch_size: usize) -> Arc<rayon::ThreadPool> {
        let cores = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .max(1);
        let threads = if batch_size <= 1 {
            cores.div_ceil(3).max(1)
        } else if batch_size >= 96 {
            cores
        } else {
            cores.div_ceil(2).max(1)
        };
        Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap_or_else(|_| {
                    rayon::ThreadPoolBuilder::new()
                        .num_threads(1)
                        .build()
                        .expect("single-thread Rayon pool should initialize")
                }),
        )
    }

    pub fn enqueue(&self, ids: Vec<i64>, what: &str) {
        *self.progress_what.lock() = what.to_string();
        let _ = self.db.set_meta("index_progress_what", what);
        let resume = self.resume_pending.swap(false, Ordering::Relaxed);
        self.cancel_checkpointed.store(false, Ordering::Relaxed);
        let mut q = self.queue.lock();
        let batch_total = ids.len() as u64;
        for id in ids {
            q.push_back((id, what.to_string()));
        }
        self.active.store(q.len() as u64, Ordering::Relaxed);
        let mut stats = self.stats.lock();
        stats.cancelled = false;
        if resume && stats.total > stats.completed {
            // 取消后只把仍处于 pending 的文件重新入队。保留原批次的
            // completed/total，让 UI 从上次检查点继续显示，而不是回到 0%。
            stats.total = stats.total.max(stats.completed.saturating_add(batch_total));
        } else {
            stats.total = batch_total;
            stats.completed = 0;
            stats.text_done = 0;
            stats.text_fail = 0;
            stats.emb_done = 0;
            stats.emb_fail = 0;
            stats.thumb_done = 0;
            stats.cross_done = 0;
            stats.cross_fail = 0;
        }
        if let Ok((total, completed)) = self.progress(what) {
            stats.total = total.max(0) as u64;
            stats.completed = completed.max(0).min(total.max(0)) as u64;
        }
        stats.last_error = None;
    }

    pub fn reset_progress(&self) {
        let mut stats = self.stats.lock();
        stats.total = 0;
        stats.completed = 0;
        stats.text_done = 0;
        stats.text_fail = 0;
        stats.emb_done = 0;
        stats.emb_fail = 0;
        stats.thumb_done = 0;
        stats.cross_done = 0;
        stats.cross_fail = 0;
    }

    /// 把库里所有 pending 的文件入队。
    pub fn enqueue_all_pending(&self, what: &str) -> usize {
        let ids = self.db.pending_ids_active(what, 100_000).unwrap_or_default();
        let n = ids.len();
        self.enqueue(ids, what);
        n
    }

    /// 同上，但最多只排 `limit` 条（启动阶段用，避免积压几十万条时一开机就满载）。
    pub fn enqueue_all_pending_limited(&self, what: &str, limit: usize) -> usize {
        let ids = self.db.pending_ids_active(what, limit).unwrap_or_default();
        let n = ids.len();
        self.enqueue(ids, what);
        n
    }

    /// 启动后台线程（幂等）。
    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let handle = std::thread::spawn(move || this.run_loop());
        *self.worker.lock() = Some(handle);
    }

    fn run_loop(&self) {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            if self.cancel_requested.load(Ordering::Relaxed) {
                self.active.store(self.queue.lock().len() as u64, Ordering::Relaxed);
                self.clear_current_batch();
                if !self.cancel_checkpointed.swap(true, Ordering::Relaxed) {
                    if let Err(error) = self.checkpoint() {
                        self.log_checkpoint_error("索引取消时保存失败", &error);
                    }
                }
                self.busy.store(false, Ordering::Relaxed);
                self.cancel_requested.store(false, Ordering::Relaxed);
                continue;
            }
            let item = {
                let mut q = self.queue.lock();
                q.pop_front()
            };
            let Some(first) = item else {
                // 队列空了：把攒着的向量落盘
                self.flush();
                std::thread::sleep(std::time::Duration::from_millis(150));
                continue;
            };
            self.busy.store(true, Ordering::Relaxed);
            let mut batch = vec![first];
            let first_pdf = self.db.get_by_id(batch[0].0).ok().flatten().map(|row| {
                row.ext.eq_ignore_ascii_case("pdf")
            }).unwrap_or(false);
            let first_text_like = self.db.get_by_id(batch[0].0).ok().flatten().map(|row| {
                matches!(row.kind.as_str(), "text" | "document")
                    && !row.ext.eq_ignore_ascii_case("pdf")
            }).unwrap_or(false);
            let first_image = self.db.get_by_id(batch[0].0).ok().flatten().filter(|row| {
                row.kind == "image"
                    && self.embedder.info().modalities.iter().any(|m| m == "image")
            });
            let batchable = first_text_like || first_image.is_some() || first_pdf;
            let configured_limit = self.batch_size.load(Ordering::Relaxed).clamp(1, 128) as usize;
            // A PDF can contribute several decoded 1024px page images. Keep
            // its flattened image batch bounded so turbo mode cannot retain
            // gigabytes of raster data while still processing multiple files.
            let batch_limit = if first_pdf { configured_limit.min(8) } else { configured_limit };
            if batch_limit > 1
                && batchable
                && matches!(batch[0].1.as_str(), "auto" | "embed" | "text")
            {
                while batch.len() < batch_limit {
                    let next = {
                        let mut q = self.queue.lock();
                        let position = q.iter().position(|(fid, what)| {
                            if what != &batch[0].1 {
                                return false;
                            }
                            self.db.get_by_id(*fid).ok().flatten().map(|row| {
                                if first_image.is_some() {
                                    row.kind == "image"
                                } else if first_pdf {
                                    row.ext.eq_ignore_ascii_case("pdf")
                                } else {
                                    matches!(row.kind.as_str(), "text" | "document")
                                        && !row.ext.eq_ignore_ascii_case("pdf")
                                }
                            }).unwrap_or(false)
                        });
                        position.and_then(|position| q.remove(position))
                    };
                    let Some(next) = next else { break };
                    batch.push(next);
                }
            }
            let r = if first_pdf {
                self.process_pdf_batch(&batch, &batch[0].1)
            } else if first_image.is_some() {
                self.process_image_batch(&batch)
            } else if batch.len() > 1 {
                self.process_text_batch_parallel(&batch, &batch[0].1)
            } else {
                self.process_one(batch[0].0, &batch[0].1)
            };
            self.busy.store(false, Ordering::Relaxed);
            self.active.store(self.queue.lock().len() as u64, Ordering::Relaxed);
            self.clear_current_batch();
            if let Err(e) = r {
                if !self.is_cancelled() {
                    let mut s = self.stats.lock();
                    s.last_error = Some(e.clone());
                    drop(s);
                    let _ = self.db.log("error", &e);
                }
            }
            if self.pending_states.lock().len() >= VECTOR_CHECKPOINT_ITEMS
                || self.dirty.load(Ordering::Relaxed) >= VECTOR_CHECKPOINT_ITEMS as u64
                || self.queue.lock().is_empty()
            {
                if let Err(error) = self.checkpoint() {
                    self.stats.lock().last_error = Some(error.clone());
                    self.log_checkpoint_error("索引检查点保存失败", &error);
                }
            }
            let progress_what = self.progress_what.lock().clone();
            if let Ok((total, completed)) = self.progress(&progress_what) {
                let mut stats = self.stats.lock();
                stats.total = total.max(0) as u64;
                let in_memory = if progress_what == "text" {
                    0
                } else {
                    self.pending_states.lock().len() as i64
                };
                stats.completed = completed
                    .saturating_add(in_memory)
                    .max(0)
                    .min(total.max(0)) as u64;
            }
            // 每处理 200 条记一次心跳，发布版崩溃时能靠它判断「死在哪一步」。
            let done = self.dirty.load(Ordering::Relaxed);
            if done > 0 && done % 200 == 0 {
                let _ = self.db.log("index", &format!("索引进度：已处理 {done} 条"));
            }
        }
    }

    fn process_one(&self, fid: i64, what: &str) -> Result<(), String> {
        let row = self
            .db
            .get_by_id(fid)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("文件 {fid} 不在库中"))?;
        if row.path.is_empty() || !Path::new(&row.path).exists() {
            // 文件已经不在磁盘上了。必须标记掉，否则它永远是 pending，
            // 每次启动都会重新排进队列 —— 库里有几万条这种"幽灵记录"时，
            // 程序会一开机就满载空转，用户以为卡死。
            let _ = self.db.mark_missing(&row.path);
            return Ok(());
        }
        self.set_current_items(std::slice::from_ref(&row.name), 1);

        // 1. 文本
        // auto 模式下这条可能是「正文已建、向量没建」的补索引，重新抽一遍纯属浪费。
        let text_pending = row.text_state == "pending" || row.text_state == "fail";
        if (what == "text" || what == "auto") && text_pending {
            // 之前的判定写成 "text" | "doc"，但 ext_kind() 返回的是 "document"，
            // 结果所有 PDF/DOCX/XLSX 都被当成「不需要抽正文」直接 skip，
            // 用户搜正文永远搜不到 —— 全文检索形同虚设。
            let want_text = matches!(row.kind.as_str(), "text" | "document");
            if want_text {
                match crate::textract::extract_text(Path::new(&row.path), self.cfg.max_text_chars) {
                    Some(text) if !text.trim().is_empty() => {
                        let body = crate::util::index_text(&text);
                        let _ = self.db.set_doc(fid, &crate::util::index_text(&row.name), &body, &text);
                        self.stats.lock().text_done += 1;
                    }
                    // 抽取成功但正文为空（扫描版 PDF / 空表格）：标 skip，
                    // 否则它永远 pending，每次启动都重新排一遍。
                    _ => {
                        let _ = self.db.set_state(fid, "text", "skip");
                        self.stats.lock().text_fail += 1;
                    }
                }
            } else {
                let _ = self.db.set_state(fid, "text", "skip");
            }
        }

        // 2. 缩略图
        if matches!(row.kind.as_str(), "image" | "video")
            || (row.kind == "document"
                && matches!(row.ext.to_ascii_lowercase().as_str(), "pdf" | "docx"))
        {
            let dest = self.thumb_dir.join(format!("{fid}.jpg"));
            if !dest.exists() {
                let _ = std::fs::create_dir_all(&self.thumb_dir);
                let made = if matches!(row.kind.as_str(), "image" | "video") {
                    media::make_thumb(Path::new(&row.path), &dest, self.cfg.image_max_side)
                } else {
                    media::make_document_thumb(
                        Path::new(&row.path),
                        &dest,
                        self.cfg.image_max_side,
                    )
                };
                if made {
                    let _ = self.db.set_thumb(fid, &format!("{fid}.jpg"));
                    self.stats.lock().thumb_done += 1;
                }
            }

            // 2.5 视觉标签：给图片/视频打中文关键词（蓝天/草地/雪景/夜景…）。
            //
            // 为什么不用向量：llama.cpp 只跑文本塔，视觉塔输出与文本向量
            // 不在同一空间（实测图文余弦 0.03~0.09，毫无区分度），做不了
            // 「一句话描述找图」。颜色+构图统计虽然覆盖面窄，但描述准确、
            // 零幻觉、可解释，能让用户用日常词把图找出来。
            //
            // 分析缩略图而不是原图：缩略图已经缩放好（几百像素），
            // 又快又稳；视频也能复用抽出来的中间帧。
            if matches!(row.kind.as_str(), "image" | "video") && row.tags.is_none() {
                let src = if dest.exists() {
                    dest.as_path()
                } else {
                    Path::new(&row.path)
                };
                let tags = crate::vision::describe_file(src, &row.kind);
                if !tags.is_empty() {
                    let joined = tags.join(",");
                    let _ = self.db.update_fields(&row.path, &[("tags", joined)]);
                }
            }
        }

        // PDF extraction is shared by the text and vector stages. This avoids
        // reopening the same ZIP/object streams and gives the multimodal
        // encoder exactly the images that were discovered for this item.
        let is_pdf = row.ext.eq_ignore_ascii_case("pdf");
        let (pdf_text, pdf_images) = if is_pdf && (what == "embed" || what == "auto") {
            let text = self
                .db
                .get_doc_raw(fid)
                .ok()
                .flatten()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    crate::textract::extract_text(Path::new(&row.path), self.cfg.max_text_chars)
                        .filter(|value| !value.trim().is_empty())
                });
            let images = crate::textract::pdf_images_with_cancel(
                Path::new(&row.path),
                8,
                || self.is_cancelled(),
            );
            (text, images)
        } else {
            (None, Vec::new())
        };

        // 3. 向量
        //
        // 文本类走主 embedding 空间（llama.cpp GGUF，512 维）；
        // 图片/音频/视频走 hash 空间（256 维）——llama.cpp 只处理文本，
        // 两个空间维度不同，必须分开存，否则向量库会错位。
        if what == "embed" || what == "auto" {
            let is_media = matches!(row.kind.as_str(), "image" | "audio" | "video");
            let modality = match row.kind.as_str() {
                "image" => "image",
                "audio" => "audio",
                "video" => "video",
                _ => "",
            };
            let primary_supports_media = is_media
                && self.embedder.info().modalities.iter().any(|item| item == modality);
            let (embedder, store) = if is_media && !primary_supports_media {
                (
                    Arc::clone(&self.media_embedder),
                    Arc::clone(&self.media_vectors),
                )
            } else {
                (Arc::clone(&self.embedder), Arc::clone(&self.vectors))
            };
            let embedded = if is_pdf {
                crate::ai::embed_pdf_parts(embedder.as_ref(), pdf_text.as_deref(), &pdf_images)
            } else {
                embed_path(embedder.as_ref(), Path::new(&row.path), &row.kind)
            };
            match embedded {
                Ok(vec) => {
                    store.add(fid, &vec);
                    self.pending_states.lock().push(fid);
                    // 批量落盘：向量库是全量写盘（几百 MB 量级），每条都 save()
                    // 会让硬盘狂转、索引速度掉到个位数。每 64 条或队列空闲时再写。
                    self.dirty.fetch_add(1, Ordering::Relaxed);
                    self.stats.lock().emb_done += 1;
                }
                Err(e) => {
                    // A scan-only PDF with unsupported image filters is a
                    // valid, permanently empty item. Keep it out of retry
                    // queues and report it as skipped rather than as a model
                    // failure on every subsequent indexing pass.
                    if is_pdf && e.contains("PDF") {
                        let _ = self.db.set_state(fid, "embed", "skip");
                    } else {
                        let _ = self.db.set_state(fid, "embed", "fail");
                        self.stats.lock().emb_fail += 1;
                        return Err(format!("{}: {e}", row.name));
                    }
                }
            }
        }

        // 4. 跨模态（CLIP / CLAP）：把图片和音频编码到「和文字同一空间」，
        //    这样「一句话描述找图片」才成立。hash 向量只描述颜色/能量分布，
        //    跟文字八竿子打不着，不能拿来给文字查询用。
        self.embed_cross_modal(&row, if is_pdf { Some(&pdf_images) } else { None });

        Ok(())
    }

    /// Process a PDF batch with parallel extraction and flattened model batches.
    /// Text and page images are extracted independently, then fused back into
    /// one primary vector per PDF in the same order as the queue.
    fn process_pdf_batch(&self, batch: &[(i64, String)], what: &str) -> Result<(), String> {
        let pdf_started = std::time::Instant::now();
        let _ = self.db.log("index", &format!("PDF batch start: {} files", batch.len()));
        struct Candidate {
            id: i64,
            name: String,
            path: PathBuf,
            cached_text: Option<String>,
            needs_extract: bool,
            text_pending: bool,
        }
        struct Item {
            id: i64,
            name: String,
            text: Option<String>,
            images: Vec<image::DynamicImage>,
        }

        let mut candidates = Vec::with_capacity(batch.len());
        let mut errors = Vec::new();
        for (id, _) in batch {
            if self.is_cancelled() {
                return Err("index cancellation requested".into());
            }
            let row = match self.db.get_by_id(*id) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    errors.push(format!("file {id} is missing from the database"));
                    continue;
                }
                Err(error) => {
                    errors.push(format!("failed to read file {id}: {error}"));
                    continue;
                }
            };
            if row.path.is_empty() || !Path::new(&row.path).exists() {
                let _ = self.db.mark_missing(&row.path);
                continue;
            }
            let text_pending = row.text_state == "pending" || row.text_state == "fail";
            let cached_text = self.db.get_doc_raw(*id).ok().flatten();
            candidates.push(Candidate {
                id: *id,
                name: row.name,
                path: PathBuf::from(row.path),
                needs_extract: ((what == "text" || what == "auto") && text_pending)
                    || (what != "text" && cached_text.is_none()),
                cached_text,
                text_pending,
            });
        }

        let names: Vec<String> = candidates.iter().map(|candidate| candidate.name.clone()).collect();
        self.set_current_items(&names, self.embedder.concurrency());
        let max_chars = self.cfg.max_text_chars;
        let pool = self.cpu_pool.read().clone();
        let extract_started = std::time::Instant::now();
        let extracted: Vec<(Option<String>, Vec<image::DynamicImage>)> = pool.install(|| {
            candidates
                .par_iter()
                .map(|candidate| {
                    if self.is_cancelled() {
                        return (None, Vec::new());
                    }
                    let text = if candidate.needs_extract {
                        crate::textract::extract_text(&candidate.path, max_chars)
                    } else {
                        None
                    };
                    let images = if what == "text" {
                        Vec::new()
                    } else {
                        crate::textract::pdf_images_with_cancel(
                            &candidate.path,
                            8,
                            || self.is_cancelled(),
                        )
                    };
                    (text, images)
                })
                .collect()
        });

        let extracted_images: usize = extracted.iter().map(|(_, images)| images.len()).sum();
        let _ = self.db.log(
            "index",
            &format!(
                "PDF extract finished: {} files, {} images, {:.2}s",
                extracted.len(),
                extracted_images,
                extract_started.elapsed().as_secs_f64()
            ),
        );

        if self.is_cancelled() {
            return Err("index cancellation requested".into());
        }

        if what == "text" {
            for (candidate, (text, _)) in candidates.into_iter().zip(extracted) {
                if self.is_cancelled() {
                    return Err("index cancellation requested".into());
                }
                if !candidate.text_pending {
                    continue;
                }
                match text.filter(|value| !value.trim().is_empty()) {
                    Some(text) => {
                        let body = crate::util::index_text(&text);
                        let _ = self.db.set_doc(
                            candidate.id,
                            &crate::util::index_text(&candidate.name),
                            &body,
                            &text,
                        );
                        self.stats.lock().text_done += 1;
                    }
                    None => {
                        let _ = self.db.set_state(candidate.id, "text", "skip");
                        self.stats.lock().text_fail += 1;
                    }
                }
            }
            return if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) };
        }

        let mut items = Vec::with_capacity(candidates.len());
        for (candidate, (extracted_text, images)) in candidates.into_iter().zip(extracted) {
            let text = extracted_text
                .clone()
                .or_else(|| candidate.cached_text.clone())
                .map(|value| value.chars().take(max_chars).collect::<String>())
                .filter(|value| !value.trim().is_empty());

            if what == "auto" && candidate.text_pending {
                if let Some(value) = extracted_text.as_deref().filter(|value| !value.trim().is_empty()) {
                    let body = crate::util::index_text(value);
                    let _ = self.db.set_doc(
                        candidate.id,
                        &crate::util::index_text(&candidate.name),
                        &body,
                        value,
                    );
                    self.stats.lock().text_done += 1;
                } else {
                    let _ = self.db.set_state(candidate.id, "text", "skip");
                    self.stats.lock().text_fail += 1;
                }
            }

            items.push(Item {
                id: candidate.id,
                name: candidate.name,
                text,
                images,
            });
        }

        let parts: Vec<(Option<String>, &[image::DynamicImage])> = items
            .iter()
            .map(|item| (item.text.clone(), item.images.as_slice()))
            .collect();
        if self.is_cancelled() {
            return Err("index cancellation requested".into());
        }
        let mut errors = Vec::new();
        let embed_started = std::time::Instant::now();
        crate::ai::embed_pdf_parts_batch_with(
            self.embedder.as_ref(),
            &parts,
            || self.is_cancelled(),
            |index, vector| {
                if self.is_cancelled() {
                    return;
                }
                let item = &items[index];
                match vector {
                    Ok(vector) => {
                        self.vectors.add(item.id, &vector);
                        self.pending_states.lock().push(item.id);
                        self.dirty.fetch_add(1, Ordering::Relaxed);
                        self.stats.lock().emb_done += 1;
                        if let Ok(Some(row)) = self.db.get_by_id(item.id) {
                            self.embed_cross_modal(&row, Some(&item.images));
                        }
                    }
                    Err(error) if error.contains("PDF") => {
                        let _ = self.db.set_state(item.id, "embed", "skip");
                        if let Ok(Some(row)) = self.db.get_by_id(item.id) {
                            self.embed_cross_modal(&row, Some(&item.images));
                        }
                    }
                    Err(error) => {
                        let _ = self.db.set_state(item.id, "embed", "fail");
                        self.stats.lock().emb_fail += 1;
                        errors.push(format!("{}: {error}", item.name));
                    }
                }
            },
        )?;
        let _ = self.db.log(
            "index",
            &format!(
                "PDF embedding finished: {} files, {:.2}s",
                parts.len(),
                embed_started.elapsed().as_secs_f64()
            ),
        );
        let _ = self.db.log(
            "index",
            &format!(
                "PDF batch finished: {} files, {:.2}s",
                parts.len(),
                pdf_started.elapsed().as_secs_f64()
            ),
        );
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }

    /// Cross-modal vectorization. Missing models and unsupported files are skipped.
    fn process_image_batch(&self, batch: &[(i64, String)]) -> Result<(), String> {
        struct Item {
            id: i64,
            name: String,
            path: PathBuf,
        }
        let mut items = Vec::with_capacity(batch.len());
        let mut errors = Vec::new();
        for (id, _) in batch {
            let row = match self.db.get_by_id(*id) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    errors.push(format!("文件 {id} 不在数据库中"));
                    continue;
                }
                Err(error) => {
                    errors.push(format!("读取文件 {id} 失败：{error}"));
                    continue;
                }
            };
            if row.path.is_empty() || !Path::new(&row.path).exists() {
                let _ = self.db.mark_missing(&row.path);
                continue;
            }
            if let Err(error) = self.process_one(*id, "text") {
                errors.push(error);
                continue;
            }
            items.push(Item {
                id: *id,
                name: row.name,
                path: PathBuf::from(row.path),
            });
        }
        if items.is_empty() {
            return Ok(());
        }

        let names: Vec<String> = items.iter().map(|item| item.name.clone()).collect();
        self.set_current_items(&names, self.embedder.concurrency());
        if self.is_cancelled() {
            return Err("index cancellation requested".into());
        }
        let paths: Vec<&Path> = items.iter().map(|item| item.path.as_path()).collect();
        let vectors: Vec<Result<Vec<f32>, String>> = match self.embedder.embed_image_batch(&paths) {
            Ok(vectors) if vectors.len() == items.len() => vectors.into_iter().map(Ok).collect(),
            batch_result => {
                let reason = match batch_result {
                    Ok(vectors) => format!("批量图片向量数量不匹配：{} / {}", vectors.len(), items.len()),
                    Err(error) => error,
                };
                let _ = self.db.log("error", &format!("图片批量编码失败，改为逐个处理：{reason}"));
                let mut fallback = Vec::with_capacity(items.len());
                for item in &items {
                    if self.is_cancelled() {
                        return Err("index cancellation requested".into());
                    }
                    fallback.push(self.embedder.embed_image(&item.path));
                }
                fallback
            }
        };
        if self.is_cancelled() {
            return Err("index cancellation requested".into());
        }
        for (item, vector) in items.into_iter().zip(vectors) {
            match vector {
                Ok(vector) => {
                    self.vectors.add(item.id, &vector);
                    self.pending_states.lock().push(item.id);
                    self.dirty.fetch_add(1, Ordering::Relaxed);
                    self.stats.lock().emb_done += 1;
                }
                Err(error) => {
                    let _ = self.db.set_state(item.id, "embed", "fail");
                    self.stats.lock().emb_fail += 1;
                    errors.push(format!("{}: {error}", item.name));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    #[allow(dead_code)]
    fn process_text_batch(&self, batch: &[(i64, String)], what: &str) -> Result<(), String> {
        struct Item {
            id: i64,
            name: String,
            text: String,
        }

        let mut items = Vec::with_capacity(batch.len());
        let mut errors = Vec::new();
        for (id, _) in batch {
            let row = match self.db.get_by_id(*id) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    errors.push(format!("文件 {id} 不在数据库中"));
                    continue;
                }
                Err(error) => {
                    errors.push(format!("读取文件 {id} 失败：{error}"));
                    continue;
                }
            };
            if row.path.is_empty() || !Path::new(&row.path).exists() {
                let _ = self.db.mark_missing(&row.path);
                continue;
            }
            self.stats.lock().current = Some(row.name.clone());

            let mut extracted = None;
            let text_pending = row.text_state == "pending" || row.text_state == "fail";
            if what == "auto" && text_pending {
                match crate::textract::extract_text(Path::new(&row.path), self.cfg.max_text_chars) {
                    Some(text) if !text.trim().is_empty() => {
                        let body = crate::util::index_text(&text);
                        let _ = self.db.set_doc(
                            *id,
                            &crate::util::index_text(&row.name),
                            &body,
                            &text,
                        );
                        self.stats.lock().text_done += 1;
                        extracted = Some(text);
                    }
                    _ => {
                        let _ = self.db.set_state(*id, "text", "skip");
                        self.stats.lock().text_fail += 1;
                    }
                }
            }

            let text = extracted
                .or_else(|| self.db.get_doc_raw(*id).ok().flatten())
                .unwrap_or_else(|| {
                    crate::textract::extract_text(Path::new(&row.path), self.cfg.max_text_chars)
                        .unwrap_or_default()
                })
                .chars()
                .take(self.cfg.max_text_chars)
                .collect::<String>();
            if text.trim().is_empty() {
                let _ = self.db.set_state(*id, "embed", "fail");
                self.stats.lock().emb_fail += 1;
                errors.push(format!("{}: 没有可向量化的正文", row.name));
                continue;
            }
            items.push(Item {
                id: *id,
                name: row.name,
                text,
            });
        }

        if !items.is_empty() {
            let texts: Vec<String> = items.iter().map(|item| item.text.clone()).collect();
            let vectors: Vec<Result<Vec<f32>, String>> = match self.embedder.embed_text_batch(&texts) {
                Ok(vectors) if vectors.len() == items.len() => {
                    vectors.into_iter().map(Ok).collect()
                }
                batch_result => {
                    let batch_error = match batch_result {
                        Ok(vectors) => format!("批量向量数量不匹配：{} / {}", vectors.len(), items.len()),
                        Err(error) => error,
                    };
                    let vectors: Vec<Result<Vec<f32>, String>> = items
                        .iter()
                        .map(|item| self.embedder.embed_text(&item.text, false))
                        .collect();
                    if vectors.iter().all(|vector| vector.is_err()) {
                        errors.push(format!("批量编码失败（{batch_error}）"));
                    }
                    vectors
                }
            };

            for (item, vector) in items.into_iter().zip(vectors) {
                match vector {
                    Ok(vector) => {
                        self.vectors.add(item.id, &vector);
                        self.pending_states.lock().push(item.id);
                        self.dirty.fetch_add(1, Ordering::Relaxed);
                        self.stats.lock().emb_done += 1;
                    }
                    Err(error) => {
                        let _ = self.db.set_state(item.id, "embed", "fail");
                        self.stats.lock().emb_fail += 1;
                        errors.push(format!("{}: {error}", item.name));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// Parallel extraction path used by the indexing worker. Text extraction
    /// is CPU-heavy while Candle is busy only during the later batch encode;
    /// doing it across the Rayon pool keeps all CPU cores useful without
    /// concurrent SQLite/vector writes.
    fn process_text_batch_parallel(&self, batch: &[(i64, String)], what: &str) -> Result<(), String> {
        struct Candidate {
            id: i64,
            name: String,
            path: PathBuf,
            needs_extract: bool,
            cached_text: Option<String>,
        }
        struct Item {
            id: i64,
            name: String,
            text: String,
        }

        let mut candidates = Vec::with_capacity(batch.len());
        let mut errors = Vec::new();
        let max_chars = self.cfg.max_text_chars;
        for (id, _) in batch {
            if self.is_cancelled() {
                return Err("index cancellation requested".into());
            }
            let row = match self.db.get_by_id(*id) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    errors.push(format!("file {id} is missing from the database"));
                    continue;
                }
                Err(error) => {
                    errors.push(format!("failed to read file {id}: {error}"));
                    continue;
                }
            };
            if row.path.is_empty() || !Path::new(&row.path).exists() {
                let _ = self.db.mark_missing(&row.path);
                continue;
            }
            self.stats.lock().current = Some(row.name.clone());
            let pending = row.text_state == "pending" || row.text_state == "fail";
            let cached = self.db.get_doc_raw(*id).ok().flatten();
            let needs_extract = (what == "auto" && pending) || cached.is_none();
            candidates.push(Candidate {
                id: *id,
                name: row.name,
                path: PathBuf::from(row.path),
                needs_extract,
                // Keep a previous extraction as a fallback when a parser
                // temporarily fails (for example a malformed PDF font).
                cached_text: cached,
            });
        }

        let candidate_names: Vec<String> = candidates.iter().map(|candidate| candidate.name.clone()).collect();
        self.set_current_items(&candidate_names, self.embedder.concurrency());
        let pool = self.cpu_pool.read().clone();
        let texts: Vec<Option<String>> = pool.install(|| {
            candidates
                .par_iter()
                .map(|candidate| {
                    if candidate.needs_extract {
                        crate::textract::extract_text(&candidate.path, max_chars)
                            .or_else(|| candidate.cached_text.clone())
                    } else {
                        candidate.cached_text.clone()
                    }
                })
                .collect()
        });
        if self.is_cancelled() {
            return Err("index cancellation requested".into());
        }

        let mut items = Vec::with_capacity(texts.len());
        for (candidate, text) in candidates.into_iter().zip(texts) {
            let text = text
                .unwrap_or_default()
                .chars()
                .take(max_chars)
                .collect::<String>();
            if text.trim().is_empty() {
                if candidate.needs_extract {
                    let _ = self.db.set_state(candidate.id, "text", "skip");
                    self.stats.lock().text_fail += 1;
                }
                let _ = self.db.set_state(candidate.id, "embed", "fail");
                self.stats.lock().emb_fail += 1;
                errors.push(format!("{}: no extractable text", candidate.name));
                continue;
            }
            if candidate.needs_extract {
                let body = crate::util::index_text(&text);
                let _ = self.db.set_doc(
                    candidate.id,
                    &crate::util::index_text(&candidate.name),
                    &body,
                    &text,
                );
                self.stats.lock().text_done += 1;
            }
            items.push(Item {
                id: candidate.id,
                name: candidate.name,
                text,
            });
        }

        if !items.is_empty() {
            let names: Vec<String> = items.iter().map(|item| item.name.clone()).collect();
            self.set_current_items(&names, self.embedder.concurrency());
            if self.is_cancelled() {
                return Err("index cancellation requested".into());
            }
            let input: Vec<String> = items.iter().map(|item| item.text.clone()).collect();
            let vectors: Vec<Result<Vec<f32>, String>> = match self.embedder.embed_text_batch(&input) {
                Ok(vectors) if vectors.len() == items.len() => vectors.into_iter().map(Ok).collect(),
                Ok(vectors) => {
                    errors.push(format!("batch embedding returned {} of {} vectors", vectors.len(), items.len()));
                    let mut fallback = Vec::with_capacity(items.len());
                    for item in &items {
                        if self.is_cancelled() {
                            return Err("index cancellation requested".into());
                        }
                        fallback.push(self.embedder.embed_text(&item.text, false));
                    }
                    fallback
                }
                Err(batch_error) => {
                    let mut fallback = Vec::with_capacity(items.len());
                    for item in &items {
                        if self.is_cancelled() {
                            return Err("index cancellation requested".into());
                        }
                        fallback.push(self.embedder.embed_text(&item.text, false));
                    }
                    if fallback.iter().all(Result::is_err) {
                        errors.push(format!("batch embedding failed: {batch_error}"));
                    }
                    fallback
                }
            };
            if self.is_cancelled() {
                return Err("index cancellation requested".into());
            }
            for (item, vector) in items.into_iter().zip(vectors) {
                match vector {
                    Ok(vector) => {
                        self.vectors.add(item.id, &vector);
                        self.pending_states.lock().push(item.id);
                        self.dirty.fetch_add(1, Ordering::Relaxed);
                        self.stats.lock().emb_done += 1;
                    }
                    Err(error) => {
                        let _ = self.db.set_state(item.id, "embed", "fail");
                        self.stats.lock().emb_fail += 1;
                        errors.push(format!("{}: {error}", item.name));
                    }
                }
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }

    fn embed_cross_modal(
        &self,
        row: &crate::db::FileRow,
        cached_pdf_images: Option<&[image::DynamicImage]>,
    ) {
        let path = Path::new(&row.path);
        let mut dirty = 0u64;
        match row.kind.as_str() {
            "document" if row.ext.eq_ignore_ascii_case("pdf") => {
                let clip = self.clip.read().clone();
                let Some(clip) = clip else { return };
                if self.clip_vectors.has(row.id) {
                    return;
                }
                let owned_images;
                let images = match cached_pdf_images {
                    Some(images) => images,
                    None => {
                        owned_images = crate::textract::pdf_images(path, 8);
                        &owned_images
                    }
                };
                let mut merged = vec![0.0f32; clip.dim()];
                let mut count = 0usize;
                // Four representative pages keep CLIP work bounded. The
                // primary embedding still covers all decoded pages.
                for image in images.iter().take(4) {
                    if self.is_cancelled() {
                        return;
                    }
                    if let Ok(vector) = clip.image_data_vec(image) {
                        if vector.len() != merged.len() {
                            continue;
                        }
                        for (dst, value) in merged.iter_mut().zip(vector) {
                            *dst += value;
                        }
                        count += 1;
                    }
                }
                if count == 0 {
                    // A text-only PDF or an image filter that we do not
                    // decode is a valid cross-modal skip, not a retryable
                    // model error.
                    return;
                }
                for value in &mut merged {
                    *value /= count as f32;
                }
                crate::ai::l2_normalize(&mut merged);
                self.clip_vectors.add(row.id, &merged);
                self.stats.lock().cross_done += 1;
                dirty += 1;
            }
            "image" => {
                let clip = self.clip.read().clone();
                let Some(clip) = clip else { return };
                if self.clip_vectors.has(row.id) {
                    return;
                }
                match clip.image_vec(path) {
                    Ok(v) => {
                        self.clip_vectors.add(row.id, &v);
                        self.stats.lock().cross_done += 1;
                        dirty += 1;
                    }
                    Err(e) => {
                        self.record_error(format!("CLIP {}: {e}", row.name));
                    }
                }
            }
            "video" => {
                // 视频取中间那帧代表整个视频：CLIP 对构图/主体的判断
                // 比逐帧平均更稳，也省一大截时间。
                let clip = self.clip.read().clone();
                let Some(clip) = clip else { return };
                if self.clip_vectors.has(row.id) {
                    return;
                }
                let frames = media::sample_video_frames(path, 1, 256);
                let Some(frame) = frames.first() else {
                    // 抽帧失败多半是没装 ffmpeg 或临时目录不可写，说清楚，
                    // 否则用户只看到「视频搜不到」，猜不出原因。
                    self.record_error(format!("视频 {}: 抽帧失败（需要 ffmpeg）", row.name));
                    return;
                };
                match clip.image_vec(frame) {
                    Ok(v) => {
                        self.clip_vectors.add(row.id, &v);
                        self.stats.lock().cross_done += 1;
                        dirty += 1;
                    }
                    Err(e) => {
                        self.record_error(format!("CLIP 帧 {}: {e}", row.name));
                    }
                }
                let _ = std::fs::remove_file(frame);
            }
            "audio" => {
                let clap = self.clap.read().clone();
                let Some(clap) = clap else { return };
                if self.audio_vectors.has(row.id) {
                    return;
                }
                match clap.audio_vec(path) {
                    Ok(v) => {
                        self.audio_vectors.add(row.id, &v);
                        self.stats.lock().cross_done += 1;
                        dirty += 1;
                    }
                    Err(e) => {
                        self.record_error(format!("CLAP {}: {e}", row.name));
                    }
                }
            }
            _ => {}
        }
        if dirty > 0 {
            self.dirty.fetch_add(dirty, Ordering::Relaxed);
        }
    }

    fn record_error(&self, message: String) {
        {
            let mut stats = self.stats.lock();
            stats.cross_fail += 1;
            stats.last_error = Some(message.clone());
        }
        let _ = self.db.log("error", &message);
    }

    fn checkpoint(&self) -> Result<(), String> {
        self.vectors
            .save()
            .map_err(|error| format!("文本向量保存失败：{error}"))?;
        self.media_vectors
            .save()
            .map_err(|error| format!("媒体向量保存失败：{error}"))?;
        self.clip_vectors
            .save()
            .map_err(|error| format!("CLIP 向量保存失败：{error}"))?;
        self.audio_vectors
            .save()
            .map_err(|error| format!("CLAP 向量保存失败：{error}"))?;

        let ids = std::mem::take(&mut *self.pending_states.lock());
        for id in ids {
            self.db
                .set_state(id, "embed", "ok")
                .map_err(|error| format!("提交文件 {id} 的索引状态失败：{error}"))?;
        }
        self.dirty.store(0, Ordering::Relaxed);
        Ok(())
    }

    fn log_checkpoint_error(&self, context: &str, error: &str) {
        let now = crate::util::now_ts().max(0) as u64;
        let previous = self.last_checkpoint_error_log.load(Ordering::Relaxed);
        if now.saturating_sub(previous) >= 10
            && self
                .last_checkpoint_error_log
                .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            let _ = self.db.log("error", &format!("{context}：{error}"));
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let handle = self.worker.lock().take();
        if let Some(handle) = handle {
            if std::thread::current().id() != handle.thread().id() {
                let _ = handle.join();
            }
        }
        if let Err(error) = self.checkpoint() {
            self.log_checkpoint_error("停止索引时保存失败", &error);
        }
    }

    /// Cancel queued work while keeping this indexer alive for the next batch.
    pub fn cancel_pending(&self) {
        self.cancel_requested.store(true, Ordering::Relaxed);
        // Mark the batch as resumable immediately. The worker will perform the
        // durable checkpoint on its next turn, but a new scan may finish before
        // that turn and must still retain the previous completed/total counts.
        self.resume_pending.store(true, Ordering::Relaxed);
        self.cancel_checkpointed.store(false, Ordering::Relaxed);
        self.queue.lock().clear();
        self.active.store(0, Ordering::Relaxed);
        let mut stats = self.stats.lock();
        stats.cancelled = true;
        stats.current = None;
        stats.current_items.clear();
    }

    /// 把缓冲区里的向量立刻写盘（队列空闲时调用）。
    pub fn flush(&self) {
        if self.dirty.load(Ordering::Relaxed) > 0 || !self.pending_states.lock().is_empty() {
            if let Err(error) = self.checkpoint() {
                self.log_checkpoint_error("索引刷新失败", &error);
            }
        }
    }

    /// 等待队列清空（最多 timeout 秒）。
    pub fn wait_idle(&self, timeout_secs: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        loop {
            if self.queue.lock().is_empty() && !self.busy.load(Ordering::Relaxed) {
                // 再宽限两轮，避免刚出队还没置 busy 的空档
                std::thread::sleep(std::time::Duration::from_millis(200));
                if self.queue.lock().is_empty() && !self.busy.load(Ordering::Relaxed) {
                    return true;
                }
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
}
