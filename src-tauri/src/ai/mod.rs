//! 向量化抽象层。
//!
//! - `LlamaEmbedder`：通过 llama.cpp 的 C API 做文本/图片 embedding（真模型）
//! - `HashEmbedder`：无模型时的确定性兜底（文本 n-gram + 图片颜色直方图 + 音视频字节特征）
pub mod bpe;
pub mod clap;
pub mod clip;
pub mod hash;
#[cfg(feature = "candle")]
pub mod candle_backend;
#[cfg(feature = "llama")]
pub mod llama;
pub mod ort;

use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize)]
pub struct EmbedInfo {
    pub backend: String,
    pub model: String,
    pub dim: usize,
    pub device: String,
    pub modalities: Vec<String>,
    pub ready: bool,
    pub detail: String,
}

pub trait Embedder: Send + Sync {
    fn info(&self) -> EmbedInfo;
    fn dim(&self) -> usize;
    fn concurrency(&self) -> usize { 1 }
    /// 文本向量；`query=true` 时加检索指令前缀。
    fn embed_text(&self, text: &str, query: bool) -> Result<Vec<f32>, String>;
    /// Batch document encoding. Backends that support native batching should
    /// override this; the default preserves compatibility with older models.
    fn embed_text_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        texts.iter().map(|text| self.embed_text(text, false)).collect()
    }
    fn embed_image_batch(&self, paths: &[&Path]) -> Result<Vec<Vec<f32>>, String> {
        paths.iter().map(|path| self.embed_image(path)).collect()
    }
    fn embed_image(&self, path: &Path) -> Result<Vec<f32>, String>;
    fn embed_image_data(&self, _image: &image::DynamicImage) -> Result<Vec<f32>, String> {
        Err("当前模型不支持内存图像编码".into())
    }
    fn embed_image_data_batch(
        &self,
        images: &[&image::DynamicImage],
    ) -> Result<Vec<Vec<f32>>, String> {
        images.iter().map(|image| self.embed_image_data(image)).collect()
    }
    fn embed_audio(&self, path: &Path) -> Result<Vec<f32>, String>;
    fn embed_video(&self, path: &Path) -> Result<Vec<f32>, String>;
}

/// Runs independent model instances on disjoint slices of a batch. A single
/// instance is still used for queries and for media items that arrive one at
/// a time, which keeps search results deterministic and avoids duplicating
/// work for small requests.
pub struct ShardedEmbedder {
    shards: Vec<Arc<dyn Embedder>>,
}

impl ShardedEmbedder {
    fn new(shards: Vec<Arc<dyn Embedder>>) -> Self {
        Self { shards }
    }

    fn parallel_batch<T, F>(&self, len: usize, f: F) -> Result<Vec<T>, String>
    where
        T: Send,
        F: Fn(&dyn Embedder, usize, usize) -> Result<Vec<T>, String> + Send + Sync,
    {
        if len == 0 {
            return Ok(Vec::new());
        }
        let shard_count = self.shards.len().min(len).max(1);
        if shard_count == 1 {
            return f(self.shards[0].as_ref(), 0, len);
        }
        let ranges: Vec<(usize, usize)> = (0..shard_count)
            .map(|index| (index * len / shard_count, (index + 1) * len / shard_count))
            .collect();

        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(ranges.len());
            for (index, &(start, end)) in ranges.iter().enumerate() {
                let shard = Arc::clone(&self.shards[index % self.shards.len()]);
                let task = &f;
                handles.push(scope.spawn(move || task(shard.as_ref(), start, end)));
            }
            let mut output = Vec::with_capacity(len);
            for ((start, end), handle) in ranges.into_iter().zip(handles) {
                let part = handle
                    .join()
                    .map_err(|_| "并行模型实例线程异常退出".to_string())??;
                if part.len() != end - start {
                    return Err(format!(
                        "模型分片返回数量不匹配：{} / {}",
                        part.len(),
                        end - start
                    ));
                }
                output.extend(part);
            }
            Ok(output)
        })
    }
}

impl Embedder for ShardedEmbedder {
    fn info(&self) -> EmbedInfo {
        self.shards[0].info()
    }

    fn dim(&self) -> usize {
        self.shards[0].dim()
    }

    fn concurrency(&self) -> usize {
        self.shards.len()
    }

    fn embed_text(&self, text: &str, query: bool) -> Result<Vec<f32>, String> {
        self.shards[0].embed_text(text, query)
    }

    fn embed_text_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        self.parallel_batch(texts.len(), |shard, start, end| {
            shard.embed_text_batch(&texts[start..end])
        })
    }

    fn embed_image(&self, path: &Path) -> Result<Vec<f32>, String> {
        self.shards[0].embed_image(path)
    }

    fn embed_image_batch(&self, paths: &[&Path]) -> Result<Vec<Vec<f32>>, String> {
        self.parallel_batch(paths.len(), |shard, start, end| {
            shard.embed_image_batch(&paths[start..end])
        })
    }

    fn embed_image_data(&self, image: &image::DynamicImage) -> Result<Vec<f32>, String> {
        self.shards[0].embed_image_data(image)
    }

    fn embed_image_data_batch(
        &self,
        images: &[&image::DynamicImage],
    ) -> Result<Vec<Vec<f32>>, String> {
        self.parallel_batch(images.len(), |shard, start, end| {
            shard.embed_image_data_batch(&images[start..end])
        })
    }

    fn embed_audio(&self, path: &Path) -> Result<Vec<f32>, String> {
        self.shards[0].embed_audio(path)
    }

    fn embed_video(&self, path: &Path) -> Result<Vec<f32>, String> {
        self.shards[0].embed_video(path)
    }
}

/// 按文件类型分发到对应模态。
pub fn embed_path(e: &dyn Embedder, path: &Path, kind: &str) -> Result<Vec<f32>, String> {
    if path.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("pdf")).unwrap_or(false) {
        let text = crate::textract::extract_text(path, 20_000).unwrap_or_default();
        let images = crate::textract::pdf_images(path, 8);
        return embed_pdf_parts(e, (!text.trim().is_empty()).then_some(text.as_str()), &images);
    }
    match kind {
        "image" => e.embed_image(path),
        "audio" => e.embed_audio(path),
        "video" => e.embed_video(path),
        _ => {
            let text = crate::textract::extract_text(path, 20_000).unwrap_or_default();
            if text.trim().is_empty() {
                return Err("无文本内容".into());
            }
            e.embed_text(&text, false)
        }
    }
}

/// Encode the pieces already extracted from a PDF and fuse them into one
/// document vector. Callers can pass a cached extraction so a PDF is unpacked
/// only once per indexing item.
pub fn embed_pdf_parts(
    e: &dyn Embedder,
    text: Option<&str>,
    images: &[image::DynamicImage],
) -> Result<Vec<f32>, String> {
    let mut vectors = Vec::new();
    if let Some(text) = text.filter(|value| !value.trim().is_empty()) {
        vectors.push(e.embed_text(text, false)?);
    }
    if !images.is_empty() {
        let image_refs: Vec<&image::DynamicImage> = images.iter().collect();
        match e.embed_image_data_batch(&image_refs) {
            Ok(encoded) => vectors.extend(encoded),
            Err(_) => {
                // A single malformed/unsupported raster should not discard
                // the other page images. Retry per image only on the rare
                // batch failure, keeping the normal path fully batched.
                for image in images {
                    if let Ok(vector) = e.embed_image_data(image) {
                        vectors.push(vector);
                    }
                }
            }
        }
    }
    if vectors.is_empty() {
        return Err("PDF 未抽取到可用文字或图片".into());
    }
    fuse_vectors(vectors, "PDF 编码结果为空")
}

fn fuse_vectors(vectors: Vec<Vec<f32>>, empty_message: &str) -> Result<Vec<f32>, String> {
    let Some(first) = vectors.first() else {
        return Err(empty_message.into());
    };
    let mut merged = vec![0.0f32; first.len()];
    let mut count = 0.0f32;
    for vector in vectors {
        if vector.len() != merged.len() {
            continue;
        }
        for (dst, value) in merged.iter_mut().zip(vector) {
            *dst += value;
        }
        count += 1.0;
    }
    if count == 0.0 {
        return Err(empty_message.into());
    }
    for value in &mut merged {
        *value /= count;
    }
    l2_normalize(&mut merged);
    Ok(merged)
}

/// Encode several PDFs in one operation. Texts and page images are flattened
/// into batch calls, then fused back into one vector per source document.
pub fn embed_pdf_parts_batch(
    e: &dyn Embedder,
    parts: &[(Option<String>, &[image::DynamicImage])],
) -> Vec<Result<Vec<f32>, String>> {
    let mut output: Vec<Option<Result<Vec<f32>, String>>> = vec![None; parts.len()];
    let result = embed_pdf_parts_batch_with(e, parts, || false, |index, vector| {
        output[index] = Some(vector);
    });
    if let Err(error) = result {
        return (0..parts.len()).map(|_| Err(error.clone())).collect();
    }
    output
        .into_iter()
        .map(|value| value.unwrap_or_else(|| Err("PDF 向量编码未完成".into())))
        .collect()
}

/// Encode bounded chunks and report each PDF once all of its page images are
/// encoded. The caller's cancellation hook is checked between model calls.
pub fn embed_pdf_parts_batch_with<F, C>(
    e: &dyn Embedder,
    parts: &[(Option<String>, &[image::DynamicImage])],
    cancelled: C,
    mut on_document: F,
) -> Result<(), String>
where
    F: FnMut(usize, Result<Vec<f32>, String>),
    C: Fn() -> bool,
{
    const PDF_EMBED_CHUNK: usize = 8;
    let mut per_document: Vec<Vec<Vec<f32>>> = vec![Vec::new(); parts.len()];
    let mut remaining_images: Vec<usize> = parts.iter().map(|(_, images)| images.len()).collect();
    let mut reported = vec![false; parts.len()];
    let mut text_map = Vec::new();
    let mut texts = Vec::new();
    for (index, (text, _)) in parts.iter().enumerate() {
        if let Some(text) = text.as_deref().filter(|value| !value.trim().is_empty()) {
            text_map.push(index);
            texts.push(text.to_owned());
        }
    }
    for (map_chunk, text_chunk) in text_map.chunks(PDF_EMBED_CHUNK).zip(texts.chunks(PDF_EMBED_CHUNK)) {
        if cancelled() {
            return Err("index cancellation requested".into());
        }
        let encoded: Vec<Option<Vec<f32>>> = match e.embed_text_batch(text_chunk) {
            Ok(rows) if rows.len() == text_chunk.len() => rows.into_iter().map(Some).collect(),
            _ => {
                let mut rows = Vec::with_capacity(text_chunk.len());
                for text in text_chunk {
                    if cancelled() {
                        return Err("index cancellation requested".into());
                    }
                    rows.push(e.embed_text(text, false).ok());
                }
                rows
            }
        };
        for (&index, vector) in map_chunk.iter().zip(encoded) {
            if let Some(vector) = vector {
                per_document[index].push(vector);
            }
        }
    }
    if cancelled() {
        return Err("index cancellation requested".into());
    }

    let mut image_map = Vec::new();
    let mut images = Vec::new();
    for (index, (_, document_images)) in parts.iter().enumerate() {
        for image in document_images.iter() {
            image_map.push(index);
            images.push(image);
        }
    }
    for index in 0..parts.len() {
        if remaining_images[index] == 0 {
            reported[index] = true;
            on_document(index, fuse_vectors(std::mem::take(&mut per_document[index]), "PDF 未抽取到可用文字或图片"));
        }
    }

    for (map_chunk, image_chunk) in image_map.chunks(PDF_EMBED_CHUNK).zip(images.chunks(PDF_EMBED_CHUNK)) {
        if cancelled() {
            return Err("index cancellation requested".into());
        }
        let encoded: Vec<Option<Vec<f32>>> = match e.embed_image_data_batch(image_chunk) {
            Ok(rows) if rows.len() == image_chunk.len() => rows.into_iter().map(Some).collect(),
            _ => {
                let mut rows = Vec::with_capacity(image_chunk.len());
                for image in image_chunk {
                    if cancelled() {
                        return Err("index cancellation requested".into());
                    }
                    rows.push(e.embed_image_data(image).ok());
                }
                rows
            }
        };
        for (&index, vector) in map_chunk.iter().zip(encoded) {
            remaining_images[index] = remaining_images[index].saturating_sub(1);
            if let Some(vector) = vector {
                per_document[index].push(vector);
            }
            if remaining_images[index] == 0 && !reported[index] {
                reported[index] = true;
                on_document(index, fuse_vectors(std::mem::take(&mut per_document[index]), "PDF 未抽取到可用文字或图片"));
            }
        }
    }
    Ok(())
}

pub fn l2_normalize(v: &mut [f32]) {
    let mut sum = 0.0f32;
    for x in v.iter() {
        sum += x * x;
    }
    let n = sum.sqrt();
    if n > 1e-8 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

/// Resolve a user-selected model file or folder to the actual embedding
/// entrypoint. Model downloads often contain both a safetensors package and a
/// GGUF `mmproj`; the projector is a companion, never a standalone embedder.
pub fn resolve_model_path(input: &str) -> Result<std::path::PathBuf, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("模型路径为空".into());
    }
    let path = Path::new(raw);
    if path.is_file() {
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or_default();
        if ext.eq_ignore_ascii_case("gguf") {
            let name = path.file_name().and_then(|x| x.to_str()).unwrap_or_default().to_ascii_lowercase();
            if name.contains("mmproj") || name.contains("projector") {
                return Err("mmproj 是投影配套文件，不能作为独立嵌入模型加载".into());
            }
            return Ok(path.to_path_buf());
        }
        if ext.eq_ignore_ascii_case("safetensors") {
            return Ok(path.to_path_buf());
        }
        return Err(format!("无法识别模型格式：{raw}"));
    }
    if !path.is_dir() {
        return Err(format!("模型路径不存在：{raw}"));
    }

    let mut safe_dirs = Vec::new();
    let mut ggufs = Vec::new();
    for entry in walkdir::WalkDir::new(path).follow_links(false).max_depth(4).into_iter().flatten() {
        let p = entry.path();
        if !p.is_file() { continue; }
        let name = p.file_name().and_then(|x| x.to_str()).unwrap_or_default().to_ascii_lowercase();
        if name == "model.safetensors" && p.parent().map(|d| d.join("config.json").is_file() && d.join("tokenizer.json").is_file()).unwrap_or(false) {
            safe_dirs.push(p.parent().unwrap().to_path_buf());
        }
        if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("gguf")).unwrap_or(false)
            && !name.contains("mmproj") && !name.contains("projector")
        {
            ggufs.push(p.to_path_buf());
        }
    }
    #[cfg(feature = "candle")]
    if let Some(dir) = safe_dirs.into_iter().next() {
        return Ok(dir);
    }
    ggufs.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)));
    ggufs.into_iter().next().ok_or_else(|| format!("目录中没有找到完整的 safetensors 或 GGUF 嵌入模型：{raw}"))
}

/// 按配置构建 embedding 后端。
///
/// `embed_model` 根据模型格式选择可用的原生适配器：GGUF 使用 llama.cpp，
/// EmbeddingGemma2 safetensors 在启用 candle feature 时使用 Candle；未知格式
/// 不会被强行交给错误后端。
pub fn build_embedder(cfg: &crate::config::ModelConfig) -> Result<Box<dyn Embedder>, String> {
    build_embedder_with_batch(cfg, 8)
}

/// Pick a throughput-oriented batch size for the selected indexing profile.
///
/// `Options::batch_size` is the batch size used inside the Candle model.  The
/// old code only changed the queue batch size, while Candle kept its default
/// eight-item batches, so a safetensors model could leave a capable GPU mostly
/// idle.  Keep the values bounded to avoid turning a large document set into
/// an uncontrolled VRAM allocation; Candle still splits long sequences by
/// attention cost.
pub fn effective_batch_size(cfg: &crate::config::ModelConfig, configured: usize) -> usize {
    let requested = configured.clamp(1, 128);
    match cfg.parallel_mode.trim().to_ascii_lowercase().as_str() {
        "eco" => requested.min(4),
        "turbo" | "狂暴" => requested.max(32).min(128),
        // Automatic mode raises the small historical default, but honours a
        // deliberate larger value from the settings page.
        _ => requested.max(16).min(64),
    }
}

#[cfg(feature = "llama")]
fn available_cpu_cores() -> usize {
    let logical = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .max(1);
    #[cfg(windows)]
    if let Some(idle_fraction) = windows_cpu_idle_fraction() {
        return ((logical as f64 * idle_fraction).floor() as usize).clamp(1, logical);
    }
    logical
}

#[cfg(all(windows, feature = "llama"))]
fn windows_cpu_idle_fraction() -> Option<f64> {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    impl FileTime {
        fn value(self) -> u64 {
            (u64::from(self.high) << 32) | u64::from(self.low)
        }
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }

    let mut idle_before = FileTime::default();
    let mut kernel_before = FileTime::default();
    let mut user_before = FileTime::default();
    if unsafe { GetSystemTimes(&mut idle_before, &mut kernel_before, &mut user_before) } == 0 {
        return None;
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
    let mut idle_after = FileTime::default();
    let mut kernel_after = FileTime::default();
    let mut user_after = FileTime::default();
    if unsafe { GetSystemTimes(&mut idle_after, &mut kernel_after, &mut user_after) } == 0 {
        return None;
    }
    let idle = idle_after.value().saturating_sub(idle_before.value());
    let kernel = kernel_after.value().saturating_sub(kernel_before.value());
    let user = user_after.value().saturating_sub(user_before.value());
    let total = kernel.saturating_add(user);
    (total > 0).then(|| (idle as f64 / total as f64).clamp(0.0, 1.0))
}

#[cfg(feature = "llama")]
fn effective_model_threads(cfg: &crate::config::ModelConfig) -> i32 {
    if cfg.n_threads > 0 {
        return cfg.n_threads;
    }
    let cores = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .max(1);
    let mode = cfg.parallel_mode.trim().to_ascii_lowercase();
    let threads = match mode.as_str() {
        "eco" | "节能" => (cores / 3).max(1),
        "turbo" | "狂暴" => cores.div_ceil(5).max(1),
        _ => (cores / 8).max(1),
    };
    threads.min(i32::MAX as usize) as i32
}

#[cfg(feature = "llama")]
fn model_weight_bytes(cfg: &crate::config::ModelConfig) -> u64 {
    let Ok(path) = resolve_model_path(&cfg.embed_model) else {
        return 0;
    };
    if path.is_file() {
        return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            matches!(
                entry.path().extension().and_then(|x| x.to_str()).map(|x| x.to_ascii_lowercase()).as_deref(),
                Some("safetensors") | Some("bin") | Some("gguf")
            )
        })
        .filter_map(|entry| entry.metadata().ok().map(|m| m.len()))
        .sum()
}

#[cfg(feature = "llama")]
const GPU_RESERVE_BYTES: u64 = 1024 * 1024 * 1024;

#[cfg(feature = "llama")]
fn replica_memory_budget(cfg: &crate::config::ModelConfig, info: &EmbedInfo) -> u64 {
    let weight_bytes = model_weight_bytes(cfg);
    if weight_bytes == 0 {
        return 0;
    }
    let mode = cfg.parallel_mode.trim().to_ascii_lowercase();
    let turbo = matches!(mode.as_str(), "turbo" | "狂暴");
    let (weight_factor, safety_bytes) = if info.backend == "candle" {
        (32, 2 * GPU_RESERVE_BYTES)
    } else if turbo {
        (28, 2 * GPU_RESERVE_BYTES)
    } else {
        (32, 2 * GPU_RESERVE_BYTES)
    };
    weight_bytes
        .saturating_mul(weight_factor)
        .saturating_div(10)
        .saturating_add(safety_bytes)
}

/// Choose extra devices for model replicas. Candle safetensors stays on CUDA;
/// GGUF can also use Vulkan adapters exposed by llama.cpp.
pub fn replica_device_plan(
    cfg: &crate::config::ModelConfig,
    info: &EmbedInfo,
) -> Vec<String> {
    let mode = cfg.parallel_mode.trim().to_ascii_lowercase();
    // The primary instance is loaded before this function. Keep at most four
    // extra replicas so effective concurrency is always 1 (eco), 1..5 (auto),
    // or 1..5 (turbo), and the UI can report the real shard count.
    #[cfg(feature = "llama")]
    let cpu_cap = available_cpu_cores().div_ceil(2).clamp(1, 5);
    #[cfg(not(feature = "llama"))]
    let cpu_cap = 1;
    let cap: usize = match mode.as_str() {
        "eco" | "节能" => 1,
        "turbo" | "狂暴" => 5,
        _ => cpu_cap,
    };
    if cap <= 1 || info.backend == "hash" {
        return Vec::new();
    }
    #[cfg(feature = "llama")]
    {
        let weight_bytes = model_weight_bytes(cfg);
        if weight_bytes == 0 {
            return Vec::new();
        }
        // Keep one GiB unavailable on every adapter. The file size is only
        // the resident weights; CUDA/Candle also needs allocator arenas,
        // token buffers and image activations. A deliberately conservative
        // per-replica budget prevents a larger replica count from turning
        // into paging/OOM and lower throughput.
        let required = replica_memory_budget(cfg, info);
        if required == 0 {
            return Vec::new();
        }
        let mut devices = llama::gpu_devices();
        if info.backend == "candle" {
            devices.retain(|device| device.backend.eq_ignore_ascii_case("cuda"));
        }
        if devices.is_empty() {
            return Vec::new();
        }
        let primary = if cfg.gpu_device.trim().is_empty() || cfg.gpu_device.eq_ignore_ascii_case("auto") {
            devices.iter().max_by_key(|device| device.total_memory)
        } else {
            devices.iter().find(|device| device.id == cfg.gpu_device)
        };
        let Some(primary) = primary else {
            return Vec::new();
        };
        let primary_id = primary.id.clone();
        let mut ordered: Vec<_> = std::iter::once(primary)
            .chain(devices.iter().filter(|device| device.id != primary_id))
            .collect();
        ordered.sort_by_key(|device| {
            (device.id != primary_id, u64::MAX - device.total_memory)
        });
        let capacities: Vec<usize> = ordered
            .iter()
            .map(|device| {
                let free = if device.integrated {
                    device.free_memory.min(device.total_memory / 2)
                } else {
                    device.free_memory
                };
                let occupied = device.total_memory.saturating_sub(free);
                let budget = device.total_memory.saturating_sub(GPU_RESERVE_BYTES);
                let room = budget.saturating_sub(occupied);
                if device.id == primary_id {
                    1 + (room / required) as usize
                } else {
                    (room / required) as usize
                }
            })
            .collect();
        let mut used = vec![0usize; ordered.len()];
        if let Some(index) = ordered.iter().position(|device| device.id == primary_id) {
            used[index] = 1;
        }
        let mut plan = Vec::with_capacity(cap.saturating_sub(1));
        while plan.len() + 1 < cap {
            let mut added = false;
            for (index, device) in ordered.iter().enumerate() {
                if used[index] < capacities[index] {
                    plan.push(device.id.clone());
                    used[index] += 1;
                    added = true;
                    if plan.len() + 1 >= cap {
                        break;
                    }
                }
            }
            if !added {
                break;
            }
        }
        plan
    }
    #[cfg(not(feature = "llama"))]
    {
        let _ = (cfg, info, cap);
        Vec::new()
    }
}

/// Expand a successfully loaded model into a guarded multi-device shard.
/// Extra copies are best-effort; any failed device keeps the working model.
pub fn maybe_replicate(
    initial: Arc<dyn Embedder>,
    cfg: &crate::config::ModelConfig,
    batch_size: usize,
) -> Arc<dyn Embedder> {
    let plan = replica_device_plan(cfg, &initial.info());
    if plan.is_empty() {
        return initial;
    }
    let mut shards = vec![Arc::clone(&initial)];
    let mut blocked_devices = std::collections::HashSet::new();
    for device_id in plan {
        if blocked_devices.contains(&device_id) {
            continue;
        }
        #[cfg(feature = "llama")]
        {
            let required = replica_memory_budget(cfg, &initial.info());
            let enough_room = llama::gpu_devices()
                .into_iter()
                .find(|device| device.id == device_id)
                .map(|device| {
                    let free = if device.integrated {
                        device.free_memory.min(device.total_memory / 2)
                    } else {
                        device.free_memory
                    };
                    required > 0 && free.saturating_sub(GPU_RESERVE_BYTES) >= required
                })
                .unwrap_or(false);
            if !enough_room {
                blocked_devices.insert(device_id);
                continue;
            }
        }
        let mut replica_cfg = cfg.clone();
        replica_cfg.gpu_device = device_id.clone();
        match build_embedder_with_batch(&replica_cfg, batch_size) {
            Ok(extra) => {
                let extra: Arc<dyn Embedder> = Arc::from(extra);
                if extra.dim() != initial.dim() || extra.info().backend != initial.info().backend {
                    blocked_devices.insert(device_id);
                    continue;
                }
                shards.push(extra);
            }
            Err(_) => {
                blocked_devices.insert(device_id);
            }
        }
    }
    if shards.len() > 1 {
        Arc::new(ShardedEmbedder::new(shards))
    } else {
        initial
    }
}

#[cfg(test)]
mod sharded_tests {
    use super::*;

    struct NumberEmbedder {
        offset: f32,
    }

    impl Embedder for NumberEmbedder {
        fn info(&self) -> EmbedInfo {
            EmbedInfo {
                backend: "test".into(),
                model: "test".into(),
                dim: 1,
                device: "test".into(),
                modalities: vec!["text".into()],
                ready: true,
                detail: String::new(),
            }
        }

        fn dim(&self) -> usize { 1 }

        fn embed_text(&self, text: &str, _query: bool) -> Result<Vec<f32>, String> {
            Ok(vec![text.parse::<f32>().map_err(|e| e.to_string())? + self.offset])
        }

        fn embed_image(&self, _path: &Path) -> Result<Vec<f32>, String> { Err("unused".into()) }
        fn embed_audio(&self, _path: &Path) -> Result<Vec<f32>, String> { Err("unused".into()) }
        fn embed_video(&self, _path: &Path) -> Result<Vec<f32>, String> { Err("unused".into()) }
    }

    #[test]
    fn sharded_batches_preserve_order_and_queries_use_first_instance() {
        let shards: Vec<Arc<dyn Embedder>> = vec![
            Arc::new(NumberEmbedder { offset: 0.0 }),
            Arc::new(NumberEmbedder { offset: 100.0 }),
        ];
        let embedder = ShardedEmbedder::new(shards);
        assert_eq!(embedder.concurrency(), 2);
        let input: Vec<String> = (0..5).map(|n| n.to_string()).collect();
        let vectors = embedder.embed_text_batch(&input).unwrap();
        let values: Vec<f32> = vectors.into_iter().map(|v| v[0]).collect();
        assert_eq!(values, vec![0.0, 1.0, 102.0, 103.0, 104.0]);
        assert_eq!(embedder.embed_text("7", true).unwrap(), vec![7.0]);
    }

    #[test]
    fn sharded_batch_uses_each_available_replica() {
        let shards: Vec<Arc<dyn Embedder>> = (0..5)
            .map(|index| Arc::new(NumberEmbedder { offset: index as f32 * 100.0 }) as Arc<dyn Embedder>)
            .collect();
        let embedder = ShardedEmbedder::new(shards);
        let input: Vec<String> = (0..8).map(|n| n.to_string()).collect();
        let vectors = embedder.embed_text_batch(&input).unwrap();
        let values: Vec<f32> = vectors.into_iter().map(|v| v[0]).collect();
        assert_eq!(values, vec![0.0, 101.0, 102.0, 203.0, 304.0, 305.0, 406.0, 407.0]);
    }

    #[test]
    fn pdf_batch_fallback_keeps_each_document_result_in_order() {
        let embedder = NumberEmbedder { offset: 0.0 };
        let no_images: &[image::DynamicImage] = &[];
        let parts = [
            (Some("invalid".to_owned()), no_images),
            (Some("2".to_owned()), no_images),
        ];

        let vectors = embed_pdf_parts_batch(&embedder, &parts);
        assert!(vectors[0].is_err());
        assert_eq!(vectors[1].as_ref().unwrap(), &vec![1.0]);
    }
}

pub fn build_embedder_with_batch(
    cfg: &crate::config::ModelConfig,
    batch_size: usize,
) -> Result<Box<dyn Embedder>, String> {
    #[cfg(not(feature = "candle"))]
    let _ = batch_size;
    let path = cfg.embed_model.trim();
    if path.is_empty() {
        return Ok(Box::new(hash::HashEmbedder::new()));
    }
    let resolved = resolve_model_path(path)?;
    let model_path = resolved.as_path();
    let safe_dir = if model_path.is_dir() {
        Some(model_path)
    } else if model_path.extension().map(|x| x.eq_ignore_ascii_case("safetensors")).unwrap_or(false) {
        model_path.parent()
    } else {
        None
    };
    if let Some(_dir) = safe_dir.filter(|p| p.exists()) {
        #[cfg(feature = "candle")]
        {
            return candle_backend::CandleEmbedder::load(
                _dir,
                &cfg.gpu_device,
                effective_batch_size(cfg, batch_size),
            ).map(|e| Box::new(e) as Box<dyn Embedder>);
        }
        #[cfg(not(feature = "candle"))]
        return Err("此 safetensors 模型需要使用 --features candle 构建原生 Candle 后端".into());
    }
    #[cfg(feature = "llama")]
    {
        if model_path.extension().map(|x| x.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
            return match llama::LlamaEmbedder::load(
                &resolved.to_string_lossy(),
                cfg.context_size as i32,
                effective_model_threads(cfg),
                &cfg.gpu_device,
            ) {
                Ok(e) => Ok(Box::new(e)),
                Err(e) => Err(format!("llama.cpp 加载失败：{e}")),
            };
        }
        return Err(format!("模型文件不存在或没有匹配的原生后端：{path}"));
    }
    #[cfg(not(feature = "llama"))]
    {
        let _ = path;
        Ok(Box::new(hash::HashEmbedder::new()))
    }
}
