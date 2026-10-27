//! 无模型兜底向量：确定性哈希特征。
//!
//! 文本用字符 2-gram + 词哈希；图片用 8x8 缩略图 + RGB 直方图；
//! 音频用 PCM 能量分段；视频用抽帧后的图片特征平均。
use super::{l2_normalize, EmbedInfo, Embedder};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

const DIM: usize = 256;

fn h64(s: &str, salt: u64) -> u64 {
    let mut h = DefaultHasher::new();
    salt.hash(&mut h);
    s.hash(&mut h);
    h.finish()
}

fn add_feature(v: &mut [f32], key: &str, weight: f32) {
    let a = h64(key, 1) as usize % v.len();
    let b = h64(key, 2) as usize % v.len();
    let sign = if h64(key, 3) & 1 == 0 { 1.0 } else { -1.0 };
    v[a] += weight * sign;
    v[b] += weight * 0.6 * sign;
}

pub struct HashEmbedder;

impl HashEmbedder {
    pub fn new() -> Self {
        Self
    }

    fn text_vec(&self, text: &str, query: bool) -> Vec<f32> {
        let mut v = vec![0.0f32; DIM];
        let low = text.to_lowercase();
        let prefix = if query { "task: search result | query: " } else { "title: none | text: " };
        add_feature(&mut v, prefix, 0.2);
        // 词
        for w in low.split(|c: char| !c.is_alphanumeric()) {
            if w.is_empty() {
                continue;
            }
            add_feature(&mut v, w, 1.0);
            if w.chars().count() > 3 {
                for i in 0..w.len().saturating_sub(2) {
                    if let Some(s) = w.get(i..i + 2) {
                        add_feature(&mut v, s, 0.35);
                    }
                }
            }
        }
        // 中文 2-gram
        let chars: Vec<char> = low.chars().collect();
        for i in 0..chars.len().saturating_sub(1) {
            let pair: String = chars[i..i + 2].iter().collect();
            add_feature(&mut v, &pair, 0.9);
        }
        l2_normalize(&mut v);
        v
    }

    fn image_vec(&self, path: &Path) -> Result<Vec<f32>, String> {
        let img = image::open(path).map_err(|e| format!("图片读取失败: {e}"))?;
        Ok(self.image_vec_from(&img))
    }

    fn image_vec_from(&self, img: &image::DynamicImage) -> Vec<f32> {
        let small = img.resize_exact(8, 8, image::imageops::FilterType::Triangle).to_rgb8();
        let mut v = vec![0.0f32; DIM];
        let mut i = 0;
        for px in small.pixels() {
            let [r, g, b] = px.0;
            v[i % DIM] += r as f32 / 255.0;
            v[(i + 1) % DIM] += g as f32 / 255.0 * 0.7;
            v[(i + 2) % DIM] += b as f32 / 255.0 * 0.5;
            i += 3;
        }
        // RGB 直方图（每通道 16 桶）
        let full = img.to_rgb8();
        let mut hist = [[0.0f32; 16]; 3];
        let total = (full.width() * full.height()).max(1) as f32;
        for px in full.pixels() {
            let [r, g, b] = px.0;
            hist[0][(r >> 4) as usize] += 1.0;
            hist[1][(g >> 4) as usize] += 1.0;
            hist[2][(b >> 4) as usize] += 1.0;
        }
        for c in 0..3 {
            for b in 0..16 {
                v[192 + c * 16 + b] = hist[c][b] / total * 4.0;
            }
        }
        // 亮度均值/方差
        let mut mean = 0.0f32;
        let mut sq = 0.0f32;
        for px in full.pixels() {
            let l = (px.0[0] as f32 + px.0[1] as f32 + px.0[2] as f32) / 765.0;
            mean += l;
            sq += l * l;
        }
        mean /= total;
        sq /= total;
        v[0] += mean;
        v[1] += (sq - mean * mean).max(0.0).sqrt();
        l2_normalize(&mut v);
        v
    }

    fn audio_vec(&self, path: &Path) -> Result<Vec<f32>, String> {
        let samples = crate::media::load_audio(path, 8.0).ok_or("音频解码失败（需要 ffmpeg）")?;
        if samples.is_empty() {
            return Err("音频为空".into());
        }
        let mut v = vec![0.0f32; DIM];
        let seg = (samples.len() / DIM).max(1);
        for (i, chunk) in samples.chunks(seg).take(DIM).enumerate() {
            let energy = chunk.iter().map(|x| x.abs()).sum::<f32>() / chunk.len() as f32;
            v[i] = energy.sqrt();
        }
        l2_normalize(&mut v);
        Ok(v)
    }

    fn video_vec(&self, path: &Path) -> Result<Vec<f32>, String> {
        // 兜底特征用智能抽帧（去重后帧数更少也更代表画面），
        // 尺寸压到 128 够算颜色直方图，速度优先。
        let frames = crate::media::sample_video_frames_smart(
            path,
            crate::media::FramePlan { interval: 4.0, max_frames: 6, max_side: 128, dedup_distance: 6 },
        );
        if frames.is_empty() {
            return Err("视频抽帧失败（需要 ffmpeg）".into());
        }
        let mut acc = vec![0.0f32; DIM];
        for f in &frames {
            if let Ok(v) = self.image_vec(f) {
                for (i, x) in v.iter().enumerate() {
                    acc[i] += x;
                }
            }
            let _ = std::fs::remove_file(f);
        }
        l2_normalize(&mut acc);
        Ok(acc)
    }
}

impl Embedder for HashEmbedder {
    fn info(&self) -> EmbedInfo {
        EmbedInfo {
            backend: "hash".into(),
            model: "内置哈希基线".into(),
            dim: DIM,
            device: "cpu".into(),
            // Hash media features are only a fallback for similarity; they do
            // not place media in the model's shared text space.
            modalities: vec!["text".into()],
            ready: true,
            detail: "无需模型即可使用的确定性特征，仅用于兜底/自检".into(),
        }
    }
    fn dim(&self) -> usize {
        DIM
    }
    fn embed_text(&self, text: &str, query: bool) -> Result<Vec<f32>, String> {
        Ok(self.text_vec(text, query))
    }
    fn embed_image(&self, path: &Path) -> Result<Vec<f32>, String> {
        self.image_vec(path)
    }
    fn embed_image_data(&self, image: &image::DynamicImage) -> Result<Vec<f32>, String> {
        Ok(self.image_vec_from(image))
    }
    fn embed_audio(&self, path: &Path) -> Result<Vec<f32>, String> {
        self.audio_vec(path)
    }
    fn embed_video(&self, path: &Path) -> Result<Vec<f32>, String> {
        self.video_vec(path)
    }
}
