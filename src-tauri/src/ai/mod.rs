//! 向量化抽象层。
//!
//! - `LlamaEmbedder`：通过 llama.cpp 的 C API 做文本/图片 embedding（真模型）
//! - `HashEmbedder`：无模型时的确定性兜底（文本 n-gram + 图片颜色直方图 + 音视频字节特征）
pub mod bpe;
pub mod clap;
pub mod clip;
pub mod hash;
#[cfg(feature = "llama")]
pub mod llama;
pub mod ort;

use std::path::Path;

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
    /// 文本向量；`query=true` 时加检索指令前缀。
    fn embed_text(&self, text: &str, query: bool) -> Result<Vec<f32>, String>;
    fn embed_image(&self, path: &Path) -> Result<Vec<f32>, String>;
    fn embed_audio(&self, path: &Path) -> Result<Vec<f32>, String>;
    fn embed_video(&self, path: &Path) -> Result<Vec<f32>, String>;
}

/// 按文件类型分发到对应模态。
pub fn embed_path(e: &dyn Embedder, path: &Path, kind: &str) -> Result<Vec<f32>, String> {
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

/// 按配置构建 embedding 后端。
///
/// `embed_model` 指向 `.gguf` 文件且构建时启用了 `llama` feature 时用
/// llama.cpp，否则回退到 hash 基线（保证任何机器上都能跑起来）。
pub fn build_embedder(cfg: &crate::config::ModelConfig) -> Result<Box<dyn Embedder>, String> {
    let path = cfg.embed_model.trim();
    #[cfg(feature = "llama")]
    {
        if path.to_lowercase().ends_with(".gguf") && Path::new(path).exists() {
            return match llama::LlamaEmbedder::load(
                path,
                cfg.n_gpu_layers,
                cfg.context_size as i32,
                cfg.n_threads,
            ) {
                Ok(e) => Ok(Box::new(e)),
                Err(e) => Err(format!("llama.cpp 加载失败：{e}")),
            };
        }
        if path.is_empty() {
            return Ok(Box::new(hash::HashEmbedder::new()));
        }
        return Err(format!(
            "模型文件不存在或不是 .gguf：{path}\n请先在「模型」页转换或选择 GGUF 文件"
        ));
    }
    #[cfg(not(feature = "llama"))]
    {
        let _ = path;
        Ok(Box::new(hash::HashEmbedder::new()))
    }
}
