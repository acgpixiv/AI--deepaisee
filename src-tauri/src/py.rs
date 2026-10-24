//! 拼音检索：全拼 / 首字母匹配打分。
use pinyin::ToPinyin;

pub fn full_pinyin(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if let Some(py) = c.to_pinyin() {
            out.push_str(py.plain());
        } else if c.is_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(' ');
        }
    }
    out
}

pub fn initials(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if let Some(py) = c.to_pinyin() {
            if let Some(f) = py.first_letter().chars().next() {
                out.push(f.to_ascii_lowercase());
            }
        } else if c.is_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

fn is_ascii_word(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.')
}

/// 拼音匹配：返回 0.0 表示不匹配，0.70~0.97 表示匹配强度。
pub fn pinyin_match(query: &str, name: &str, path: &str) -> f32 {
    let q = query.trim().to_lowercase();
    if q.is_empty() || !is_ascii_word(&q) {
        return 0.0;
    }
    // 只在查询是纯英文/数字时启用拼音匹配
    let has_cjk = name.chars().any(|c| (c as u32) >= 0x4E00);
    if !has_cjk {
        return 0.0;
    }
    let full = full_pinyin(name);
    let ini = initials(name);
    let mut best: f32 = 0.0;
    if full.contains(&q) {
        let ratio = q.len() as f32 / full.len().max(1) as f32;
        best = best.max(0.86 + 0.11 * ratio.min(1.0));
    }
    if ini.contains(&q) {
        let ratio = q.len() as f32 / ini.len().max(1) as f32;
        best = best.max(0.70 + 0.20 * ratio.min(1.0));
    }
    if best == 0.0 {
        // 路径中的中文也参与
        let pfull = full_pinyin(path);
        if pfull.contains(&q) {
            best = 0.62;
        }
    }
    best
}
