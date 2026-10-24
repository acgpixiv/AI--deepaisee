//! 视觉描述：给图片/视频生成中文关键词标签。
//!
//! llama.cpp 的 EmbeddingGemma2 只跑了文本塔，视觉塔的输出与文本向量不在同一空间
//! （实测余弦 0.03~0.09，无区分度），所以「一句话描述找图片」没法用向量做。
//!
//! 这里退一步，用可解释的视觉统计（HSV 色彩分布、亮度、饱和度、上下分区、
//! 边缘密度）生成一批中文标签写进 `files.tags`，让用户仍能用
//! 「蓝天」「白云」「草地」「森林」「雪景」「夜景」「夕阳」「海洋」「截图」
//! 这类词把图片找出来 —— 覆盖面窄，但描述准确、零幻觉、不需要额外模型。
use std::path::Path;

/// 一张图的视觉画像。
#[derive(Debug, Clone, Default)]
pub struct VisualProfile {
    /// 平均色相（0~360），饱和度过低时为 None
    pub hue: Option<f32>,
    /// 平均饱和度 0~1
    pub sat: f32,
    /// 平均亮度 0~1
    pub val: f32,
    /// 上半区平均亮度
    pub top_val: f32,
    /// 下半区平均亮度
    pub bottom_val: f32,
    /// 上蓝下非蓝（典型的天空-地面构图）
    pub sky_layout: bool,
    /// 上半区暖色占比（日落/黄昏的天空）
    pub top_warm: f32,
    /// 下半区暖色占比
    pub bot_warm: f32,
    /// 高饱和蓝色占比
    pub blue: f32,
    /// 绿色占比
    pub green: f32,
    /// 暖色（红/橙/黄）占比
    pub warm: f32,
    /// 低饱和高亮（白/浅灰）占比
    pub white: f32,
    /// 低饱和低亮（黑/深灰）占比
    pub dark: f32,
    /// 低饱和中亮（灰）占比
    pub gray: f32,
    /// 边缘密度 0~1（越高越"碎"，照片/纹理高，纯色/截图低）
    pub edge: f32,
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let d = max - min;
    let v = max;
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    let h = if d <= f32::EPSILON {
        0.0
    } else if (max - rf).abs() < f32::EPSILON {
        60.0 * (((gf - bf) / d) % 6.0)
    } else if (max - gf).abs() < f32::EPSILON {
        60.0 * (((bf - rf) / d) + 2.0)
    } else {
        60.0 * (((rf - gf) / d) + 4.0)
    };
    (if h < 0.0 { h + 360.0 } else { h }, s, v)
}

/// 统计一张已解码的图像。
pub fn analyze(img: &image::RgbImage) -> VisualProfile {
    let (w, h) = (img.width().max(1), img.height().max(1));
    // 采样步长：大图不必逐像素，64x64 网格足够稳。
    let step_x = (w / 64).max(1);
    let step_y = (h / 64).max(1);

    let mut n = 0.0f64;
    let mut sum_hx = 0.0f64; // 色相用「余弦/正弦」加权平均，避免 0/360 断裂
    let mut sum_hy = 0.0f64;
    let mut sum_s = 0.0f64;
    let mut sum_v = 0.0f64;
    let (mut top_v, mut top_n, mut bot_v, mut bot_n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut blue, mut green, mut warm, mut white, mut dark, mut gray) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut top_blue, mut bot_blue) = (0.0f64, 0.0f64);
    let (mut top_warm, mut bot_warm) = (0.0f64, 0.0f64);

    for y in (0..h).step_by(step_y as usize) {
        for x in (0..w).step_by(step_x as usize) {
            let p = img.get_pixel(x, y).0;
            let (hue, s, v) = rgb_to_hsv(p[0], p[1], p[2]);
            let rad = hue.to_radians();
            let wgt = (s * 1.5 + 0.25) as f64; // 饱和度高的像素更能代表主色
            sum_hx += rad.cos() as f64 * wgt;
            sum_hy += rad.sin() as f64 * wgt;
            sum_s += s as f64;
            sum_v += v as f64;
            n += 1.0;
            if y < h / 2 {
                top_v += v as f64;
                top_n += 1.0;
                if s > 0.35 && (190.0..=260.0).contains(&hue) {
                    top_blue += 1.0;
                }
                if s > 0.18 && (hue < 70.0 || hue >= 330.0) {
                    top_warm += 1.0;
                }
            } else {
                bot_v += v as f64;
                bot_n += 1.0;
                if s > 0.35 && (190.0..=260.0).contains(&hue) {
                    bot_blue += 1.0;
                }
                if s > 0.18 && (hue < 70.0 || hue >= 330.0) {
                    bot_warm += 1.0;
                }
            }
            // 分类占比
            if s >= 0.35 {
                if (190.0..=260.0).contains(&hue) {
                    blue += 1.0;
                } else if (70.0..=170.0).contains(&hue) {
                    green += 1.0;
                } else if hue < 70.0 || hue >= 330.0 {
                    warm += 1.0;
                }
            }
            if s < 0.18 {
                if v > 0.72 {
                    white += 1.0;
                } else if v < 0.22 {
                    dark += 1.0;
                } else {
                    gray += 1.0;
                }
            }
        }
    }

    let n = n.max(1.0);
    let mut prof = VisualProfile {
        hue: if sum_s / n > 0.12 {
            let mut ang = sum_hy.atan2(sum_hx).to_degrees() as f32;
            if ang < 0.0 {
                ang += 360.0;
            }
            Some(ang)
        } else {
            None
        },
        sat: (sum_s / n) as f32,
        val: (sum_v / n) as f32,
        top_val: (top_v / top_n.max(1.0)) as f32,
        bottom_val: (bot_v / bot_n.max(1.0)) as f32,
        blue: (blue / n) as f32,
        green: (green / n) as f32,
        warm: (warm / n) as f32,
        white: (white / n) as f32,
        dark: (dark / n) as f32,
        gray: (gray / n) as f32,
        top_warm: (top_warm / top_n.max(1.0)) as f32,
        bot_warm: (bot_warm / bot_n.max(1.0)) as f32,
        ..Default::default()
    };
    prof.sky_layout = prof.top_val > prof.bottom_val + 0.12
        && top_blue / top_n.max(1.0) > 0.30
        && bot_blue / bot_n.max(1.0) < top_blue / top_n.max(1.0) * 0.7;

    // 边缘密度：相邻像素亮度差的均值（稀疏采样）
    let mut edge = 0.0f64;
    let mut en = 0.0f64;
    for y in (0..h.saturating_sub(1)).step_by(step_y as usize) {
        for x in (0..w.saturating_sub(1)).step_by(step_x as usize) {
            let a = img.get_pixel(x, y).0;
            let b = img.get_pixel(x + 1, y).0;
            let la = (a[0] as f32 + a[1] as f32 + a[2] as f32) / 765.0;
            let lb = (b[0] as f32 + b[1] as f32 + b[2] as f32) / 765.0;
            edge += (la - lb).abs() as f64;
            en += 1.0;
        }
    }
    prof.edge = (edge / en.max(1.0) * 4.0).min(1.0) as f32;
    prof
}

/// 把视觉画像翻译成中文标签（每个都对应一个用户会输入的说法）。
pub fn tags_of(p: &VisualProfile, kind: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();

    if kind == "video" {
        out.push("视频");
    } else if kind == "image" {
        out.push("图片");
    }

    // ---- 整体明暗
    if p.val < 0.18 {
        out.push("夜景");
        out.push("暗色");
    } else if p.val > 0.82 && p.sat < 0.15 {
        out.push("白色背景");
        out.push("明亮");
    }

    // ---- 色系（用户最常用的搜索词）
    if p.blue > 0.16 {
        if p.blue > 0.30 {
            out.push("蓝色");
        }
        if p.sky_layout {
            out.push("蓝天");
            if p.white > 0.06 {
                out.push("蓝天白云");
                out.push("白云");
            }
        } else if p.blue > 0.60 {
            // 大面积纯蓝：可能是纯色壁纸，也可能是海。不强行判定，
            // 只用「蓝色」+「背景图」（构图阶段会补）表达。
        } else if p.val < 0.62 || p.blue > 0.55 {
            // 大面积蓝、且不是「亮天空」→ 水面
            out.push("海洋");
            out.push("水");
        }
        if p.top_val > p.bottom_val + 0.12 && p.blue > 0.30 {
            out.push("天空");
        }
    }
    if p.green > 0.06 {
        if p.green > 0.28 {
            out.push("绿色");
            if p.edge > 0.12 {
                out.push("森林");
                out.push("树木");
            } else {
                out.push("草地");
            }
        }
        if p.blue > 0.05 || p.green > 0.25 {
            out.push("自然");
            out.push("户外");
        }
        if p.blue > 0.08 && p.sky_layout {
            out.push("草原");
            out.push("风景");
        }
    }
    if p.warm > 0.22 {
        out.push("暖色");
        if p.hue.map(|h| h < 55.0 || h > 330.0).unwrap_or(false) && p.warm > 0.3 {
            out.push("红色");
        }
        // 夕阳/黄昏：上半区明显比下半区暖（天空被落日染红），
        // 或者整体暖且偏暗。纯靠 warm 总占比会漏掉「红天+暗水面」
        // 这类构图（实测 natural 照片 warm 只有 0.26）。
        let warm_sky = p.top_warm > 0.30 && p.top_warm > p.bot_warm + 0.15;
        if warm_sky || (p.warm > 0.30 && p.val < 0.55) {
            out.push("夕阳");
            out.push("黄昏");
            out.push("晚霞");
            if p.val < 0.35 {
                out.push("日落");
            }
        }
    }
    if p.white > 0.55 {
        out.push("白色");
        if p.val > 0.75 {
            out.push("雪景");
            out.push("冰雪");
        }
    }
    if p.gray > 0.45 {
        out.push("灰色");
        if p.sat < 0.08 {
            out.push("黑白");
            out.push("单色");
        }
    }
    if p.dark > 0.45 {
        out.push("黑色");
    }

    // ---- 构图/用途
    // 纯色/背景图：几乎没有边缘，且颜色高度单一（可能是高饱和的纯蓝/纯绿）。
    let mono = p.blue > 0.6 || p.green > 0.6 || p.warm > 0.6 || p.dark > 0.6 || p.white > 0.6;
    if p.edge < 0.03 && (p.sat < 0.25 || mono) {
        out.push("纯色");
        out.push("背景图");
        out.push("壁纸");
    }
    if p.white > 0.62 && p.edge > 0.06 && p.sat < 0.22 {
        out.push("截图");
        out.push("文档");
    }
    if p.sat > 0.45 && p.edge > 0.10 && p.val > 0.30 {
        out.push("色彩鲜艳");
        out.push("照片");
    }
    // 雪山/冰雪：大面积白 + 偏冷色相
    if p.white > 0.18 && p.hue.map(|h| (180.0..=280.0).contains(&h)).unwrap_or(false) {
        out.push("雪景");
        out.push("冰雪");
        out.push("雪山");
    }
    // 水面倒影/湖泊：下半区明显比上半区暗，且整体偏冷
    if p.bottom_val + 0.10 < p.top_val
        && p.hue.map(|h| (180.0..=280.0).contains(&h)).unwrap_or(false)
        && p.sat > 0.25
    {
        out.push("湖");
        out.push("倒影");
    }

    // ---- 通俗整体描述（拼一句可以直接搜的说法）
    if p.blue > 0.25 && p.sky_layout {
        out.push("天空");
    }
    if p.green > 0.06 && p.blue < 0.10 {
        out.push("自然");
        out.push("户外");
    }

    out.sort_unstable();
    out.dedup();
    out
}

/// 直接读文件并返回标签（失败返回空）。
pub fn describe_file(path: &Path, kind: &str) -> Vec<String> {
    let img = match image::open(path) {
        Ok(i) => i.to_rgb8(),
        Err(_) => return Vec::new(),
    };
    let prof = analyze(&img);
    tags_of(&prof, kind).into_iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_blue_sky() {
        let mut img = image::RgbImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let (r, g, b) = if y < 40 { (110, 170, 235) } else { (60, 130, 60) };
                img.put_pixel(x, y, image::Rgb([r, g, b]));
            }
        }
        let p = analyze(&img);
        let tags = tags_of(&p, "image");
        assert!(tags.contains(&"蓝天"), "tags = {tags:?}, profile = {p:?}");
        assert!(tags.contains(&"绿色"), "tags = {tags:?}");
    }
}
