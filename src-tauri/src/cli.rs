//! 命令行接口：`DeepDark_Oracle.exe --cli <子命令>`
//!
//! 打包版是 windowed 程序，没有控制台。为了让它能当 CLI 用，这里在写输出之后
//! 用 `AttachConsole(ATTACH_PARENT_PROCESS)` 挂到调用者的控制台上——这样用户在
//! cmd/PowerShell 里跑就能直接看到结果，双击运行则什么都不发生。
//!
//! 子命令：
//!   scan <目录>...            扫描并入库
//!   search <词> [--kind K]    混合检索（文件名/路径/拼音/全文/语义）
//!   sem <一句话>              纯语义检索
//!   similar <文件路径>        找相似文件（同类型）
//!   stats                     库统计
//!   index [--what auto|text|embed] [--wait]
//!   rules                     列分类规则
//!   serve [--port N]          启动局域网服务

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::config::{config_path, data_dir, AppConfig};
use crate::db::Db;
use crate::scan::Scanner;
use crate::search::{SearchEngine, SearchQuery};
use crate::vector::VectorStore;

/// 把 stdout 接到父进程的控制台上（windowed 程序默认没有控制台）。
///
/// 关键：**先检查当前 stdout 是否已经被调用者重定向**（PowerShell `|` / `>`、
/// 批处理的管道重定向都会给进程一个有效的 stdout 句柄）。已经有句柄时绝不能
/// 再 `AttachConsole` + `SetStdHandle`，否则输出会从管道「跳」到控制台缓冲区，
/// 调用方一个字也收不到。
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(dwProcessId: u32) -> i32;
            fn GetStdHandle(nStdHandle: u32) -> *mut core::ffi::c_void;
            fn SetStdHandle(nStdHandle: u32, h: *mut core::ffi::c_void) -> i32;
            fn CreateFileW(
                name: *const u16,
                access: u32,
                share: u32,
                sec: *mut core::ffi::c_void,
                disp: u32,
                flags: u32,
                tmpl: *mut core::ffi::c_void,
            ) -> *mut core::ffi::c_void;
        }
        const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
        const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4;
        const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;

        let existing = GetStdHandle(STD_OUTPUT_HANDLE);
        // INVALID_HANDLE_VALUE(-1) 与 NULL(0) 都表示「没有可用的 stdout」
        if !existing.is_null() && existing as isize != -1 {
            return; // 已被重定向到管道/文件，保持不动
        }

        // 失败（例如双击运行）就静默继续，不能让它变成 panic
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
            let h = CreateFileW(
                name.as_ptr(),
                0x4000_0000 | 0x8000_0000, // GENERIC_WRITE | GENERIC_READ
                3,                         // FILE_SHARE_READ | FILE_SHARE_WRITE
                std::ptr::null_mut(),
                3, // OPEN_EXISTING
                0,
                std::ptr::null_mut(),
            );
            if h as isize != -1 {
                SetStdHandle(STD_OUTPUT_HANDLE, h);
                SetStdHandle(STD_ERROR_HANDLE, h);
            }
        }
    }
}

struct Ctx {
    app_dir: PathBuf,
    data_dir: PathBuf,
    cfg: AppConfig,
    db: Arc<Db>,
    vectors: Arc<VectorStore>,
    media: Arc<VectorStore>,
    clip: Arc<VectorStore>,
    audio: Arc<VectorStore>,
    clip_model: Option<Arc<crate::ai::clip::ClipModel>>,
    clap_model: Option<Arc<crate::ai::clap::ClapModel>>,
}

impl Ctx {
    fn open() -> Result<Self, String> {
        let app_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|x| x.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let data_dir = data_dir(Some(&app_dir));
        let mut cfg = AppConfig::load(&config_path(Some(&app_dir)));
        // 与主程序一致：没配置模型时在 models\ 下自动探测。
        if cfg.model.embed_model.trim().is_empty() {
            if let Some(found) =
                crate::models::autodetect_embed_model(&cfg.model, &app_dir, &data_dir)
            {
                cfg.model.embed_model = found;
            }
        }
        let db = Db::open(&data_dir.join("index.db")).map_err(|e| format!("打开数据库失败：{e}"))?;
        db.sync_index_roots(&cfg.scan.roots, &cfg.scan.disabled_roots)
            .map_err(|e| format!("同步索引目录失败：{e}"))?;
        // 维度必须和真正要用的 embedder 一致，否则向量库会按错误步长解析
        let dim = crate::ai::build_embedder_with_batch(&cfg.model, cfg.index.batch_size)
            .map(|e| e.dim())
            .unwrap_or(512);
        let vectors = VectorStore::open(&data_dir.join("vectors"), dim)
            .map_err(|e| format!("打开向量库失败：{e}"))?;
        let media = VectorStore::open(&data_dir.join("vectors_media"), 256)
            .map_err(|e| format!("打开媒体向量库失败：{e}"))?;
        let clip = VectorStore::open(&data_dir.join("vectors_clip"), 512)
            .map_err(|e| format!("打开跨模态图片向量库失败：{e}"))?;
        let audio = VectorStore::open(&data_dir.join("vectors_audio"), 512)
            .map_err(|e| format!("打开跨模态音频向量库失败：{e}"))?;
        // 命令行跑跨模态：模型缺失就静默关闭，不阻塞其它子命令。
        let threads = if cfg.model.n_threads > 0 { cfg.model.n_threads } else { 0 };
        let clips = crate::ai::clip::resolve_dir(&cfg.model, &app_dir, &data_dir);
        let clip_model = clips.and_then(|d| crate::ai::clip::ClipModel::load(&d, threads)).map(Arc::new);
        let claps = crate::ai::clap::resolve_dir(&cfg.model, &app_dir, &data_dir);
        let clap_model = claps.and_then(|d| crate::ai::clap::ClapModel::load(&d, threads)).map(Arc::new);
        Ok(Self {
            app_dir,
            data_dir,
            cfg,
            db: Arc::new(db),
            vectors: Arc::new(vectors),
            media: Arc::new(media),
            clip: Arc::new(clip),
            audio: Arc::new(audio),
            clip_model,
            clap_model,
        })
    }

    fn engine(&self) -> SearchEngine<'_> {
        SearchEngine {
            db: &self.db,
            vectors: &self.vectors,
            media_vectors: &self.media,
            clip_vectors: &self.clip,
            audio_vectors: &self.audio,
            cfg: &self.cfg.search,
        }
    }

    fn apply_root_scope(&self, q: &mut SearchQuery) {
        crate::search::apply_enabled_root_scope(
            q,
            &self.cfg.scan.roots,
            &self.cfg.scan.disabled_roots,
        );
    }

    fn query_vec(&self, text: &str) -> Option<Vec<f32>> {
        crate::ai::build_embedder_with_batch(&self.cfg.model, self.cfg.index.batch_size)
            .ok()
            .and_then(|e| e.embed_text(text, true).ok())
    }

}

/// 打印一条命中：`分数  类型  大小  名称`
fn print_hits(hits: &[crate::search::Hit], show_snippet: bool) {
    if hits.is_empty() {
        println!("（没有结果）");
        return;
    }
    for h in hits {
        println!(
            "{:>5.2}  {:<4}  {:>9}  {}",
            h.score, h.kind_label, h.size_label, h.name
        );
        println!("        {}", h.path);
        if show_snippet {
            if let Some(s) = h.snippet.as_deref() {
                let s = s.replace('\n', " ");
                let s: String = s.chars().take(120).collect();
                println!("        {s}");
            }
        }
    }
}

fn parse_flag(args: &[String], name: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == name)?;
    args.get(pos + 1).cloned()
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// 取所有「不是 flag、也不是 flag 取值」的位置参数。
fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            // 除开关型 flag 外，其余都吃掉下一个参数
            if !matches!(a.as_str(), "--no-semantic" | "--wait" | "--help" | "-h") {
                skip = true;
            }
            continue;
        }
        out.push(a.clone());
    }
    out
}

pub fn run(argv: &[String]) -> i32 {
    attach_console();
    // argv[0] 是 exe；--cli 之后的才是子命令
    let rest: Vec<String> = match argv.iter().position(|a| a == "--cli") {
        Some(i) => argv[i + 1..].to_vec(),
        None => Vec::new(),
    };
    if rest.is_empty() {
        print_usage();
        return 2;
    }
    let ctx = match Ctx::open() {
        Ok(c) => c,
        Err(e) => {
            println!("错误：{e}");
            return 1;
        }
    };
    let sub = rest[0].as_str();
    let args = &rest[1..];
    match sub {
        "scan" => cmd_scan(&ctx, args),
        "search" => cmd_search(&ctx, args),
        "sem" => cmd_sem(&ctx, args),
        "similar" => cmd_similar(&ctx, args),
        "stats" => cmd_stats(&ctx),
        "index" => cmd_index(&ctx, args),
        "rules" => cmd_rules(&ctx),
        "serve" => cmd_serve(&ctx, args),
        "-h" | "--help" | "help" => {
            print_usage();
            0
        }
        other => {
            println!("未知子命令：{other}");
            print_usage();
            2
        }
    }
}

fn print_usage() {
    println!("DeepDark_Oracle {} 命令行", crate::APP_DISPLAY_VERSION);
    println!();
    println!("用法：DeepDark_Oracle.exe --cli <子命令> [参数]");
    println!();
    println!("  scan <目录>...              扫描并入库（可多个目录）");
    println!("  search <关键词>             混合检索：文件名/路径/拼音/全文/语义");
    println!("         --kind image|video|audio|doc|text|archive");
    println!("         --limit N           默认 20");
    println!("         --no-semantic       关闭语义（更快）");
    println!("  sem <一句话描述>             纯语义检索");
    println!("  similar <文件路径>           找同类型相似文件");
    println!("  stats                       库统计");
    println!("  index [--what auto|text|embed] [--wait]");
    println!("  rules                       列分类规则");
    println!("  serve [--port N]            启动局域网服务（前台运行）");
}

fn cmd_scan(ctx: &Ctx, args: &[String]) -> i32 {
    let roots: Vec<String> = positional(args);
    if roots.is_empty() {
        println!("用法：--cli scan <目录>...");
        return 2;
    }
    let t = Instant::now();
    println!("扫描 {} 个目录 …", roots.len());
    let mut cfg = ctx.cfg.scan.clone();
    cfg.roots = roots.clone();
    let scanner = Scanner::new();
    let stats = scanner.run(&ctx.db, &cfg, |batch| {
        print!("\r  已入库 +{} 个   ", batch.len());
    });
    println!();
    println!(
        "完成：目录 {}，文件 {}，新增 {}，更新 {}，跳过 {}，错误 {}，已消失 {}，{:.1} MB，{:.1}s",
        stats.dirs,
        stats.files,
        stats.added,
        stats.updated,
        stats.skipped,
        stats.errors,
        stats.missing,
        stats.bytes as f64 / 1_048_576.0,
        t.elapsed().as_secs_f64()
    );
    // 记住这次扫的目录，界面下次打开能直接看到
    let mut cfg_out = ctx.cfg.clone();
    cfg_out.scan.roots = roots;
    let _ = cfg_out.save(&config_path(Some(&ctx.app_dir)));
    println!("下一步：--cli index --wait 建立全文与向量索引");
    0
}

fn cmd_search(ctx: &Ctx, args: &[String]) -> i32 {
    let text: String = positional(args).join(" ");
    if text.is_empty() {
        println!("用法：--cli search <关键词>");
        return 2;
    }
    let limit: usize = parse_flag(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let kinds: Vec<String> = match parse_flag(args, "--kind") {
        Some(k) => vec![k],
        None => Vec::new(),
    };
    let no_sem = has_flag(args, "--no-semantic");
    let mut q = SearchQuery {
        text: text.clone(),
        kinds,
        limit: Some(limit),
        use_semantic: Some(!no_sem),
        ..Default::default()
    };
    if !no_sem {
        q.query_vector = ctx.query_vec(&text);
    }
    // 跨模态：命令行默认开启，模型没装时字段为 None，检索段自动跳过。
    if !has_flag(args, "--no-cross") {
        if let Some(m) = &ctx.clip_model {
            q.clip_query = m.text_vec(&text).ok();
        }
        if let Some(m) = &ctx.clap_model {
            // CLAP 是英文模型：中文查询先映射成英文提示再编码。
            let p = crate::ai::clap::prompt_for(&text);
            q.audio_query = m.text_vec(&p).ok();
        }
    }
    ctx.apply_root_scope(&mut q);
    let t = Instant::now();
    let hits = ctx.engine().search(&q);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("\n搜索「{text}」：{} 条，{ms:.0} ms", hits.len());
    println!("{}", "-".repeat(78));
    print_hits(&hits, true);
    0
}

fn cmd_sem(ctx: &Ctx, args: &[String]) -> i32 {
    let text = positional(args).join(" ");
    if text.trim().is_empty() {
        println!("用法：--cli sem <一句话描述>");
        return 2;
    }
    let Some(vec) = ctx.query_vec(&text) else {
        println!("错误：模型未加载，无法做语义检索（检查 data/config.json 的 model.embed_model）");
        return 1;
    };
    let t = Instant::now();
    let mut q = SearchQuery {
        text,
        query_vector: Some(vec),
        use_semantic: Some(true),
        limit: Some(20),
        ..Default::default()
    };
    ctx.apply_root_scope(&mut q);
    let hits = ctx.engine().search(&q);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("语义检索：{} 条，{ms:.0} ms", hits.len());
    println!("{}", "-".repeat(78));
    print_hits(&hits, false);
    0
}

fn cmd_similar(ctx: &Ctx, args: &[String]) -> i32 {
    let Some(path) = positional(args).into_iter().next() else {
        println!("用法：--cli similar <文件路径>");
        return 2;
    };
    let Some(row) = ctx.db.get_by_path(&path).ok().flatten() else {
        println!("错误：这个文件不在索引里：{path}");
        println!("提示：先跑 --cli scan <所在目录>");
        return 1;
    };
    let t = Instant::now();
    let enabled_roots: Vec<String> = ctx.cfg.scan.roots.iter()
        .filter(|root| !ctx.cfg.scan.disabled_roots.iter().any(|disabled| {
            crate::util::norm_path(disabled).eq_ignore_ascii_case(&crate::util::norm_path(root))
        }))
        .cloned()
        .collect();
    let hits = crate::search::similar_scoped(
        &ctx.db,
        &ctx.vectors,
        &ctx.media,
        &ctx.clip,
        &ctx.audio,
        row.id,
        None,
        10,
        if ctx.cfg.scan.roots.is_empty() { None } else { Some(&enabled_roots) },
    );
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("与「{}」相似：{} 条，{ms:.0} ms", row.name, hits.len());
    println!("{}", "-".repeat(78));
    print_hits(&hits, false);
    0
}

fn cmd_stats(ctx: &Ctx) -> i32 {
    let total = ctx.db.count().unwrap_or(0);
    let bytes = ctx.db.total_bytes().unwrap_or(0);
    println!("DeepDark_Oracle {}", crate::APP_DISPLAY_VERSION);
    println!("数据目录：{}", ctx.data_dir.display());
    println!("文件：{total} 个，{:.2} MB", bytes as f64 / 1_048_576.0);
    println!("正文已抽取：{}", ctx.db.count_state("text", "ok").unwrap_or(0));
    println!("向量已完成：{}", ctx.db.count_state("embed", "ok").unwrap_or(0));
    println!(
        "文本向量库：{} 条 / {} 维",
        ctx.vectors.len(),
        ctx.vectors.dim()
    );
    println!(
        "媒体向量库：{} 条 / {} 维",
        ctx.media.len(),
        ctx.media.dim()
    );
    if let Ok(kinds) = ctx.db.kind_stats() {
        println!("按类型：");
        for (k, n, b) in kinds {
            println!(
                "  {:<8} {:>6} 个  {:>10.2} MB",
                crate::util::kind_label(&k),
                n,
                b as f64 / 1_048_576.0
            );
        }
    }
    0
}

fn cmd_index(ctx: &Ctx, args: &[String]) -> i32 {
    let what = parse_flag(args, "--what").unwrap_or_else(|| "auto".into());
    let Some(emb) = crate::ai::build_embedder_with_batch(&ctx.cfg.model, ctx.cfg.index.batch_size).ok() else {
        println!("错误：模型未加载");
        return 1;
    };
    let idx = crate::index::Indexer::new(
        ctx.db.clone(),
        ctx.vectors.clone(),
        ctx.media.clone(),
        ctx.clip.clone(),
        ctx.audio.clone(),
        Arc::from(emb),
        Arc::new(crate::ai::hash::HashEmbedder::new()),
        ctx.cfg.index.clone(),
        ctx.data_dir.join("thumbs"),
    );
    idx.set_cross_modal(ctx.clip_model.clone(), ctx.clap_model.clone());
    let idx = Arc::new(idx);
    let mut queued = idx.enqueue_all_pending(&what);
    // 维度变化会把向量库清空，但 embed_state 仍是 ok → 查不到待办。
    // 这里补一道自愈：库空了但有「已完成」的记录，就全部重排。
    if queued == 0 && has_flag(args, "--force") {
        let n = ctx.db.reset_states(&what).unwrap_or(0);
        queued = idx.enqueue_all_pending(&what);
        println!("强制重建：重置 {n} 条记录");
    } else if queued == 0 && what != "text" && ctx.vectors.len() == 0 {
        let n = ctx.db.reset_states("embed").unwrap_or(0);
        if n > 0 {
            queued = idx.enqueue_all_pending("embed");
            println!("检测到文本向量库为空，已重置 {n} 条记录重新入队");
        }
    }
    println!("已入队 {queued} 个文件（{what}）");
    if queued == 0 {
        return 0;
    }
    idx.start();
    if has_flag(args, "--wait") {
        print!("索引中 …");
        let ok = idx.wait_idle(1800);
        let s = idx.snapshot();
        println!();
        println!(
            "{}：正文 {}（失败 {}），向量 {}（失败 {}），缩略图 {}",
            if ok { "完成" } else { "超时" },
            s.text_done,
            s.text_fail,
            s.emb_done,
            s.emb_fail,
            s.thumb_done
        );
        idx.stop();
        if !ok {
            return 1;
        }
    }
    0
}

fn cmd_rules(ctx: &Ctx) -> i32 {
    match ctx.db.list_rules() {
        Ok(rules) if rules.is_empty() => println!("（还没有分类规则，可在界面的「设置 → 分类规则」里添加）"),
        Ok(rules) => {
            println!("{:<20} {:<10} {:<12} {:<12}", "名称", "字段", "操作", "归类到");
            for r in rules {
                println!(
                    "{:<20} {:<10} {:<12} {:<12}",
                    r.name, r.field, r.op, r.category
                );
            }
        }
        Err(e) => {
            println!("读取规则失败：{e}");
            return 1;
        }
    }
    0
}

fn cmd_serve(ctx: &Ctx, args: &[String]) -> i32 {
    let mut cfg = ctx.cfg.server.clone();
    if let Some(p) = parse_flag(args, "--port").and_then(|s| s.parse().ok()) {
        cfg.port = p;
    }
    println!("正在启动局域网服务 …");
    let state = crate::server::ServerState {
        db: ctx.db.clone(),
        vectors: ctx.vectors.clone(),
        media_vectors: ctx.media.clone(),
        clip_vectors: ctx.clip.clone(),
        audio_vectors: ctx.audio.clone(),
        cfg: Arc::new(ctx.cfg.search.clone()),
        scan_roots: ctx.cfg.scan.roots.clone(),
        disabled_roots: ctx.cfg.scan.disabled_roots.clone(),
        token: cfg.token.clone(),
        allow_write: cfg.allow_write,
        data_dir: ctx.data_dir.clone(),
    };
    let server = crate::server::LanServer::new();
    match server.start(&cfg, state) {
        Ok(url) => {
            println!("已启动：{url}");
            println!("按 Ctrl+C 停止");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        Err(e) => {
            println!("启动失败：{e}");
            1
        }
    }
}
