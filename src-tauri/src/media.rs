//! 媒体处理：用内置 ffmpeg 抽帧/解码音频，生成缩略图。
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 找 ffmpeg：优先 exe 同级的 tools/bin/ffmpeg.exe，其次 PATH。
pub fn ffmpeg_path() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        // 从 exe 所在目录逐级向上找 4 层：debug 版 exe 在 src-tauri/target/debug，
        // 打包版在 <app>/ 下，两种情况 tools/bin 都可能在更上层
        for _ in 0..4 {
            let Some(d) = dir else { break };
            cands.push(d.join("tools").join("bin").join("ffmpeg.exe"));
            cands.push(d.join("ffmpeg.exe"));
            cands.push(d.join("resources").join("ffmpeg.exe"));
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd);
        for _ in 0..4 {
            let Some(d) = dir else { break };
            cands.push(d.join("tools").join("bin").join("ffmpeg.exe"));
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    for c in cands {
        if c.is_file() {
            return Some(c);
        }
    }
    if Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok() {
        return Some(PathBuf::from("ffmpeg"));
    }
    None
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 临时工作目录：优先 exe 同级的 `data/tmp`，退回系统 %TEMP%。
///
/// 为什么要自己找：有些机器的 %TEMP%（漫游配置文件损坏 / 安全软件锁目录）
/// 建不了子目录，抽帧命令会静默失败，表现为「视频永远索引不出向量」——
/// 用户完全看不出原因。放在自己的数据目录下则一定能写。
fn work_dir() -> PathBuf {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            cands.push(d.join("data").join("tmp"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        cands.push(cwd.join("data").join("tmp"));
    }
    cands.push(std::env::temp_dir().join("deepaisee"));
    for d in cands {
        if std::fs::create_dir_all(&d).is_ok() {
            return d;
        }
    }
    std::env::temp_dir()
}

/// 抽视频帧到临时目录，返回图片路径列表。
pub fn sample_video_frames(path: &Path, count: usize, max_side: u32) -> Vec<PathBuf> {
    let Some(ff) = ffmpeg_path() else { return vec![] };
    let base = work_dir();
    sweep_old_frames(&base);
    let tmp = base.join(format!("frames-{}", std::process::id()));
    if std::fs::create_dir_all(&tmp).is_err() {
        return vec![];
    }
    let mut out = Vec::new();
    // 先取时长
    let dur = video_duration(path).unwrap_or(4.0);
    let n = count.max(1);
    for i in 0..n {
        let t = dur * (i as f64 + 0.5) / n as f64;
        let dest = tmp.join(format!("f{i}.jpg"));
        let scale = format!("scale='min({},iw)':'min({},ih)':force_original_aspect_ratio=decrease", max_side, max_side);
        let ok = hidden(
            Command::new(&ff)
                .args(["-y", "-ss", &format!("{t:.3}"), "-i"])
                .arg(path)
                .args(["-frames:v", "1", "-vf", &scale, "-q:v", "3"])
                .arg(&dest)
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
        if ok && dest.is_file() {
            out.push(dest);
        }
    }
    if out.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    out
}

/// 智能抽帧参数。需求：按时间间隔抽帧（默认至少 4 秒一帧）、I 帧优先、
/// 场景切换优先、图像指纹去重、总帧数封顶。
#[derive(Debug, Clone, Copy)]
pub struct FramePlan {
    /// 相邻采样点的最小时间间隔（秒），内部强制不低于 4 秒。
    pub interval: f32,
    /// 单个视频最多保留多少帧。
    pub max_frames: u32,
    /// 输出图片的最长边。
    pub max_side: u32,
    /// dHash 汉明距离小于等于该值时视为同一画面丢弃。
    pub dedup_distance: u32,
}

impl Default for FramePlan {
    fn default() -> Self {
        Self { interval: 4.0, max_frames: 64, max_side: 256, dedup_distance: 6 }
    }
}

/// 从 ffmpeg 的 showinfo 输出里抓所有 `pts_time:` 后面的秒数。
fn parse_showinfo_times(text: &str) -> Vec<f64> {
    let mut times = Vec::new();
    for line in text.lines() {
        // [Parsed_showinfo_1 @ ...] n:   0 pts:      0 pts_time:0       duration:...
        let Some(pos) = line.find("pts_time:") else { continue };
        let rest = &line[pos + 9..];
        let value: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        if let Ok(seconds) = value.parse::<f64>() {
            if seconds.is_finite() && seconds >= 0.0 {
                times.push(seconds);
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    times
}

/// 视频关键帧（I 帧）时间戳列表，秒。
///
/// 用 `-skip_frame nokey` 只解码关键帧，长视频也能在一两秒内跑完；
/// 关键帧天然落在场景切换处，既满足「I 帧优先」，又顺带拿到场景变化点。
/// 失败时返回空列表，调用方退回均匀采样（不能因为探测失败就不抽帧）。
fn video_keyframe_times(path: &Path) -> Vec<f64> {
    let Some(ff) = ffmpeg_path() else { return vec![] };
    let out = hidden(
        Command::new(&ff)
            .args(["-hide_banner", "-nostats", "-loglevel", "info", "-skip_frame", "nokey", "-i"])
            .arg(path)
            .args([
                "-vf",
                "select=eq(pict_type\\,I),showinfo",
                "-an",
                "-f",
                "null",
                "-",
            ]),
    )
    .output();
    let Ok(out) = out else { return vec![] };
    parse_showinfo_times(&String::from_utf8_lossy(&out.stderr))
}

/// 场景切换时间戳（Scene Change Detection），秒。
///
/// 需求里的「动作片提高采样率、PPT/网课降采样」就靠它判断：
/// ffmpeg 的 `scene` 分数是相邻帧的直方图差异，超过阈值即视为换镜头。
///
/// 为控制开销做了三件事：先 `fps=2` 降到每秒 2 帧再算（scene 分数只看相邻帧，
/// 降帧后差异只会更明显，阈值相应取 0.4）；缩到 160 宽；超过 40 分钟直接跳过
/// （长视频仍然有 I 帧可以挑，不影响「I 帧优先」）。
fn video_scene_change_times(path: &Path, dur: f64) -> Vec<f64> {
    if dur > 2400.0 {
        return vec![];
    }
    let Some(ff) = ffmpeg_path() else { return vec![] };
    let out = hidden(
        Command::new(&ff)
            .args(["-hide_banner", "-nostats", "-loglevel", "info", "-i"])
            .arg(path)
            .args([
                "-vf",
                "fps=2,scale=160:-2,select='gt(scene\\,0.4)',showinfo",
                "-an",
                "-f",
                "null",
                "-",
            ]),
    )
    .output();
    let Ok(out) = out else { return vec![] };
    parse_showinfo_times(&String::from_utf8_lossy(&out.stderr))
}

/// 按场景切换密度自适应调整采样间隔（秒）。
///
/// - 几乎不换镜头（PPT / 网课 / 录屏）：放稀到 1.5 倍，省 CLIP 计算量与磁盘；
/// - 换镜头很频繁（动作片 / 体育）：缩到 0.5 倍，尽量不漏关键画面；
/// - 中间档保持配置值。
///
/// 需求要求「至少每 4 秒一帧」，所以任何情况下都不低于 4 秒。
fn adaptive_interval(base: f64, dur: f64, scene_count: usize) -> f64 {
    if dur < 1.0 {
        return base.max(4.0);
    }
    if scene_count == 0 {
        return (base * 1.5).max(4.0);
    }
    let per_minute = scene_count as f64 * 60.0 / dur;
    if per_minute >= 8.0 {
        return (base * 0.5).max(4.0);
    }
    base.max(4.0)
}

/// 在指定时间点抽一帧到 `dest`。
fn grab_frame(ff: &Path, src: &Path, seconds: f64, max_side: u32, dest: &Path) -> bool {
    let scale = format!(
        "scale='min({m},iw)':'min({m},ih)':force_original_aspect_ratio=decrease",
        m = max_side
    );
    hidden(
        Command::new(ff)
            .args(["-y", "-ss", &format!("{seconds:.3}"), "-i"])
            .arg(src)
            .args(["-frames:v", "1", "-vf", &scale, "-q:v", "4"])
            .arg(dest)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .status()
    .map(|s| s.success())
    .unwrap_or(false)
        && dest.is_file()
}

/// 图像指纹（dHash）：缩到 9x8 灰度，比较相邻像素亮度，得到 64 位指纹。
///
/// 不用 phash 是为了零依赖、零调参：dHash 对缩放/轻微压缩不敏感，
/// 对画面变化敏感，正好用来丢弃「同一镜头里的连续帧」。
pub fn dhash(path: &Path) -> Option<u64> {
    frame_signature(path).map(|sig| sig.0)
}

/// 帧特征：dHash 指纹 + 平均颜色。
///
/// 只用 dHash 时，纯色 / 极低对比的画面（幻灯片白底、黑屏、纯色转场）
/// 几乎都会得到同一个指纹，"红底切绿底" 会被误判成同一帧直接丢掉。
/// 带上平均颜色就能区分这类切换 —— 两帧既要求结构相近、又要求颜色相近
/// 才算重复。
fn frame_signature(path: &Path) -> Option<(u64, [u8; 3])> {
    let img = image::open(path).ok()?;
    let small = img
        .resize_exact(9, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mut hash = 0u64;
    let mut bit = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let left = small.get_pixel(x, y).0[0];
            let right = small.get_pixel(x + 1, y).0[0];
            if left > right {
                hash |= 1u64 << bit;
            }
            bit += 1;
        }
    }
    let mean = img
        .resize_exact(1, 1, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let rgb = mean.get_pixel(0, 0).0;
    Some((hash, rgb))
}

fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 两帧平均颜色的距离（RGB 三分量差的平方和，满值 195075）。
/// 阈值取 300 相当于每通道平均差 ~10/255，轻微偏色不算换画面。
fn color_distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let d = *x as i32 - *y as i32;
            (d * d) as u32
        })
        .sum()
}

/// 智能抽帧：自适应间隔 + 场景切换/I 帧优先 + 图像指纹去重 + 帧数封顶。
///
/// 和旧版「按 index 均匀切 N 份」的区别：
/// 1. 采样点按**时间间隔**给，长视频不会因为 `video_frames` 太小而只抽到几帧；
/// 2. 间隔本身按**镜头切换密度**自适应：PPT/网课这类几乎不换镜头的视频放稀，
///    动作片/体育这类高频换镜头的视频加密（下限仍是 4 秒）；
/// 3. 每个采样窗口内优先落在**场景切换点**上，其次才是 I 帧
///    （画面最清晰、最可能是镜头切换点）；
/// 4. 抽出来的帧算 dHash，和上一张几乎一样的直接丢掉 —— 一段静止镜头
///    只会留一张，省掉大量重复的 CLIP 计算和缩略图；
/// 5. 总帧数按 `max_frames` 封顶，超出时自动放稀采样间隔。
pub fn sample_video_frames_smart(path: &Path, plan: FramePlan) -> Vec<PathBuf> {
    let Some(ff) = ffmpeg_path() else { return vec![] };
    let base = work_dir();
    sweep_old_frames(&base);
    let tmp = base.join(format!("frames-{}", std::process::id()));
    if std::fs::create_dir_all(&tmp).is_err() {
        return vec![];
    }

    let dur = video_duration(path).unwrap_or(4.0).max(0.1);
    // 关键帧与场景切换时间戳（探测失败就是空表，下面自然退回窗口中心点）
    let keys = video_keyframe_times(path);
    let scenes = video_scene_change_times(path, dur);

    // 需求明确「至少每 4 秒一帧」，这里兜住配置里填了 0/负数的情况。
    let interval = adaptive_interval((plan.interval as f64).max(4.0), dur, scenes.len());
    // 排障开关：设了 DEEPAISEE_DEBUG_FRAMES 就把探测结果打到 stderr。
    if std::env::var_os("DEEPAISEE_DEBUG_FRAMES").is_some() {
        eprintln!(
            "[frames] {} dur={dur:.2}s dhash_dedup={} scenes={} keys={} interval={interval:.2}s",
            path.display(),
            plan.dedup_distance,
            scenes.len(),
            keys.len()
        );
    }
    let max_frames = plan.max_frames.clamp(1, 512) as usize;
    let wanted = ((dur / interval).ceil() as usize).clamp(1, max_frames);
    // 帧数封顶后实际间隔会被拉大，避免长视频仍然按 4 秒切出几万帧。
    let step = (dur / wanted as f64).max(interval);

    // 窗口内挑一个最靠近中心的候选时间点：场景切换点 > I 帧 > 窗口中心。
    let pick_in = |candidates: &[f64], start: f64, end: f64, center: f64| -> Option<f64> {
        candidates
            .iter()
            .copied()
            .filter(|t| *t >= start && *t < end)
            .min_by(|a, b| {
                (a - center)
                    .abs()
                    .partial_cmp(&(b - center).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    };

    let mut out: Vec<PathBuf> = Vec::new();
    let mut last_sig: Option<(u64, [u8; 3])> = None;
    for i in 0..wanted {
        let start = i as f64 * step;
        let end = ((i + 1) as f64 * step).min(dur);
        let center = (start + end) / 2.0;
        let pick = pick_in(&scenes, start, end, center)
            .or_else(|| pick_in(&keys, start, end, center))
            .unwrap_or(center);
        let dest = tmp.join(format!("f{i}.jpg"));
        if !grab_frame(&ff, path, pick, plan.max_side, &dest) {
            continue;
        }
        match frame_signature(&dest) {
            Some((hash, rgb)) => {
                if let Some((prev_hash, prev_rgb)) = last_sig {
                    // 结构相近 + 颜色也相近才算「同一画面」，纯色切换不会误丢。
                    if hamming(prev_hash, hash) <= plan.dedup_distance
                        && color_distance(prev_rgb, rgb) <= 300
                    {
                        let _ = std::fs::remove_file(&dest);
                        continue;
                    }
                }
                last_sig = Some((hash, rgb));
                out.push(dest);
            }
            // 算不出指纹（解码异常）就保守保留，宁多勿漏。
            None => out.push(dest),
        }
    }
    if out.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    out
}
/// 清掉上次运行残留的 `frames-<pid>` 目录。
///
/// 每个进程只用自己的 pid 目录，正常结束会删；但程序被强杀 / 崩溃时
/// 目录会留下来，日积月累能堆出几百兆的 jpg。启动新一轮抽帧时顺手清掉
/// 那些「不是当前进程」的旧目录（失败也无所谓，下次再清）。
fn sweep_old_frames(base: &Path) {
    let Ok(rd) = std::fs::read_dir(base) else { return };
    let me = format!("frames-{}", std::process::id());
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("frames-") && name != me {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// 视频时长（秒）。
pub fn video_duration(path: &Path) -> Option<f64> {
    let ff = ffmpeg_path()?;
    let out = hidden(Command::new(&ff).arg("-i").arg(path).args(["-f", "null", "-"]))
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    // Duration: 00:00:04.00,
    let idx = text.find("Duration: ")?;
    let rest = &text[idx + 10..];
    let end = rest.find(',')?;
    let dur = &rest[..end];
    let parts: Vec<&str> = dur.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].trim().parse().ok()?;
    let m: f64 = parts[1].trim().parse().ok()?;
    let s: f64 = parts[2].trim().parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// 把音频转成 16k 单声道 f32 采样。
pub fn load_audio(path: &Path, seconds: f64) -> Option<Vec<f32>> {
    let ff = ffmpeg_path()?;
    let out = hidden(
        Command::new(&ff)
            .arg("-v")
            .arg("quiet")
            .args(["-i"])
            .arg(path)
            .args(["-t", &format!("{seconds:.2}"), "-ac", "1", "-ar", "16000", "-f", "f32le", "-"]),
    )
    .output()
    .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    let bytes = out.stdout;
    let n = bytes.len() / 4;
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let b = [bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]];
        v.push(f32::from_le_bytes(b));
    }
    Some(v)
}

/// 把音频转成 48k 单声道 f32 采样（CLAP 的输入采样率）。
///
/// 不能复用 `load_audio` 的 16k：CLAP 的 64 个 mel 滤波器上限到 14kHz，
/// 采样率降一半，高频整段消失，海浪/鸟鸣这类声音就分不出来了。
pub fn load_audio_48k(path: &Path, seconds: f32) -> Option<Vec<f32>> {
    let ff = ffmpeg_path()?;
    let out = hidden(
        Command::new(&ff)
            .arg("-v")
            .arg("quiet")
            .args(["-i"])
            .arg(path)
            .args(["-t", &format!("{seconds:.2}"), "-ac", "1", "-ar", "48000", "-f", "f32le", "-"]),
    )
    .output()
    .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    let bytes = out.stdout;
    let n = bytes.len() / 4;
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let b = [bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]];
        v.push(f32::from_le_bytes(b));
    }
    Some(v)
}

/// 生成缩略图（jpeg），返回是否成功。
pub fn make_thumb(src: &Path, dest: &Path, size: u32) -> bool {
    let is_video = matches!(
        src.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()).as_deref(),
        Some("mp4") | Some("mkv") | Some("avi") | Some("mov") | Some("wmv") | Some("webm") | Some("flv") | Some("m4v")
    );

    // 图片直接用 image crate 解码缩放，不必依赖 ffmpeg（也快得多）
    if !is_video {
        if let Ok(img) = image::open(src) {
            let img = img.thumbnail(size, size);
            if img.save_with_format(dest, image::ImageFormat::Jpeg).is_ok() {
                return dest.is_file();
            }
        }
        // 遇到 image crate 不认识的格式（heic 等）再退回 ffmpeg
    }

    let Some(ff) = ffmpeg_path() else { return false };
    let mut cmd = Command::new(&ff);
    cmd.arg("-y");
    if is_video {
        // 取中间帧
        let dur = video_duration(src).unwrap_or(2.0);
        cmd.args(["-ss", &format!("{:.3}", dur / 2.0)]);
    }
    cmd.arg("-i").arg(src).args(["-frames:v", "1"]);
    let scale = format!("scale='min({s},iw)':'min({s},ih)':force_original_aspect_ratio=decrease", s = size);
    cmd.args(["-vf", &scale, "-q:v", "4"]).arg(dest);
    hidden(&mut cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success() && dest.is_file())
        .unwrap_or(false)
}

/// Generate a first-page/embedded-image thumbnail for document cards.
/// PDF extraction already decodes common raster streams, while DOCX stores
/// previewable images under `word/media`. Unsupported documents simply fall
/// back to the file-type icon in the gallery.
pub fn make_document_thumb(src: &Path, dest: &Path, size: u32) -> bool {
    let ext = src
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let image = if ext == "pdf" {
        crate::textract::pdf_images(src, 1).into_iter().next()
    } else if ext == "docx" {
        let Some(file) = std::fs::File::open(src).ok() else { return false };
        let Ok(mut archive) = zip::ZipArchive::new(file) else { return false };
        let mut names: Vec<String> = archive
            .file_names()
            .filter(|name| name.starts_with("word/media/"))
            .map(str::to_owned)
            .collect();
        names.sort();
        let mut image = None;
        for name in names {
            let Ok(mut entry) = archive.by_name(&name) else { continue };
            let mut bytes = Vec::new();
            if std::io::Read::read_to_end(&mut entry, &mut bytes).is_err() {
                continue;
            }
            if let Ok(value) = image::load_from_memory(&bytes) {
                image = Some(value);
                break;
            }
        }
        image
    } else {
        None
    };
    let Some(image) = image else { return false };
    let image = image.thumbnail(size, size);
    image.save_with_format(dest, image::ImageFormat::Jpeg).is_ok() && dest.is_file()
}

/// 视频基本信息（宽高）。
pub fn video_size(path: &Path) -> Option<(u32, u32)> {
    let ff = ffmpeg_path()?;
    let out = hidden(Command::new(&ff).arg("-i").arg(path).args(["-f", "null", "-"]))
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    // Video: h264, yuv420p, 320x240
    for line in text.lines() {
        if let Some(pos) = line.find("Video:") {
            let rest = &line[pos..];
            if let Some(x) = rest.find('x') {
                let before = &rest[..x];
                let after = &rest[x + 1..];
                let w: String = before.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<String>().chars().rev().collect();
                let h: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) {
                    return Some((w, h));
                }
            }
        }
    }
    None
}