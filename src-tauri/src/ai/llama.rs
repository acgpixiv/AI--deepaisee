//! llama.cpp 绑定（通过 native/llama_shim.cpp 暴露的极简 C ABI）。
//!
//! 只在 `llama` feature 打开时编译。

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::Mutex;

use super::{l2_normalize, EmbedInfo, Embedder};

#[allow(non_camel_case_types)]
type c_void = std::ffi::c_void;

extern "C" {
    fn ds_llama_create(
        model_path: *const c_char,
        n_gpu_layers: c_int,
        n_ctx: c_int,
        n_threads: c_int,
        err: *mut c_char,
        err_len: c_int,
    ) -> *mut c_void;
    fn ds_llama_dim(handle: *mut c_void) -> c_int;
    fn ds_llama_embed(
        handle: *mut c_void,
        text: *const c_char,
        is_query: c_int,
        out: *mut f32,
        out_len: c_int,
        err: *mut c_char,
        err_len: c_int,
    ) -> c_int;
    fn ds_llama_free(handle: *mut c_void);
}

/// 安全包装：Drop 时释放句柄。
struct Handle(*mut c_void);

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ds_llama_free(self.0) };
            self.0 = std::ptr::null_mut();
        }
    }
}

pub struct LlamaEmbedder {
    handle: Mutex<Handle>,
    dim: usize,
    model_name: String,
    device: String,
    n_gpu_layers: i32,
}

impl LlamaEmbedder {
    /// 加载 GGUF 模型。`n_gpu_layers` > 0 表示尽量卸载到 GPU。
    pub fn load(
        model_path: &str,
        n_gpu_layers: i32,
        n_ctx: i32,
        n_threads: i32,
    ) -> Result<Self, String> {
        if !std::path::Path::new(model_path).exists() {
            return Err(format!("GGUF 模型不存在：{model_path}"));
        }
        let c_path = CString::new(model_path).map_err(|_| "路径含 NUL 字节".to_string())?;
        let mut err = vec![0u8; 512];
        let raw = unsafe {
            ds_llama_create(
                c_path.as_ptr(),
                n_gpu_layers,
                n_ctx,
                n_threads,
                err.as_mut_ptr() as *mut c_char,
                err.len() as c_int,
            )
        };
        if raw.is_null() {
            let msg = unsafe { CStr::from_ptr(err.as_ptr() as *const c_char) }
                .to_string_lossy()
                .to_string();
            return Err(format!("加载 GGUF 失败：{msg}"));
        }
        let dim = unsafe { ds_llama_dim(raw) };
        if dim <= 0 {
            unsafe { ds_llama_free(raw) };
            return Err("模型未报告 embedding 维度".into());
        }
        let name = std::path::Path::new(model_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "gguf".into());
        Ok(Self {
            handle: Mutex::new(Handle(raw)),
            dim: dim as usize,
            model_name: name,
            device: if n_gpu_layers > 0 { "cuda".into() } else { "cpu".into() },
            n_gpu_layers,
        })
    }

    fn embed_one(&self, text: &str, is_query: bool) -> Result<Vec<f32>, String> {
        let c_text = CString::new(text).map_err(|_| "文本含 NUL 字节".to_string())?;
        let mut out = vec![0f32; self.dim];
        let mut err = vec![0u8; 512];
        let guard = self.handle.lock().unwrap();
        let n = unsafe {
            ds_llama_embed(
                guard.0,
                c_text.as_ptr(),
                if is_query { 1 } else { 0 },
                out.as_mut_ptr(),
                out.len() as c_int,
                err.as_mut_ptr() as *mut c_char,
                err.len() as c_int,
            )
        };
        if n < 0 {
            let msg = unsafe { CStr::from_ptr(err.as_ptr() as *const c_char) }
                .to_string_lossy()
                .to_string();
            return Err(format!("编码失败：{msg}"));
        }
        let mut v = out;
        l2_normalize(&mut v);
        Ok(v)
    }
}

impl Embedder for LlamaEmbedder {
    fn info(&self) -> EmbedInfo {
        EmbedInfo {
            backend: "llama.cpp".into(),
            model: self.model_name.clone(),
            dim: self.dim,
            device: self.device.clone(),
            // llama.cpp 只处理文本；图片/音频/视频由 hash 兜底
            modalities: vec!["text".into()],
            ready: true,
            detail: format!("llama.cpp GGUF · {} 层卸载 GPU", self.n_gpu_layers),
        }
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_text(&self, text: &str, query: bool) -> Result<Vec<f32>, String> {
        self.embed_one(text, query)
    }

    fn embed_image(&self, _path: &std::path::Path) -> Result<Vec<f32>, String> {
        Err("llama.cpp 后端不支持图片，请改用哈希基线".into())
    }

    fn embed_audio(&self, _path: &std::path::Path) -> Result<Vec<f32>, String> {
        Err("llama.cpp 后端不支持音频，请改用哈希基线".into())
    }

    fn embed_video(&self, _path: &std::path::Path) -> Result<Vec<f32>, String> {
        Err("llama.cpp 后端不支持视频，请改用哈希基线".into())
    }
}
