//! Native Candle adapter for EmbeddingGemma2.
//!
//! The adapter is deliberately kept behind the `candle` feature. The stable
//! search/indexing core only sees `Embedder`, so another Candle model can be
//! added without changing search fusion or file scanning.

use super::{EmbedInfo, Embedder};
use embeddinggemma2::{EmbeddingGemma2, Input, Modalities, Options};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(all(feature = "candle-cuda", not(feature = "llama")))]
fn cuda_ordinal_fallback(selected_device: &str) -> Result<usize, String> {
    let selected = selected_device.trim();
    if selected.is_empty() || selected.eq_ignore_ascii_case("auto") {
        return Ok(0);
    }

    // llama.cpp exposes accelerator ids as `<backend>|<device-id>`. CUDA
    // device ids are ordinals, so this also accepts the common `cuda:N`
    // spelling used by config files from older builds.
    let lower = selected.to_ascii_lowercase();
    let ordinal = if let Some(rest) = lower.strip_prefix("cuda:") {
        rest.parse::<usize>().ok()
    } else if let Some((backend, rest)) = lower.split_once('|') {
        if backend.contains("cuda") {
            rest.parse::<usize>().ok()
        } else {
            None
        }
    } else if lower.starts_with("cuda") {
        lower
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .collect::<String>()
            .parse::<usize>()
            .ok()
    } else {
        None
    };
    ordinal.ok_or_else(|| {
        format!(
            "所选设备不是可由 Candle 绑定的 CUDA 设备：{selected}；请在 GPU 列表中选择 NVIDIA CUDA 设备"
        )
    })
}

pub struct CandleEmbedder {
    model: Mutex<EmbeddingGemma2>,
    model_dir: PathBuf,
    device: String,
}

impl CandleEmbedder {
    pub fn load(dir: &Path, selected_device: &str, batch_size: usize) -> Result<Self, String> {
        if !dir.join("config.json").is_file()
            || !dir.join("model.safetensors").is_file()
            || !dir.join("tokenizer.json").is_file()
        {
            return Err(format!("EmbeddingGemma2 目录缺少 config.json、model.safetensors 或 tokenizer.json：{}", dir.display()));
        }
        let mut options = Options::default();
        options.modalities = Modalities::ALL;
        // The model internally splits batches by sequence length and attention
        // cost, so a larger ceiling lets short documents keep the GPU busy
        // without forcing long documents into one oversized allocation.
        options.batch_size = batch_size.clamp(1, 128);
        #[cfg(feature = "candle-cuda")]
        {
            let ordinal = {
                #[cfg(feature = "llama")]
                {
                    // The UI persists the stable GGML device id, while
                    // Candle expects a CUDA ordinal. Resolve it from the
                    // backend enumeration so VRAM sorting cannot change it.
                    crate::ai::llama::cuda_device_ordinal(selected_device)?
                }
                #[cfg(not(feature = "llama"))]
                {
                    cuda_ordinal_fallback(selected_device)?
                }
            };
            options = options
                .with_cuda_device(ordinal)
                .map_err(|e| format!("Candle 选择 CUDA 设备 {ordinal} 失败：{e}"))?;
            options = options.with_bf16();
        }
        if !cfg!(feature = "candle-cuda") {
            let _ = selected_device;
            return Err(
                "当前程序未集成 Candle CUDA 后端；请使用包含 candle-cuda 的构建版本".into(),
            );
        }
        let device = format!("{:?}", options.device);
        if device.to_ascii_lowercase().contains("cpu") {
            return Err("EmbeddingGemma2 Candle 后端需要 CUDA；当前机器没有可用 Candle GPU".into());
        }
        if !device.to_ascii_lowercase().contains("cuda") {
            return Err(format!("所选设备与 Candle 后端不兼容：{selected_device}"));
        }
        let model = EmbeddingGemma2::from_dir(dir, options)
            .map_err(|e| format!("Candle 加载 EmbeddingGemma2 失败：{e}"))?;
        Ok(Self {
            model: Mutex::new(model),
            model_dir: dir.to_path_buf(),
            device,
        })
    }

    fn one(&self, input: Input, prompt: Option<&str>) -> Result<Vec<f32>, String> {
        let model = self.model.lock().map_err(|e| e.to_string())?;
        let tensor = model.encode(&[input], prompt).map_err(|e| format!("Candle 编码失败：{e}"))?;
        let rows = tensor.to_vec2::<f32>().map_err(|e| format!("读取 Candle 向量失败：{e}"))?;
        rows.into_iter().next().ok_or_else(|| "Candle 没有返回向量".into())
    }
}

impl Embedder for CandleEmbedder {
    fn info(&self) -> EmbedInfo {
        EmbedInfo {
            backend: "candle".into(),
            model: self.model_dir.display().to_string(),
            dim: 768,
            device: self.device.clone(),
            modalities: vec!["text".into(), "image".into(), "audio".into(), "video".into()],
            ready: true,
            detail: "EmbeddingGemma2 Candle 原生多模态后端 · 文本/图片/音频/视频共用 768 维空间".into(),
        }
    }

    fn dim(&self) -> usize { 768 }

    fn embed_text(&self, text: &str, query: bool) -> Result<Vec<f32>, String> {
        if !query {
            return self
                .embed_text_batch(&[text.to_owned()])?
                .into_iter()
                .next()
                .ok_or_else(|| "Candle 没有返回文本向量".into());
        }
        let model = self.model.lock().map_err(|e| e.to_string())?;
        let tensor = model.encode_queries(&[text]).map_err(|e| format!("Candle 文本编码失败：{e}"))?;
        tensor
            .to_vec2::<f32>()
            .map_err(|e| format!("读取 Candle 文本向量失败：{e}"))?
            .into_iter()
            .next()
            .ok_or_else(|| "Candle 没有返回文本向量".into())
    }

    fn embed_text_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        // Leave room for the model's task prefix under its 8192-token limit.
        const MAX_CHUNK_TOKENS: usize = 8_000;
        const MAX_CHUNK_CHARS: usize = 3_500;
        const OVERLAP_CHARS: usize = 128;
        let model = self.model.lock().map_err(|e| e.to_string())?;
        let tokenizer = model.tokenizer();
        let mut chunks = Vec::new();
        let mut owners = Vec::new();
        for (owner, text) in texts.iter().enumerate() {
            let chars: Vec<char> = text.chars().collect();
            if chars.is_empty() {
                chunks.push(String::new());
                owners.push(owner);
                continue;
            }
            let mut start = 0usize;
            while start < chars.len() {
                let mut end = (start + MAX_CHUNK_CHARS).min(chars.len());
                loop {
                    let chunk: String = chars[start..end].iter().collect();
                    let token_count = tokenizer
                        .encode(chunk.as_str(), true)
                        .map_err(|error| format!("Candle 文本分块计数失败：{error}"))?
                        .get_ids()
                        .len();
                    if token_count <= MAX_CHUNK_TOKENS {
                        chunks.push(chunk);
                        owners.push(owner);
                        break;
                    }
                    let span = end - start;
                    if span <= 1 {
                        return Err("Candle 文本存在超过模型上下文长度的单字符片段".into());
                    }
                    end = start + (span / 2).max(1);
                }
                if end == chars.len() {
                    break;
                }
                start = end.saturating_sub(OVERLAP_CHARS.min(end - start - 1));
            }
        }
        let rows = model
            .encode_documents(&chunks)
            .map_err(|e| format!("Candle 批量文本编码失败：{e}"))?
            .to_vec2::<f32>()
            .map_err(|e| format!("读取 Candle 批量向量失败：{e}"))?;
        let mut sums = vec![Vec::<f32>::new(); texts.len()];
        let mut counts = vec![0usize; texts.len()];
        for (row, &owner) in rows.into_iter().zip(&owners) {
            if sums[owner].is_empty() {
                sums[owner] = vec![0.0; row.len()];
            }
            if row.len() != sums[owner].len() {
                continue;
            }
            for (dst, value) in sums[owner].iter_mut().zip(row) {
                *dst += value;
            }
            counts[owner] += 1;
        }
        if counts.iter().any(|&count| count == 0) {
            return Err("Candle 批量编码没有返回完整的文档向量".into());
        }
        for (sum, &count) in sums.iter_mut().zip(&counts) {
            for value in sum.iter_mut() {
                *value /= count as f32;
            }
            crate::ai::l2_normalize(sum);
        }
        Ok(sums)
    }

    fn embed_image(&self, path: &Path) -> Result<Vec<f32>, String> {
        let image = image::open(path).map_err(|e| format!("图片读取失败：{e}"))?;
        self.one(Input::image(image), None)
    }

    fn embed_image_batch(&self, paths: &[&Path]) -> Result<Vec<Vec<f32>>, String> {
        let inputs: Vec<Input> = paths
            .par_iter()
            .map(|path| {
                image::open(path)
                    .map(Input::image)
                    .map_err(|error| format!("读取图片 {} 失败：{error}", path.display()))
            })
            .collect::<Result<_, _>>()?;
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.model.lock().map_err(|error| error.to_string())?;
        model
            .encode(&inputs, None)
            .map_err(|error| format!("Candle 批量图片编码失败：{error}"))?
            .to_vec2::<f32>()
            .map_err(|error| format!("读取 Candle 图片向量失败：{error}"))
    }

    fn embed_image_data(&self, image: &image::DynamicImage) -> Result<Vec<f32>, String> {
        self.one(Input::image(image.clone()), None)
    }

    fn embed_image_data_batch(
        &self,
        images: &[&image::DynamicImage],
    ) -> Result<Vec<Vec<f32>>, String> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let inputs: Vec<Input> = images.iter().map(|image| Input::image((*image).clone())).collect();
        let model = self.model.lock().map_err(|error| error.to_string())?;
        model
            .encode(&inputs, None)
            .map_err(|error| format!("Candle 批量内存图片编码失败：{error}"))?
            .to_vec2::<f32>()
            .map_err(|error| format!("读取 Candle 批量图片向量失败：{error}"))
    }

    fn embed_audio(&self, path: &Path) -> Result<Vec<f32>, String> {
        let samples = crate::media::load_audio(path, 30.0).ok_or("音频解码失败（需要 ffmpeg）")?;
        self.one(Input::audio(samples), None)
    }

    fn embed_video(&self, path: &Path) -> Result<Vec<f32>, String> {
        // 与 index.rs 用同一套智能抽帧：场景切换/I 帧优先 + dHash 去重，
        // 避免「固定均匀切 32 帧」在长静止镜头上浪费算力、在快剪视频上漏画面。
        let frames = crate::media::sample_video_frames_smart(
            path,
            crate::media::FramePlan { interval: 4.0, max_frames: 32, max_side: 1024, dedup_distance: 6 },
        );
        if frames.is_empty() {
            return Err("视频抽帧失败（需要 ffmpeg）".into());
        }
        let mut images = Vec::with_capacity(frames.len());
        for frame in &frames {
            if let Ok(image) = image::open(frame) {
                images.push(image);
            }
            let _ = std::fs::remove_file(frame);
        }
        if images.is_empty() {
            return Err("视频帧读取失败".into());
        }
        self.one(Input::video(images), None)
    }
}
