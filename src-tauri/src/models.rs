//! 模型管理：扫描本地 GGUF、读取元信息、切换 / 导入 / 删除。
use crate::config::ModelConfig;
use crate::util::{format_ts, human_size};
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct ModelEntry {
    pub path: String,
    /// 展示名：优先取 GGUF 头里的 general.name，读不到就用文件名
    pub name: String,
    pub file: String,
    pub size: i64,
    pub size_label: String,
    pub mtime: i64,
    pub mtime_label: String,
    /// embedding | chat | projector | unknown
    pub kind: String,
    pub kind_label: String,
    /// 是否是当前加载的嵌入模型
    pub active: bool,
}

/// GGUF 头部里的显示名（只读前 256KB）。
fn gguf_name(path: &Path) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let mut head = vec![0u8; 256 * 1024];
    let Ok(n) = f.read(&mut head) else { return String::new() };
    head.truncate(n);
    if head.len() < 24 || &head[0..4] != b"GGUF" {
        return String::new();
    }
    let n_kv = u64::from_le_bytes(head[16..24].try_into().unwrap_or([0; 8])) as usize;
    let mut p = 24usize;
    let mut arch = String::new();
    let mut name = String::new();
    for _ in 0..n_kv.min(64) {
        let Some((key, np)) = read_str(&head, p) else { break };
        p = np;
        let Some(t) = head.get(p..p + 4) else { break };
        let vt = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
        p += 4;
        if vt == 8 {
            let Some((val, np)) = read_str(&head, p) else { break };
            p = np;
            match key.as_str() {
                "general.architecture" => arch = val,
                "general.name" => name = val,
                _ => {}
            }
        } else {
            // 其它类型：跳过数值 / 数组
            let skipped = match vt {
                0 | 1 | 7 => 1,
                2 | 3 => 2,
                4 | 5 | 6 => 4,
                10 | 11 | 12 => 8,
                9 => {
                    let Some(t) = head.get(p..p + 4) else { break };
                    let et = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
                    p += 4;
                    let Some(c) = head.get(p..p + 8) else { break };
                    let count = u64::from_le_bytes(c.try_into().unwrap_or([0; 8])) as usize;
                    p += 8;
                    let (ew, dynamic) = match et {
                        0 | 1 | 7 => (1usize, false),
                        2 | 3 => (2, false),
                        4 | 5 | 6 => (4, false),
                        10 | 11 | 12 => (8, false),
                        _ => (0, true),
                    };
                    if dynamic {
                        break;
                    }
                    let total = ew.saturating_mul(count.min(1 << 20));
                    if p + total > head.len() {
                        break;
                    }
                    total
                }
                _ => break,
            };
            if p + skipped > head.len() {
                break;
            }
            p += skipped;
        }
    }
    if name.is_empty() { arch } else { name }
}

fn read_str(buf: &[u8], p: usize) -> Option<(String, usize)> {
    let len_bytes = buf.get(p..p + 8)?;
    let len = u64::from_le_bytes(len_bytes.try_into().ok()?) as usize;
    let s = buf.get(p + 8..p + 8 + len.min(256))?;
    Some((String::from_utf8_lossy(s).to_string(), p + 8 + len))
}

fn classify(file: &str) -> (&'static str, &'static str) {
    let f = file.to_lowercase();
    if f.contains("mmproj") || f.contains("projector") || f.contains("vision") {
        ("projector", "视觉投影")
    } else if f.contains("embed") || f.contains("bge") || f.contains("gte") || f.contains("minilm") {
        ("embed", "嵌入模型")
    } else if f.contains("chat") || f.contains("instruct") || f.contains("qwen") || f.contains("llama-3") {
        ("chat", "对话模型")
    } else {
        ("unknown", "未知")
    }
}

/// 扫描目录里的 .gguf（不递归子目录，避免扫到大盘）。
pub fn scan(cfg: &ModelConfig, extra_dirs: &[PathBuf]) -> Vec<ModelEntry> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for d in extra_dirs {
        dirs.push(d.clone());
    }
    for p in [&cfg.embed_model, &cfg.chat_model] {
        if !p.trim().is_empty() {
            if let Some(parent) = Path::new(p).parent() {
                dirs.push(parent.to_path_buf());
            }
        }
    }
    dirs.sort();
    dirs.dedup();

    let mut out: Vec<ModelEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let is_gguf = p
                .extension()
                .map(|x| x.eq_ignore_ascii_case("gguf"))
                .unwrap_or(false);
            if !is_gguf {
                continue;
            }
            let key = p.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            let md = e.metadata().ok();
            let size = md.as_ref().map(|m| m.len() as i64).unwrap_or(0);
            let mtime = md
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let file = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let (kind, kind_label) = classify(&file);
            let display = if kind == "projector" { String::new() } else { gguf_name(&p) };
            let path_s = p.to_string_lossy().replace('/', "\\");
            let active = !cfg.embed_model.trim().is_empty()
                && norm(&cfg.embed_model) == norm(&path_s);
            out.push(ModelEntry {
                name: if display.is_empty() { file.clone() } else { display },
                path: path_s,
                file,
                size,
                size_label: human_size(size),
                mtime,
                mtime_label: format_ts(mtime),
                kind: kind.to_string(),
                kind_label: kind_label.to_string(),
                active,
            });
        }
    }
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

fn norm(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

/// 删除模型文件。正在使用的模型不允许删，避免程序下次启动找不到文件。
pub fn remove(path: &str, cfg: &ModelConfig) -> Result<(), String> {
    let p = Path::new(path);
    if !p.is_file() {
        return Err(format!("文件不存在：{path}"));
    }
    if norm(&cfg.embed_model) == norm(path) {
        return Err("这是当前正在使用的嵌入模型，请先切换到其它模型再删除".into());
    }
    if norm(&cfg.chat_model) == norm(path) {
        return Err("这是当前配置的对话模型，请先在设置里清空后再删除".into());
    }
    let ext_ok = p
        .extension()
        .map(|x| x.eq_ignore_ascii_case("gguf"))
        .unwrap_or(false);
    if !ext_ok {
        return Err("只允许删除 .gguf 模型文件".into());
    }
    std::fs::remove_file(p).map_err(|e| format!("删除失败：{e}"))?;
    Ok(())
}

/// 把外部 GGUF 复制进模型目录（重名自动加序号）。
pub fn import(src: &str, dir: &Path) -> Result<String, String> {
    let s = Path::new(src);
    if !s.is_file() {
        return Err(format!("文件不存在：{src}"));
    }
    let is_gguf = s
        .extension()
        .map(|x| x.eq_ignore_ascii_case("gguf"))
        .unwrap_or(false);
    if !is_gguf {
        return Err("只能导入 .gguf 模型文件".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建模型目录失败：{e}"))?;
    let file = s.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
    let dest = crate::util::unique_path(&dir.join(file));
    std::fs::copy(s, &dest).map_err(|e| format!("复制失败：{e}"))?;
    Ok(dest.to_string_lossy().replace('/', "\\"))
}

/// 模型目录：exe 同级 models/，不存在就建。
pub fn model_dir(app_dir: &Path, data_dir: &Path) -> PathBuf {
    let a = app_dir.join("models");
    if a.exists() {
        return a;
    }
    let d = data_dir.join("models");
    if d.exists() {
        return d;
    }
    a
}
