//! deepaisee —— Tauri 2 + Vue 3 + llama.cpp 的本地 AI 磁盘搜索工具。
pub mod ai;
pub mod cli;
pub mod cmd;
pub mod config;
pub mod db;
pub mod fileops;
pub mod index;
pub mod media;
pub mod models;
pub mod py;
pub mod scan;
pub mod search;
pub mod selftest;
pub mod server;
pub mod state;
pub mod textract;
pub mod util;
pub mod vector;
pub mod vision;
pub mod watch;

pub const APP_VERSION: &str = "0.0.20";
pub const APP_DISPLAY_VERSION: &str = "0.20";

use std::sync::Arc;
#[cfg(debug_assertions)]
use tauri::Manager;

pub fn run() {
    let app_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let state = match state::AppState::new(app_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("初始化失败: {e}");
            // 兜底：把错误写文件，方便无控制台的发布版排查
            let _ = std::fs::write("deepaisee-error.txt", format!("初始化失败: {e}"));
            return;
        }
    };
    let _ = state.db.log("app", &format!("deepaisee {} 启动", APP_DISPLAY_VERSION));
    let _ = state.rebuild_indexer();
    let shared = Arc::new(state);

    // 后台自动加载嵌入模型（llama.cpp 加载 GGUF 约 0.5~2 秒，不阻塞窗口出现）
    {
        let st = Arc::clone(&shared);
        std::thread::spawn(move || {
            let trace = |msg: String| {
                let _ = std::fs::write(
                    st.data_dir.join("autoload.log"),
                    format!("{}\n{}", chrono::Local::now().format("%H:%M:%S"), msg),
                );
            };
            let cfg = st.cfg.read().clone();
            trace(format!(
                "thread start auto_load={} path={}",
                cfg.model.auto_load, cfg.model.embed_model
            ));
            // 便携版没写过配置时 embed_model 是空的。
            // 这里自动在 exe 同级 models/ 与 data/ 里找 .gguf，
            // 让「解压即用」成立，而不是逼用户先去设置页填路径。
            let mut cfg = cfg;
            if cfg.model.embed_model.trim().is_empty() {
                for dir in [st.app_dir.join("models"), st.data_dir.join("models")] {
                    let entries = crate::models::scan(&cfg.model, std::slice::from_ref(&dir));
                    // Prefer a complete multimodal safetensors package when
                    // present. Its vision/audio towers would be lost if the
                    // text-only GGUF is selected first.
                    let found = entries
                        .iter()
                        .find(|m| {
                            m.kind == "embed"
                                && m.format == "safetensors"
                                && m.modalities.iter().any(|x| matches!(x.as_str(), "image" | "audio" | "video"))
                        })
                        .or_else(|| {
                            entries.iter().find(|m| {
                                m.direct_loadable && m.kind == "embed"
                            })
                        });
                    if let Some(found) = found {
                        cfg.model.embed_model = found.path.clone();
                        let mut c = st.cfg.read().clone();
                        c.model.embed_model = cfg.model.embed_model.clone();
                        let _ = c.save(&crate::config::config_path(Some(&st.app_dir)));
                        *st.cfg.write() = c;
                        trace(format!("autodetect model={}", cfg.model.embed_model));
                        break;
                    }
                }
            }
            if cfg.model.auto_load && !cfg.model.embed_model.trim().is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(600));
                match crate::ai::build_embedder_with_batch(&cfg.model, cfg.index.batch_size) {
                    Ok(emb) => {
                        let emb: Arc<dyn crate::ai::Embedder> = Arc::from(emb);
                        let emb = crate::ai::maybe_replicate(
                            emb,
                            &cfg.model,
                            cfg.index.batch_size,
                        );
                        let dim = emb.dim();
                        let info = emb.info();
                        let shared_media = info.backend == "candle"
                            && info.modalities.iter().any(|m| matches!(m.as_str(), "image" | "audio" | "video"));
                        let media_dim = if shared_media { dim } else { 256 };
                        trace(format!("loaded dim={} backend={}", dim, info.backend));
                        *st.embedder.write() = Arc::clone(&emb);
                        let mut c = st.cfg.read().clone();
                        c.model.embedding_dim = dim;
                        let _ = c.save(&crate::config::config_path(Some(&st.app_dir)));
                        *st.cfg.write() = c;
                        if let Ok(v) = crate::vector::VectorStore::open(
                            &st.data_dir.join("vectors"),
                            dim,
                        ) {
                            // 与 model_load 同理：维度变化清空了旧向量，
                            // 但库里 embed_state 仍是 ok，必须标回 pending 才会重建。
                            if v.len() == 0 && st.db.count_state("embed", "ok").unwrap_or(0) > 0 {
                                let _ = st.db.reset_states("embed");
                                let _ = st.db.log(
                                    "model",
                                    "嵌入维度变化，已把全部文件标记为待重新向量化",
                                );
                            }
                            *st.vectors.write() = Arc::new(v);
                        }
                        let media_dir = st.data_dir.join("vectors_media");
                        let old_media_count = st.media_vectors.read().len();
                        if let Ok(v) = crate::vector::VectorStore::open(&media_dir, media_dim) {
                            if old_media_count > 0 && v.len() == 0 {
                                let _ = st.db.reset_states("embed");
                                let _ = st.db.log("model", "媒体向量空间已切换，已把文件标记为待重新向量化");
                            }
                            *st.media_vectors.write() = Arc::new(v);
                        }
                        let media_embedder: Arc<dyn crate::ai::Embedder> = if shared_media {
                            Arc::clone(&emb)
                        } else {
                            Arc::new(crate::ai::hash::HashEmbedder::new())
                        };
                        *st.media_embedder.write() = media_embedder;
                        let _ = st.rebuild_indexer();
                        // 刚重置过状态的话，立刻排队，用户无需手动点重建。
                        // The model is ready for search; pending files are
                        // indexed only after an explicit user action.
                        let _ = st.db.log(
                            "model",
                            &format!(
                                "自动加载 {}（{} 维 / {}）",
                                info.model, info.dim, info.device
                            ),
                        );
                    }
                    Err(e) => {
                        trace(format!("load failed: {e}"));
                        let _ = st.db.log("error", &format!("自动加载模型失败：{e}"));
                    }
                }
            }
        });
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(shared)
        .invoke_handler(tauri::generate_handler![
            cmd::app_info,
            cmd::get_config,
            cmd::save_config,
            cmd::search,
            cmd::search_semantic,
            cmd::similar,
            cmd::stats,
            cmd::index_roots,
            cmd::recent,
            cmd::scan_start,
            cmd::scan_cancel,
            cmd::scan_progress,
            cmd::index_start,
            cmd::index_cancel,
            cmd::index_stats,
            cmd::model_info,
            cmd::gpu_devices,
            cmd::model_load,
            cmd::model_deactivate,
            cmd::model_list,
            cmd::model_dir_path,
            cmd::model_delete,
            cmd::model_import,
            cmd::model_import_folder,
            cmd::open_path,
            cmd::reveal_path,
            cmd::rename_path,
            cmd::ensure_preview,
            cmd::op_export,
            cmd::rules_list,
            cmd::rules_save,
            cmd::rules_apply,
            cmd::activity_list,
            cmd::tree_data,
            cmd::watch_start,
            cmd::watch_stop,
            cmd::watch_status,
            cmd::server_start,
            cmd::server_stop,
            cmd::server_status,
            cmd::db_vacuum,
            cmd::db_clear,
            cmd::data_dir_path,
            cmd::extension_status,
            cmd::extension_install,
        ])
        .setup(|app| {
            // 开发期打开 devtools
            #[cfg(debug_assertions)]
            {
                if let Some(w) = app.get_webview_window("main") {
                    w.open_devtools();
                }
            }
            let _ = app;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("deepaisee 初始化失败")
        .run(|app_handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                use tauri::Manager;
                let state = app_handle.state::<Arc<state::AppState>>();
                if let Some(indexer) = state.indexer() {
                    indexer.stop();
                }
            }
        });
}
