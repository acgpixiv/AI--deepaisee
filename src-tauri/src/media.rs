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
