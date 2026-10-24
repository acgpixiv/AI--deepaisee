//! 后台索引器：文本抽取 → 缩略图 → 向量化。
use crate::ai::{embed_path, Embedder};
use crate::config::IndexConfig;
use crate::db::Db;
use crate::media;
use crate::vector::VectorStore;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

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
    pub running: bool,
    pub current: Option<String>,
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
    queue: Mutex<VecDeque<(i64, String)>>, // (file_id, what)
    stats: Mutex<IndexStats>,
    stop: AtomicBool,
    busy: AtomicBool,
    active: AtomicU64,
    thumb_dir: PathBuf,
    /// 距上次向量库落盘的插入条数（批量落盘，避免每条都全量写盘）
    dirty: AtomicU64,
}

impl Indexer {
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
            queue: Mutex::new(VecDeque::new()),
            stats: Mutex::new(IndexStats::default()),
            stop: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            active: AtomicU64::new(0),
            thumb_dir,
            dirty: AtomicU64::new(0),
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
        s
    }

    pub fn backlog(&self) -> usize {
        self.queue.lock().len()
    }

    pub fn enqueue(&self, ids: Vec<i64>, what: &str) {
        let mut q = self.queue.lock();
        for id in ids {
            q.push_back((id, what.to_string()));
        }
        self.active.store(q.len() as u64, Ordering::Relaxed);
    }

    /// 把库里所有 pending 的文件入队。
    pub fn enqueue_all_pending(&self, what: &str) -> usize {
        let ids = self.db.pending_ids(what, 100_000).unwrap_or_default();
        let n = ids.len();
        self.enqueue(ids, what);
        n
    }

    /// 同上，但最多只排 `limit` 条（启动阶段用，避免积压几十万条时一开机就满载）。
    pub fn enqueue_all_pending_limited(&self, what: &str, limit: usize) -> usize {
        let ids = self.db.pending_ids(what, limit).unwrap_or_default();
        let n = ids.len();
        self.enqueue(ids, what);
        n
    }

    /// 启动后台线程（幂等）。
    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        std::thread::spawn(move || this.run_loop());
    }

    fn run_loop(&self) {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            let item = {
                let mut q = self.queue.lock();
                q.pop_front()
            };
            let Some((fid, what)) = item else {
                // 队列空了：把攒着的向量落盘
                self.flush();
                std::thread::sleep(std::time::Duration::from_millis(150));
                continue;
            };
            self.busy.store(true, Ordering::Relaxed);
            let r = self.process_one(fid, &what);
            self.busy.store(false, Ordering::Relaxed);
            self.active.fetch_sub(1, Ordering::Relaxed);
            if let Err(e) = r {
                let mut s = self.stats.lock();
                s.last_error = Some(e);
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
        {
            let mut s = self.stats.lock();
            s.current = Some(row.name.clone());
        }

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
        if matches!(row.kind.as_str(), "image" | "video") {
            let dest = self.thumb_dir.join(format!("{fid}.jpg"));
            if !dest.exists() {
                let _ = std::fs::create_dir_all(&self.thumb_dir);
                if media::make_thumb(Path::new(&row.path), &dest, self.cfg.image_max_side) {
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
            if row.tags.is_none() {
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

        // 3. 向量
        //
        // 文本类走主 embedding 空间（llama.cpp GGUF，512 维）；
        // 图片/音频/视频走 hash 空间（256 维）——llama.cpp 只处理文本，
        // 两个空间维度不同，必须分开存，否则向量库会错位。
        if what == "embed" || what == "auto" {
            let is_media = matches!(row.kind.as_str(), "image" | "audio" | "video");
            let (embedder, store) = if is_media {
                (
                    Arc::clone(&self.media_embedder),
                    Arc::clone(&self.media_vectors),
                )
            } else {
                (Arc::clone(&self.embedder), Arc::clone(&self.vectors))
            };
            match embed_path(embedder.as_ref(), Path::new(&row.path), &row.kind) {
                Ok(vec) => {
                    store.add(fid, &vec);
                    // 批量落盘：向量库是全量写盘（几百 MB 量级），每条都 save()
                    // 会让硬盘狂转、索引速度掉到个位数。每 64 条或队列空闲时再写。
                    let n = self.dirty.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 64 == 0 {
                        let _ = store.save();
                    }
                    let _ = self.db.set_state(fid, "embed", "ok");
                    self.stats.lock().emb_done += 1;
                }
                Err(e) => {
                    let _ = self.db.set_state(fid, "embed", "fail");
                    self.stats.lock().emb_fail += 1;
                    return Err(format!("{}: {e}", row.name));
                }
            }
        }

        // 4. 跨模态（CLIP / CLAP）：把图片和音频编码到「和文字同一空间」，
        //    这样「一句话描述找图片」才成立。hash 向量只描述颜色/能量分布，
        //    跟文字八竿子打不着，不能拿来给文字查询用。
        self.embed_cross_modal(&row);

        Ok(())
    }

    /// 跨模态向量化。模型缺席/文件不支持时静默跳过，不算失败。
    fn embed_cross_modal(&self, row: &crate::db::FileRow) {
        let path = Path::new(&row.path);
        let mut dirty = 0u64;
        match row.kind.as_str() {
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
                        self.stats.lock().cross_fail += 1;
                        self.stats.lock().last_error = Some(format!("CLIP {}: {e}", row.name));
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
                    self.stats.lock().cross_fail += 1;
                    self.stats.lock().last_error = Some(format!("视频 {}: 抽帧失败（需要 ffmpeg）", row.name));
                    return;
                };
                match clip.image_vec(frame) {
                    Ok(v) => {
                        self.clip_vectors.add(row.id, &v);
                        self.stats.lock().cross_done += 1;
                        dirty += 1;
                    }
                    Err(e) => {
                        self.stats.lock().cross_fail += 1;
                        self.stats.lock().last_error = Some(format!("CLIP 帧 {}: {e}", row.name));
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
                        self.stats.lock().cross_fail += 1;
                        self.stats.lock().last_error = Some(format!("CLAP {}: {e}", row.name));
                    }
                }
            }
            _ => {}
        }
        if dirty > 0 {
            let n = self.dirty.fetch_add(dirty, Ordering::Relaxed) + dirty;
            if n % 64 == 0 {
                let _ = self.clip_vectors.save();
                let _ = self.audio_vectors.save();
            }
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.vectors.save();
        let _ = self.media_vectors.save();
        let _ = self.clip_vectors.save();
        let _ = self.audio_vectors.save();
    }

    /// 把缓冲区里的向量立刻写盘（队列空闲时调用）。
    pub fn flush(&self) {
        if self.dirty.swap(0, Ordering::Relaxed) > 0 {
            let _ = self.vectors.save();
            let _ = self.media_vectors.save();
            let _ = self.clip_vectors.save();
            let _ = self.audio_vectors.save();
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
