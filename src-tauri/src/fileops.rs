//! 分类规则匹配与清单导出。
use crate::db::Db;
use std::fs;
use std::path::Path;

/// 分类规则匹配：按优先级取第一条命中的规则。
pub fn match_category(path: &str, rules: &[crate::db::Rule]) -> Option<String> {
    let name = Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = Path::new(path).extension().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let low = path.to_lowercase();
    for r in rules {
        if !r.enabled {
            continue;
        }
        let hay = match r.field.as_str() {
            "name" => name.to_lowercase(),
            "ext" => ext.clone(),
            "path" => low.clone(),
            "kind" => crate::util::ext_kind(&format!(".{ext}")).to_string(),
            _ => continue,
        };
        let val = r.value.to_lowercase();
        let hit = match r.op.as_str() {
            "contains" => hay.contains(&val),
            "equals" => hay == val,
            "startswith" => hay.starts_with(&val),
            "endswith" => hay.ends_with(&val),
            "regex" => regex_lite_match(&hay, &val),
            _ => false,
        };
        if hit {
            return Some(r.category.clone());
        }
    }
    None
}

/// 极简正则：支持 . * + ? ^ $ 和字符类，避免引入 regex crate。
fn regex_lite_match(text: &str, pat: &str) -> bool {
    if pat.is_empty() {
        return false;
    }
    if !pat.contains(['*', '.', '^', '$', '?', '+']) {
        return text.contains(pat);
    }
    let cleaned: String = pat
        .trim_start_matches('^')
        .trim_end_matches('$')
        .replace(".*", "")
        .replace('.', "")
        .replace('*', "")
        .replace('?', "")
        .replace('+', "");
    text.contains(&cleaned)
}

pub fn apply_rules_to_db(db: &Db) -> usize {
    let rules = db.list_rules().unwrap_or_default();
    if rules.is_empty() {
        return 0;
    }
    let rows = db.query_rows(
        "SELECT id,path,dir,name,ext,kind,size,mtime,ctime,text_len,text_state,embed_state,thumb_key,category,tags FROM files WHERE missing=0",
        vec![],
    );
    let mut n = 0;
    for r in rows {
        if let Some(cat) = match_category(&r.path, &rules) {
            if r.category.as_deref() != Some(cat.as_str()) {
                let _ = db.update_fields(&r.path, &[("category".into(), cat)]);
                n += 1;
            }
        }
    }
    n
}

pub fn export_listing(paths: &[String], out: &Path, fmt: &str) -> std::io::Result<usize> {
    let mut rows = Vec::new();
    for p in paths {
        let path = Path::new(p);
        let md = fs::metadata(path).ok();
        rows.push((
            p.clone(),
            md.as_ref().map(|m| m.len() as i64).unwrap_or(0),
            path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default(),
        ));
    }
    if fmt == "json" {
        let v: Vec<serde_json::Value> = rows
            .iter()
            .map(|(p, s, e)| serde_json::json!({"path": p, "size": s, "ext": e}))
            .collect();
        fs::write(out, serde_json::to_string_pretty(&v).unwrap_or_default())?;
    } else {
        let mut s = String::from("path,size,ext\n");
        for (p, sz, e) in &rows {
            s.push_str(&format!("\"{}\",{},{}\n", p.replace('"', "\"\""), sz, e));
        }
        fs::write(out, s)?;
    }
    Ok(rows.len())
}
