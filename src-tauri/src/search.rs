//! 混合检索：结构化过滤 + 名称/路径/拼音/模糊 + 视觉标签 + 全文 + 语义 + 跨模态。
use crate::config::SearchConfig;
use crate::db::{Db, FileRow};
use crate::py::pinyin_match;
use crate::util::{build_fts_expr, format_ts, human_size, kind_label, now_ts};
use crate::vector::{normalize, VectorStore};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchQuery {
    pub text: String,
    pub kinds: Vec<String>,
    pub exts: Vec<String>,
    pub roots: Vec<String>,
    pub categories: Vec<String>,
    pub size_min: Option<i64>,
    pub size_max: Option<i64>,
    pub mtime_after: Option<i64>,
    pub mtime_before: Option<i64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// relevance | name | size | mtime | kind
    pub sort: Option<String>,
    pub desc: bool,
    pub use_fts: Option<bool>,
    pub use_pinyin: Option<bool>,
    pub use_fuzzy: Option<bool>,
    pub use_semantic: Option<bool>,
    /// 「用一句话描述找图片/声音」：查询词额外编码进跨模态空间。
    pub cross_modal: Option<bool>,
    /// 纯向量检索（相似检索）时由调用方填入。
    #[serde(skip)]
    pub query_vector: Option<Vec<f32>>,
    /// 查询向量属于哪个空间："text"（默认，llama/hash 文本空间）或 "media"。
    #[serde(skip)]
    pub vector_space: Option<String>,
    /// 文字 → 图片空间（CLIP）。
    #[serde(skip)]
    pub clip_query: Option<Vec<f32>>,
    /// 文字 → 声音空间（CLAP）。
    #[serde(skip)]
    pub audio_query: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub id: i64,
    pub path: String,
    pub dir: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub kind_label: String,
    pub size: i64,
    pub size_label: String,
    pub mtime: i64,
    pub mtime_label: String,
    pub category: Option<String>,
    pub thumb: bool,
    pub score: f32,
    pub reason: String,
    pub snippet: Option<String>,
    /// 视觉标签（图片/视频）：蓝天、草地、雪景、夕阳…
    pub tags: Option<String>,
}

impl Hit {
    fn from_row(r: &FileRow, score: f32, reason: &str) -> Self {
        Self {
            id: r.id,
            path: r.path.clone(),
            dir: r.dir.clone(),
            name: r.name.clone(),
            ext: r.ext.clone(),
            kind: r.kind.clone(),
            kind_label: kind_label(&r.kind).to_string(),
            size: r.size,
            size_label: human_size(r.size),
            mtime: r.mtime,
            mtime_label: format_ts(r.mtime),
            category: r.category.clone(),
            thumb: r.thumb_key.is_some() || r.kind == "image" || r.kind == "video",
            score,
            reason: reason.to_string(),
            snippet: None,
            tags: r.tags.clone(),
        }
    }
}

pub struct SearchEngine<'a> {
    pub db: &'a Db,
    /// 文本向量空间（llama.cpp GGUF / hash 文本）
    pub vectors: &'a VectorStore,
    /// 图片/音频/视频向量空间（hash 特征，维度独立）
    pub media_vectors: &'a VectorStore,
    /// 跨模态图片空间（CLIP 512 维）
    pub clip_vectors: &'a VectorStore,
    /// 跨模态音频空间（CLAP 512 维）
    pub audio_vectors: &'a VectorStore,
    pub cfg: &'a SearchConfig,
}

pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

fn name_score(q: &str, name: &str) -> (f32, &'static str) {
    let ql = q.to_lowercase();
    let nl = name.to_lowercase();
    if nl == ql {
        return (1.0, "名称精确匹配");
    }
    if nl.starts_with(&ql) {
        return (0.92, "名称前缀匹配");
    }
    if nl.contains(&ql) {
        let cover = ql.chars().count() as f32 / nl.chars().count().max(1) as f32;
        return (0.72 + 0.26 * cover.min(1.0), "名称包含");
    }
    // 逐词命中
    let qt = tokenize(q);
    if qt.len() > 1 {
        let hit = qt.iter().filter(|t| nl.contains(t.as_str())).count();
        if hit > 0 {
            let ratio = hit as f32 / qt.len() as f32;
            return (0.55 + 0.2 * ratio, "名称多词命中");
        }
    }
    (0.0, "")
}

fn fuzzy_score(q: &str, name: &str) -> f32 {
    if q.chars().count() < 3 {
        return 0.0;
    }
    let stem = std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let sim = strsim::jaro_winkler(&q.to_lowercase(), &stem);
    if sim >= 0.80 {
        0.35f32 + 0.35 * ((sim - 0.80) as f32 / 0.20)
    } else {
        0.0f32
    }
}

impl<'a> SearchEngine<'a> {
    pub fn search(&self, q: &SearchQuery) -> Vec<Hit> {
        let text = q.text.trim().to_string();
        let limit = q.limit.unwrap_or(self.cfg.page_size).max(1);
        let offset = q.offset.unwrap_or(0);
        let low_text = text.chars().count() < 2;
        // 相似检索：只有向量、没有文本
        let pure_vector = q.query_vector.is_some() && low_text;
        let rows = self.candidate_rows(q, if low_text { 20_000 } else { 50_000 });
        let mut scored: HashMap<i64, (FileRow, f32, String)> = HashMap::new();

        if !low_text {
            let use_pinyin = q.use_pinyin.unwrap_or(self.cfg.use_pinyin);
            let use_fuzzy = q.use_fuzzy.unwrap_or(self.cfg.use_fuzzy);
            for r in &rows {
                let (mut score, mut reason) = name_score(&text, &r.name);
                if score == 0.0 {
                    let pl = norm_path_lower(&r.path);
                    if pl.contains(&text.to_lowercase()) {
                        let cover = text.chars().count() as f32 / pl.chars().count().max(1) as f32;
                        score = 0.50 + 0.15 * cover.min(1.0);
                        reason = "路径匹配";
                    }
                }
                if score == 0.0 && use_pinyin {
                    let ps = pinyin_match(&text, &r.name, &r.path);
                    if ps > 0.0 {
                        score = ps;
                        reason = "拼音匹配";
                    }
                }
                if score == 0.0 && use_fuzzy {
                    let fs = fuzzy_score(&text, &r.name);
                    if fs > 0.0 {
                        score = fs;
                        reason = "模糊匹配";
                    }
                }
                // 视觉标签：图片/视频索引时写入的中文关键词（蓝天/草地/雪景…）。
                // 命中一个标签给 0.62，命中越多越高，满分 0.82 —— 低于文件名
                // 精确匹配（1.0），高于路径匹配（0.5~0.65），符合「用户描述的是
                // 画面内容，不是文件名」的预期。
                if score < 0.62 {
                    if let Some(tags) = &r.tags {
                        let low = text.to_lowercase();
                        let parts: Vec<&str> = tags.split(',').filter(|s| !s.is_empty()).collect();
                        let hit = parts
                            .iter()
                            .filter(|t| {
                                t.to_lowercase().contains(&low) || low.contains(&t.to_lowercase())
                            })
                            .count();
                        if hit > 0 {
                            let s = 0.62 + 0.20 * (hit as f32 / parts.len().max(1) as f32).min(1.0);
                            if s > score {
                                score = s;
                                reason = "画面标签";
                            }
                        }
                    }
                }
                if score > 0.0 {
                    scored.insert(r.id, (r.clone(), score, reason.to_string()));
                }
            }
        }

        // 空查询（只按类型/大小/时间筛选，或刚打开界面）时要能「浏览」，
        // 否则列表永远为空，用户会以为索引坏了。
        if low_text && q.query_vector.is_none() {
            for r in &rows {
                scored.insert(r.id, (r.clone(), 0.5, "全部".to_string()));
            }
        }

        // 全文
        let use_fts = q.use_fts.unwrap_or(self.cfg.use_fulltext);
        let mut fts_ids: HashSet<i64> = HashSet::new();
        if use_fts && !low_text {
            if let Some(expr) = build_fts_expr(&text) {
                if let Ok(hits) = self.db.search_fts(&expr, 500) {
                    // bm25() 返回的是负数，越负越相关（ORDER BY 升序 = 最相关在前）。
                    // 所以取反后再归一化，否则最相关的那条 rel 会是 0、其余为负，
                    // 全部被阈值滤掉 —— 表现为「正文里明明有这个词却搜不到」。
                    if !hits.is_empty() {
                        let best = hits.iter().map(|(_, s)| -*s).fold(f64::MIN, f64::max);
                        let worst = hits.iter().map(|(_, s)| -*s).fold(f64::MAX, f64::min);
                        let span = (best - worst).max(1e-6);
                        // 带结构化筛选（类型/扩展名/目录/大小/时间）时，全文命中
                        // 也必须落在候选集内，否则用户筛了「图片」却冒出 PDF。
                        let filtered = has_filters(q);
                        let allowed: HashSet<i64> = if filtered {
                            rows.iter().map(|r| r.id).collect()
                        } else {
                            HashSet::new()
                        };
                        for (fid, s) in hits {
                            if filtered && !allowed.contains(&fid) {
                                continue;
                            }
                            let rel = ((-s - worst) / span) as f32; // 0..1，越大越相关
                            let add = 0.9 * (0.35 + 0.65 * rel); // 命中即得 0.315 底分，最强 0.9
                            fts_ids.insert(fid);
                            if let Some((_row, score, reason)) = scored.get_mut(&fid) {
                                *score += add;
                                if reason != "名称精确匹配" && reason != "名称前缀匹配" {
                                    *reason = "全文内容匹配".to_string();
                                }
                            } else if let Ok(Some(r)) = self.db.get_by_id(fid) {
                                // 全文表里的孤儿行（文件已删）不该冒出来；
                                // candidate_rows 已经过滤 missing，这里补一道。
                                if r.path.is_empty() {
                                    continue;
                                }
                                scored.insert(fid, (r, add, "全文内容匹配".to_string()));
                            }
                        }
                    }
                }
            }
        }

        // 语义
        let use_semantic = q.use_semantic.unwrap_or(self.cfg.use_semantic);
        if let Some(qv) = &q.query_vector {
            if use_semantic || pure_vector {
                let mut qvec = qv.clone();
                normalize(&mut qvec);
                // 不能用 scored.keys()（已命中的），否则语义就退化成「重排」：
                // 查询词与正文完全不重叠时（纯语义提问）scored 为空，
                // allowed 为空集，语义永远召不回任何文件。
                // 这里用「结构化过滤后的候选集」，既尊重类型/路径/大小筛选，又能召回。
                let allowed: HashSet<i64> = rows.iter().map(|r| r.id).collect();
                let allowed_ref = if pure_vector { None } else { Some(&allowed) };
                // 文本查询查文本空间，媒体相似检索查媒体空间；
                // 两个空间的维度不同，绝不能混查。
                let store = if q.vector_space.as_deref() == Some("media") {
                    self.media_vectors
                } else {
                    self.vectors
                };
                let hits = store.search(&qvec, 800, allowed_ref);
                let floor = self.cfg.semantic_floor;
                for (fid, sc) in hits {
                    let rel = ((sc - floor) / (1.0 - floor)).clamp(0.0, 1.0);
                    if rel <= 0.0 {
                        continue;
                    }
                    let add = self.cfg.semantic_weight * 1.6 * rel;
                    if let Some((_, score, reason)) = scored.get_mut(&fid) {
                        *score += add;
                        // 原因只做「升级」不做降级：名称/全文这种确定性命中
                        // 比语义更有解释力，不能被后执行的语义分支覆盖成「语义相似」。
                        if reason == "全部" {
                            *reason = "语义相似".to_string();
                        }
                    } else if let Ok(Some(r)) = self.db.get_by_id(fid) {
                        scored.insert(fid, (r, add, "语义相似".to_string()));
                    }
                }
            }
        }

        // 跨模态：文字 → 图片/声音空间。
        //
        // 这是「用一句话描述找图片/音频/视频」的实现：CLIP/CLAP 把图片、
        // 音频和文字编码到同一个 512 维空间，余弦相似度就是「这段话有多像
        // 这张图/这段声音」。和上面语义段的关键区别是跨空间 —— 上面比的是
        // 文本与文本，这里比的是文本与像素/波形。
        if !low_text {
            self.cross_modal_segment(q, &rows, &mut scored);
        }

        // 过滤阈值 + 排序
        let mut list: Vec<(FileRow, f32, String)> = scored
            .into_values()
            .filter(|(_, s, _)| pure_vector || *s >= 0.16)
            .collect();
        let sort = q.sort.clone().unwrap_or_else(|| "relevance".to_string());
        match sort.as_str() {
            "name" => list.sort_by(|a, b| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase())),
            "size" => list.sort_by(|a, b| a.0.size.cmp(&b.0.size)),
            "mtime" => list.sort_by(|a, b| a.0.mtime.cmp(&b.0.mtime)),
            "kind" => list.sort_by(|a, b| a.0.kind.cmp(&b.0.kind)),
            _ => list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)),
        }
        if q.desc && sort != "relevance" {
            list.reverse();
        }
        list.into_iter()
            .skip(offset)
            .take(limit)
            .map(|(r, s, reason)| {
                let mut hit = Hit::from_row(&r, s, &reason);
                // 正文命中要能看见上下文，否则用户不知道「为什么这条算命中」。
                if fts_ids.contains(&r.id) {
                    if let Ok(Some(raw)) = self.db.get_doc_raw(r.id) {
                        hit.snippet = make_snippet(&raw, &text, 60);
                    }
                }
                hit
            })
            .collect()
    }

    /// 跨模态检索段：把文字查询投到图片/声音空间里找最近邻。
    ///
    /// 与「语义相似」的区别在于跨的是模态，不是同一种文本的语义。
    fn cross_modal_segment(
        &self,
        q: &SearchQuery,
        rows: &[FileRow],
        scored: &mut HashMap<i64, (FileRow, f32, String)>,
    ) {
        let filtered = has_filters(q);
        let allowed: HashSet<i64> = if filtered {
            rows.iter().map(|r| r.id).collect()
        } else {
            HashSet::new()
        };
        let by_id: HashMap<i64, &FileRow> = rows.iter().map(|r| (r.id, r)).collect();

        // 先各查一遍，拿两个空间的最高余弦做仲裁。
        //
        // 两个模型对「不相干的描述」都会给个不低的余弦（CLIP 尤其宽容），
        // 所以只靠绝对阈值永远会互相串味：「钢琴」会带出图片，「一只黑色的
        // 狗」会带出音频。这两个余弦不在同一量纲上（CLIP 0.3 就算沾边，
        // CLAP 0.3 已经很准），所以要先各自换算成最终分数再比。
        const CLIP_P: CrossParams<'static> = CrossParams {
            // CLIP 的余弦天生偏高：随机配对的图片/文字也在 0.2~0.3，
            // 所以 0.25 才是噪声线，0.45 已经是很强的匹配。
            floor: 0.25,
            span: 0.20,
            base: 0.55,
            gain: 0.35,
            reason: "文字描述匹配画面",
        };
        const AUDIO_P: CrossParams<'static> = CrossParams {
            // CLAP 的余弦绝对值低得多：真命中也只有 0.19~0.46，
            // 0.16 以下基本是「什么都沾一点」的幻觉。
            floor: 0.16,
            span: 0.30,
            base: 0.52,
            gain: 0.36,
            reason: "文字描述匹配声音",
        };
        let clip_hits = q.clip_query.as_ref().map(|qv| {
            let mut v = qv.clone();
            normalize(&mut v);
            // 媒体空间里「视频」的向量也是从画面来的，所以可以一起查。
            self.clip_vectors.search(&v, 400, None)
        });
        let audio_hits = q.audio_query.as_ref().map(|qv| {
            let mut v = qv.clone();
            normalize(&mut v);
            self.audio_vectors.search(&v, 400, None)
        });
        // 「突出度」= top1 减去中位数。两个模型对无关内容的基线差很多
        // （CLIP 随便配都 0.30，CLAP 只有 0.13），比绝对值等于比基线，
        // 比重心之上的抬升才是「这句话真在说这个东西」。
        let peak_of = |h: &Option<Vec<(i64, f32)>>, p: &CrossParams| {
            let Some(v) = h.as_ref().filter(|v| !v.is_empty()) else { return f32::MIN };
            let mut s: Vec<f32> = v.iter().map(|(_, x)| *x).collect();
            s.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            if s[0] < p.floor {
                return f32::MIN;
            }
            s[0] - s[s.len() / 2]
        };
        let (clip_peak, audio_peak) = (peak_of(&clip_hits, &CLIP_P), peak_of(&audio_hits, &AUDIO_P));
        // 抬升不到 0.05 就说明第一名没从背景里跳出来，这个空间对这句话
        // 无话可说——实测「钢琴」的 CLIP 抬升只有 0.02，「一只黑色的狗」
        // 的 CLAP 抬升只有 0.018。
        const MIN_PEAK: f32 = 0.05;
        // 查询听起来就是在找声音（「钢琴」「鸟叫」「下雨的声音」）时，
        // CLAP 说了算，CLIP 一律让路，否则「钢琴」会带出几张图片。
        // 反过来，纯画面描述里 CLAP 只是瞎猜，要突出得多才准它插话——
        // 「峡湾和湖泊」的 CLAP 抬升也有 0.158，但 CLIP 更明确。
        let sound_intent = crate::ai::clap::sound_intent(q.text.trim());
        let use_audio = audio_peak >= MIN_PEAK && (sound_intent || audio_peak >= clip_peak * 2.0);
        let use_clip = clip_peak >= MIN_PEAK && !(sound_intent && use_audio);

        // 图片：clip_query 由调用方事先算好（编文本向量要跑一次模型）。
        if let Some(hits) = clip_hits.filter(|_| use_clip) {
            score_cross_space(
                &hits,
                filtered,
                &allowed,
                &by_id,
                self.db,
                scored,
                CLIP_P,
            );
        }

        // 声音：CLAP 文本塔（中文查询已由 state/cli 映射成英文提示）。
        if let Some(hits) = audio_hits.filter(|_| use_audio) {
            score_cross_space(
                &hits,
                filtered,
                &allowed,
                &by_id,
                self.db,
                scored,
                AUDIO_P,
            );
        }
    }

    /// 结构化候选集
    fn candidate_rows(&self, q: &SearchQuery, limit: usize) -> Vec<FileRow> {
        let mut sql = String::from(
            "SELECT id,path,dir,name,ext,kind,size,mtime,ctime,text_len,text_state,embed_state,thumb_key,category,tags \
             FROM files WHERE missing=0",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if !q.kinds.is_empty() {
            let ph = vec!["?"; q.kinds.len()].join(",");
            sql.push_str(&format!(" AND kind IN ({ph})"));
            for k in &q.kinds {
                args.push(k.clone().into());
            }
        }
        if !q.exts.is_empty() {
            let ph = vec!["?"; q.exts.len()].join(",");
            sql.push_str(&format!(" AND ext IN ({ph})"));
            for e in &q.exts {
                args.push(e.to_lowercase().trim_start_matches('.').to_string().into());
            }
        }
        if !q.roots.is_empty() {
            sql.push_str(" AND (");
            for (i, r) in q.roots.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str("path LIKE ?");
                args.push(format!("{}%", r.trim_end_matches('\\')).into());
            }
            sql.push(')');
        }
        if !q.categories.is_empty() {
            let ph = vec!["?"; q.categories.len()].join(",");
            sql.push_str(&format!(" AND category IN ({ph})"));
            for c in &q.categories {
                args.push(c.clone().into());
            }
        }
        if let Some(v) = q.size_min {
            sql.push_str(" AND size >= ?");
            args.push(v.into());
        }
        if let Some(v) = q.size_max {
            sql.push_str(" AND size <= ?");
            args.push(v.into());
        }
        if let Some(v) = q.mtime_after {
            sql.push_str(" AND mtime >= ?");
            args.push(v.into());
        }
        if let Some(v) = q.mtime_before {
            sql.push_str(" AND mtime <= ?");
            args.push(v.into());
        }
        sql.push_str(&format!(" LIMIT {limit}"));
        self.db.query_rows(&sql, args)
    }

    pub fn facets(&self, q: &SearchQuery) -> HashMap<String, Vec<(String, i64)>> {
        let rows = self.candidate_rows(q, 100_000);
        let mut by_kind: HashMap<String, i64> = HashMap::new();
        let mut by_ext: HashMap<String, i64> = HashMap::new();
        let mut by_cat: HashMap<String, i64> = HashMap::new();
        for r in rows {
            *by_kind.entry(r.kind.clone()).or_insert(0) += 1;
            if !r.ext.is_empty() {
                *by_ext.entry(r.ext.clone()).or_insert(0) += 1;
            }
            if let Some(c) = r.category {
                *by_cat.entry(c).or_insert(0) += 1;
            }
        }
        let mut out = HashMap::new();
        out.insert("kinds".to_string(), sort_desc(by_kind));
        out.insert(
            "exts".to_string(),
            sort_desc(by_ext).into_iter().take(40).collect(),
        );
        out.insert("categories".to_string(), sort_desc(by_cat));
        out
    }
}

/// 是否带了结构化筛选条件（决定全文命中是否需要与候选集求交）。
fn has_filters(q: &SearchQuery) -> bool {
    !q.kinds.is_empty()
        || !q.exts.is_empty()
        || !q.roots.is_empty()
        || !q.categories.is_empty()
        || q.size_min.is_some()
        || q.size_max.is_some()
        || q.mtime_after.is_some()
        || q.mtime_before.is_some()
}

/// 从正文里截一段包含查询词的上下文，两侧各留 `pad` 个字符。
fn make_snippet(raw: &str, query: &str, pad: usize) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_lowercase();
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    // 查询词可能被分词过，先用整串找，找不到再退回第一个 2-gram
    let pos = lower.find(&q).or_else(|| {
        let terms = crate::util::fts_query_terms(&q);
        terms.iter().filter_map(|t| lower.find(t)).min()
    });
    let Some(byte_pos) = pos else {
        let head: String = raw.chars().take(pad * 2).collect();
        return Some(clean_ws(&head));
    };
    let char_pos = lower[..byte_pos].chars().count();
    let start = char_pos.saturating_sub(pad);
    let end = (char_pos + q.chars().count() + pad).min(raw.chars().count());
    let body: String = raw
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    let mut s = clean_ws(&body);
    if start > 0 {
        s = format!("…{s}");
    }
    if end < raw.chars().count() {
        s.push('…');
    }
    Some(s)
}

fn clean_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sort_desc(m: HashMap<String, i64>) -> Vec<(String, i64)> {
    let mut v: Vec<(String, i64)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

fn norm_path_lower(p: &str) -> String {
    p.to_lowercase().replace('/', "\\")
}

/// 跨模态打分的参数：两个模型的分数量纲差得很远，各给各的。
struct CrossParams<'a> {
    /// 低于这个余弦直接当噪声丢掉
    floor: f32,
    /// 从 floor 涨到「满分」需要多少余弦
    span: f32,
    /// 刚好卡在 floor 上的命中能拿到的分数
    base: f32,
    /// 余弦打满时再加多少
    gain: f32,
    reason: &'a str,
}

/// 把跨模态近邻并进总分表。
///
/// 统一成「只加分不降分」：名称精确/前缀命中仍然是更强的信号，不能被
/// 一句描述覆盖掉用户的明确意图；反过来，跨模态命中也应该盖过路径匹配。
fn score_cross_space(
    hits: &[(i64, f32)],
    filtered: bool,
    allowed: &HashSet<i64>,
    by_id: &HashMap<i64, &FileRow>,
    db: &Db,
    scored: &mut HashMap<i64, (FileRow, f32, String)>,
    p: CrossParams,
) {
    for &(fid, sc) in hits {
        if filtered && !allowed.contains(&fid) {
            continue;
        }
        if sc < p.floor {
            continue;
        }
        let rel = ((sc - p.floor) / p.span).clamp(0.0, 1.0);
        let add = p.base + p.gain * rel;
        if let Some((_, score, reason)) = scored.get_mut(&fid) {
            if *score < add {
                *score = add;
                if reason != "名称精确匹配" && reason != "名称前缀匹配" {
                    *reason = p.reason.to_string();
                }
            }
        } else if let Some(r) = by_id.get(&fid) {
            scored.insert(fid, ((*r).clone(), add, p.reason.to_string()));
        } else if let Ok(Some(r)) = db.get_by_id(fid) {
            scored.insert(fid, (r, add, p.reason.to_string()));
        }
    }
}

/// 相似检索：用给定向量找最相近的文件（排除自身）。
///
/// 空间选择很关键：图片/视频优先用跨模态空间（CLIP 看的是「画面内容像不像」），
/// 音频优先用 CLAP（「听起来像不像」）；模型没装时才退回 hash 特征空间
/// （那比的是颜色分布/能量包络，粗但能用）。
pub fn similar(
    db: &Db,
    vectors: &VectorStore,
    media_vectors: &VectorStore,
    clip_vectors: &VectorStore,
    audio_vectors: &VectorStore,
    file_id: i64,
    kind_filter: Option<&str>,
    top: usize,
) -> Vec<Hit> {
    let src_kind = db.get_by_id(file_id).ok().flatten().map(|r| r.kind.clone());
    let spaces: [&VectorStore; 4] = [clip_vectors, audio_vectors, vectors, media_vectors];
    // 先按源文件类型挑首选空间，找不到再按顺序兜底 ——
    // 用户刚装好 CLIP 时，老文件只有 hash 向量，不能直接报「没有相似项」。
    let order: Vec<&VectorStore> = match src_kind.as_deref() {
        Some("image") | Some("video") => vec![clip_vectors, vectors, media_vectors],
        Some("audio") => vec![audio_vectors, media_vectors],
        _ => vec![vectors, media_vectors],
    };
    let mut store = None;
    let mut raw = Vec::new();
    for s in order.iter().chain(spaces.iter()) {
        if let Some(v) = s.get(file_id) {
            store = Some(*s);
            raw = v;
            break;
        }
    }
    let Some(store) = store else {
        return vec![];
    };
    let mut qvec = raw;
    normalize(&mut qvec);
    // 媒体空间里图片/音频/视频共用 256 维，但特征的物理含义完全不同
    // （颜色直方图 / 频谱 / 帧均值），跨类型比出来的分数毫无意义。
    // 所以默认只在「同一种类型」里找相似，显式传 kind_filter 时才覆盖。
    let want_kind = match kind_filter {
        Some(k) if k != "auto" && !k.is_empty() => Some(k.to_string()),
        _ => src_kind.clone(),
    };
    let hits = store.search(&qvec, top * 8 + 16, None);
    let mut out = Vec::new();
    for (fid, sc) in hits {
        if fid == file_id {
            continue;
        }
        if let Ok(Some(r)) = db.get_by_id(fid) {
            if r.path.is_empty() {
                continue;
            }
            if let Some(ref k) = want_kind {
                if &r.kind != k {
                    continue;
                }
            }
            out.push(Hit::from_row(&r, sc, "向量相似"));
            if out.len() >= top {
                break;
            }
        }
    }
    let _ = now_ts();
    out
}
