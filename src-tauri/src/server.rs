//! 局域网共享：axum HTTP 服务 + 移动端页面。
use crate::config::ServerConfig;
use crate::db::Db;
use crate::search::{SearchEngine, SearchQuery};
use crate::vector::VectorStore;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use parking_lot::Mutex;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone)]
pub struct ServerState {
    pub db: Arc<Db>,
    pub vectors: Arc<VectorStore>,
    pub media_vectors: Arc<VectorStore>,
    pub clip_vectors: Arc<VectorStore>,
    pub audio_vectors: Arc<VectorStore>,
    pub cfg: Arc<crate::config::SearchConfig>,
    pub scan_roots: Vec<String>,
    pub disabled_roots: Vec<String>,
    pub token: String,
    pub allow_write: bool,
    pub data_dir: std::path::PathBuf,
}

pub struct LanServer {
    stop_tx: Mutex<Option<oneshot::Sender<()>>>,
    running: Mutex<Option<String>>,
}

impl Default for LanServer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanServer {
    pub fn new() -> Self {
        Self { stop_tx: Mutex::new(None), running: Mutex::new(None) }
    }

    pub fn running(&self) -> bool {
        self.running.lock().is_some()
    }

    pub fn url(&self) -> Option<String> {
        self.running.lock().clone()
    }

    pub fn start(&self, cfg: &ServerConfig, state: ServerState) -> Result<String, String> {
        self.stop();
        let addr: SocketAddr = format!("{}:{}", cfg.bind, cfg.port)
            .parse()
            .map_err(|e| format!("地址无效: {e}"))?;
        let (tx, rx) = oneshot::channel::<()>();
        let url = format!("http://{}:{}", local_ip(), cfg.port);
        let app = build_router(state);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("运行时创建失败: {e}")));
                    return;
                }
            };
            rt.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => {
                        let _ = ready_tx.send(Ok(()));
                        l
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("端口 {addr} 绑定失败: {e}")));
                        return;
                    }
                };
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = rx.await;
                    })
                    .await;
            });
        });
        match ready_rx.recv_timeout(std::time::Duration::from_secs(6)) {
            Ok(Ok(())) => {
                *self.stop_tx.lock() = Some(tx);
                *self.running.lock() = Some(url.clone());
                Ok(url)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err("服务启动超时".into()),
        }
    }

    pub fn stop(&self) {
        if let Some(tx) = self.stop_tx.lock().take() {
            let _ = tx.send(());
        }
        *self.running.lock() = None;
    }
}

pub fn local_ip() -> String {
    use std::net::UdpSocket;
    if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
        if s.connect("8.8.8.8:80").is_ok() {
            if let Ok(a) = s.local_addr() {
                return a.ip().to_string();
            }
        }
    }
    "127.0.0.1".into()
}

fn auth(state: &ServerState, token: Option<&String>) -> Result<(), Response> {
    if state.token.is_empty() {
        return Ok(());
    }
    if token.map(|t| t == &state.token).unwrap_or(false) {
        return Ok(());
    }
    Err((StatusCode::UNAUTHORIZED, "需要访问令牌").into_response())
}

fn build_router(state: ServerState) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/api/search", get(api_search))
        .route("/api/stats", get(api_stats))
        .route("/api/facets", get(api_facets))
        .route("/thumb/{id}", get(thumb))
        .route("/file/{id}", get(file))
        .with_state(state)
}

async fn index_page() -> Html<&'static str> {
    Html(MOBILE_HTML)
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    kind: Option<String>,
    token: Option<String>,
    limit: Option<usize>,
}

async fn api_search(
    State(st): State<ServerState>,
    Query(p): Query<SearchParams>,
) -> Response {
    if let Err(e) = auth(&st, p.token.as_ref()) {
        return e;
    }
    let text = p.q.unwrap_or_default();
    let mut q = SearchQuery::default();
    q.text = text;
    if let Some(k) = p.kind {
        if !k.is_empty() && k != "all" {
            q.kinds = vec![k];
        }
    }
    q.limit = Some(p.limit.unwrap_or(50));
    crate::search::apply_enabled_root_scope(&mut q, &st.scan_roots, &st.disabled_roots);
    let engine = SearchEngine {
        db: &st.db,
        vectors: &st.vectors,
        media_vectors: &st.media_vectors,
        clip_vectors: &st.clip_vectors,
        audio_vectors: &st.audio_vectors,
        cfg: &st.cfg,
    };
    let t0 = std::time::Instant::now();
    let hits = engine.search(&q);
    let ms = t0.elapsed().as_millis();
    axum::Json(serde_json::json!({
        "total": hits.len(),
        "ms": ms,
        "hits": hits,
    }))
    .into_response()
}

async fn api_stats(State(st): State<ServerState>) -> Response {
    let total = st.db.count().unwrap_or(0);
    let bytes = st.db.total_bytes().unwrap_or(0);
    let kinds = st.db.kind_stats().unwrap_or_default();
    axum::Json(serde_json::json!({
        "total": total,
        "total_size": bytes,
        "kinds": kinds,
        "files": total,
    }))
    .into_response()
}

async fn api_facets(State(st): State<ServerState>, Query(p): Query<SearchParams>) -> Response {
    if let Err(e) = auth(&st, p.token.as_ref()) {
        return e;
    }
    let mut q = SearchQuery::default();
    q.text = p.q.unwrap_or_default();
    crate::search::apply_enabled_root_scope(&mut q, &st.scan_roots, &st.disabled_roots);
    let engine = SearchEngine {
        db: &st.db,
        vectors: &st.vectors,
        media_vectors: &st.media_vectors,
        clip_vectors: &st.clip_vectors,
        audio_vectors: &st.audio_vectors,
        cfg: &st.cfg,
    };
    axum::Json(engine.facets(&q)).into_response()
}

async fn thumb(State(st): State<ServerState>, AxPath(id): AxPath<i64>) -> Response {
    let p = st.data_dir.join("thumbs").join(format!("{id}.jpg"));
    match std::fs::read(&p) {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/jpeg")], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "no thumb").into_response(),
    }
}

async fn file(State(st): State<ServerState>, AxPath(id): AxPath<i64>) -> Response {
    let Ok(Some(row)) = st.db.get_by_id(id) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    match std::fs::read(&row.path) {
        Ok(bytes) => {
            let ct = mime_of(&row.ext);
            ([(header::CONTENT_TYPE, ct), (header::CONTENT_DISPOSITION, "inline")], bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "file missing").into_response(),
    }
}

fn mime_of(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

const MOBILE_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<title>DeepDark_Oracle</title>
<style>
:root{--bg:#0b0f16;--panel:#121826;--border:#24304a;--text:#dbe4f3;--muted:#8ea0bf;--accent:#4f8cff}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--text);font:15px/1.6 -apple-system,"Segoe UI",system-ui,sans-serif}
header{padding:14px 16px 8px}
h1{font-size:17px;margin:0 0 2px}
.sub{color:var(--muted);font-size:12px}
.search{display:flex;gap:8px;padding:10px 16px}
input{flex:1;background:var(--panel);border:1px solid var(--border);color:var(--text);border-radius:10px;padding:11px 12px;font-size:15px;outline:none}
input:focus{border-color:var(--accent)}
button{background:var(--accent);color:#fff;border:0;border-radius:10px;padding:11px 16px;font-size:15px}
.chips{display:flex;gap:6px;padding:0 16px 10px;overflow:auto}
.chip{background:var(--panel);border:1px solid var(--border);color:var(--muted);border-radius:999px;padding:5px 12px;font-size:13px;white-space:nowrap}
.chip.on{background:var(--accent);border-color:var(--accent);color:#fff}
.list{padding:0 12px 40px}
.item{display:flex;gap:10px;padding:10px;border-bottom:1px solid var(--border);text-decoration:none;color:inherit}
.thumb{width:56px;height:56px;border-radius:8px;background:#1b2436;object-fit:cover;flex:none}
.name{font-size:14px;word-break:break-all}
.meta{color:var(--muted);font-size:12px;margin-top:3px}
.snip{color:#a8b6cf;font-size:12px;margin-top:4px;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden}
.empty{color:var(--muted);text-align:center;padding:40px 0}
</style></head><body>
<header><h1>DeepDark_Oracle</h1><div class="sub">局域网文件搜索</div></header>
<div class="search"><input id="q" placeholder="搜索文件名或内容…"><button onclick="go()">搜索</button></div>
<div class="chips" id="chips"></div>
<div class="list" id="list"><div class="empty">输入关键词开始搜索</div></div>
<script>
const KINDS=[["all","全部"],["image","图片"],["video","视频"],["audio","音频"],["doc","文档"],["text","文本"]];
let kind="all";
const chips=document.getElementById("chips");
KINDS.forEach(([v,t])=>{const b=document.createElement("div");b.className="chip"+(v==="all"?" on":"");b.textContent=t;
 b.onclick=()=>{kind=v;[...chips.children].forEach(c=>c.classList.remove("on"));b.classList.add("on");go();};chips.appendChild(b);});
function esc(s){return (s||"").replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]))}
function fmt(n){if(n<1024)return n+" B";const u=["KB","MB","GB","TB"];let i=-1;do{n/=1024;i++}while(n>=1024&&i<3);return n.toFixed(1)+" "+u[i]}
async function go(){
 const q=document.getElementById("q").value.trim();
 if(!q){document.getElementById("list").innerHTML='<div class="empty">输入关键词开始搜索</div>';return;}
 const r=await fetch(`/api/search?q=${encodeURIComponent(q)}&kind=${kind}`);
 const d=await r.json();
 const list=document.getElementById("list");
 if(!d.hits.length){list.innerHTML='<div class="empty">没有找到结果</div>';return;}
 list.innerHTML=d.hits.map(h=>`<a class="item" href="/file/${h.id}" target="_blank">
  ${h.thumb?`<img class="thumb" src="/thumb/${h.id}" loading="lazy">`:""}
  <div><div class="name">${esc(h.name)}</div>
  <div class="meta">${esc(h.kind_label)} · ${fmt(h.size)} · ${esc(h.mtime_label)}</div>
  ${h.snippet?`<div class="snip">${esc(h.snippet)}</div>`:""}</div></a>`).join("")
  +`<div class="empty" style="padding:14px 0;font-size:12px">${d.total} 条 · ${d.ms} ms</div>`;
}
document.getElementById("q").addEventListener("keydown",e=>{if(e.key==="Enter")go()});
</script></body></html>"#;
