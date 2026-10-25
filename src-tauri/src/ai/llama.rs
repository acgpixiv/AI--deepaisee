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
        n_ctx: c_int,
        n_threads: c_int,
        device_name: *const c_char,
        err: *mut c_char,
        err_len: c_int,
    ) -> *mut c_void;
    fn ds_llama_device_count() -> c_int;
    fn ds_llama_device_info(
        index: c_int,
        name: *mut c_char,
        name_len: c_int,
        description: *mut c_char,
        description_len: c_int,
        device_id: *mut c_char,
        device_id_len: c_int,
        physical_id: *mut c_char,
        physical_id_len: c_int,
        backend: *mut c_char,
        backend_len: c_int,
        free_bytes: *mut u64,
        total_bytes: *mut u64,
        device_type: *mut c_int,
    ) -> c_int;
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

fn create_handle(
    model_path: *const c_char,
    n_ctx: c_int,
    n_threads: c_int,
    device_name: Option<&CString>,
    err: &mut [u8],
) -> *mut c_void {
    unsafe {
        ds_llama_create(
            model_path,
            n_ctx,
            n_threads,
            device_name.map_or(std::ptr::null(), |s| s.as_ptr()),
            err.as_mut_ptr() as *mut c_char,
            err.len() as c_int,
        )
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GpuDevice {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub total_memory: u64,
    pub free_memory: u64,
    pub integrated: bool,
    #[serde(skip)]
    physical_id: String,
    /// Backend-native name, for example `CUDA0` or `Vulkan0`. This remains
    /// private because the stable frontend identifier is `id`, but it lets
    /// Candle map the selected CUDA device to its ordinal at runtime.
    #[serde(skip)]
    #[allow(dead_code)]
    backend_name: String,
}

fn c_buf_string(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).trim().to_string()
}

/// Enumerate only accelerators exposed by the linked GGML backends. CPU and
/// BLAS devices are intentionally omitted because they cannot satisfy a GPU
/// selection. Entries are deduplicated by PCI id and sorted by total VRAM.
pub fn gpu_devices() -> Vec<GpuDevice> {
    let count = unsafe { ds_llama_device_count() };
    if count <= 0 {
        return Vec::new();
    }
    let mut devices = Vec::with_capacity(count as usize);
    for i in 0..count {
        let mut name = [0u8; 256];
        let mut description = [0u8; 512];
        let mut id = [0u8; 256];
        let mut physical = [0u8; 256];
        let mut backend = [0u8; 128];
        let mut free = 0u64;
        let mut total = 0u64;
        let mut device_type: c_int = 0;
        let ok = unsafe {
            ds_llama_device_info(
                i,
                name.as_mut_ptr() as *mut c_char,
                name.len() as c_int,
                description.as_mut_ptr() as *mut c_char,
                description.len() as c_int,
                id.as_mut_ptr() as *mut c_char,
                id.len() as c_int,
                physical.as_mut_ptr() as *mut c_char,
                physical.len() as c_int,
                backend.as_mut_ptr() as *mut c_char,
                backend.len() as c_int,
                &mut free,
                &mut total,
                &mut device_type,
            )
        };
        if ok != 0 {
            continue;
        }
        let name = c_buf_string(&name);
        let description = c_buf_string(&description);
        let id = c_buf_string(&id);
        let physical_id = c_buf_string(&physical);
        let backend = c_buf_string(&backend);
        if id.is_empty() || name.is_empty() {
            continue;
        }
        let backend_name = name.clone();
        let display_name = if description.is_empty() { name } else { description };
        let item = GpuDevice {
            id,
            name: display_name,
            backend,
            total_memory: total,
            free_memory: free,
            integrated: device_type == 1,
            physical_id,
            backend_name,
        };
        if let Some(existing) = devices.iter_mut().find(|x: &&mut GpuDevice| {
            !item.physical_id.is_empty() && x.physical_id == item.physical_id
        }) {
            // One physical card is one user choice. Prefer CUDA when it is
            // available because the safetensors adapter can bind that path;
            // otherwise retain the accelerator with the largest report.
            let item_cuda = item.backend.eq_ignore_ascii_case("cuda");
            let existing_cuda = existing.backend.eq_ignore_ascii_case("cuda");
            if (item_cuda && !existing_cuda) || item.total_memory > existing.total_memory {
                *existing = item;
            }
        } else {
            devices.push(item);
        }
    }
    devices.sort_by(|a, b| b.total_memory.cmp(&a.total_memory).then_with(|| a.name.cmp(&b.name)));
    devices
}

/// Return the CUDA ordinal corresponding to a frontend device id.
///
/// Candle's CUDA API addresses devices by ordinal while the app persists the
/// stable GGML id (`backend|PCI id`). The GGML CUDA backend names devices
/// `CUDA0`, `CUDA1`, ...; using that native name avoids guessing from the
/// display order (which is sorted by VRAM for the UI).
#[cfg(feature = "candle")]
pub fn cuda_device_ordinal(selected_device: &str) -> Result<usize, String> {
    let devices = gpu_devices();
    let selected = if selected_device.trim().is_empty() || selected_device.eq_ignore_ascii_case("auto") {
        devices
            .iter()
            .filter(|d| d.backend.eq_ignore_ascii_case("cuda"))
            .max_by_key(|d| d.total_memory)
    } else {
        devices.iter().find(|d| d.id == selected_device)
    }
    .ok_or_else(|| {
        if selected_device.trim().is_empty() || selected_device.eq_ignore_ascii_case("auto") {
            "未检测到可用 CUDA 显卡；请安装 NVIDIA 驱动并使用带 CUDA 的构建".to_string()
        } else {
            format!("所选 GPU 不可用于 Candle CUDA：{selected_device}")
        }
    })?;

    if !selected.backend.eq_ignore_ascii_case("cuda") {
        return Err(format!(
            "所选 GPU 后端为 {}，EmbeddingGemma2 Candle 只支持 CUDA；请选择 NVIDIA 显卡",
            selected.backend
        ));
    }
    parse_cuda_ordinal(&selected.backend_name).ok_or_else(|| {
        format!(
            "无法从 CUDA 设备标识 {} 解析设备序号，请刷新 GPU 列表",
            selected.backend_name
        )
    })
}

#[cfg(feature = "candle")]
fn parse_cuda_ordinal(name: &str) -> Option<usize> {
    let lower = name.to_ascii_lowercase();
    let suffix = lower.strip_prefix("cuda")?;
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    suffix.parse().ok()
}

fn error_message(err: &[u8]) -> String {
    unsafe { CStr::from_ptr(err.as_ptr() as *const c_char) }
        .to_string_lossy()
        .to_string()
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
    context_tokens: usize,
    model_name: String,
    device: String,
}

impl LlamaEmbedder {
    /// 加载 GGUF 模型，并要求 llama.cpp 将所有模型层放到选定 GPU。
    pub fn load(
        model_path: &str,
        n_ctx: i32,
        n_threads: i32,
        selected_device: &str,
    ) -> Result<Self, String> {
        if !std::path::Path::new(model_path).exists() {
            return Err(format!("GGUF 模型不存在：{model_path}"));
        }
        let c_path = CString::new(model_path).map_err(|_| "路径含 NUL 字节".to_string())?;
        if selected_device.eq_ignore_ascii_case("cpu") {
            return Err("CPU 推理已禁用，请选择可用 GPU 设备".into());
        }
        let selected = {
            let devices = gpu_devices();
            if devices.is_empty() {
                return Err("当前 llama.cpp 构建或驱动没有可用 GPU；请使用 CUDA/Vulkan 后端重新构建并安装对应驱动".into());
            }
            let device = if selected_device.trim().is_empty() || selected_device.eq_ignore_ascii_case("auto") {
                devices.first()
            } else {
                devices.iter().find(|d| d.id == selected_device)
            }
            .ok_or_else(|| format!("所选 GPU 不可用或已断开：{selected_device}；请重新选择设备"))?;
            Some(device.clone())
        };
        let device_name = selected
            .as_ref()
            .map(|d| CString::new(d.id.as_str()).map_err(|_| "GPU 设备标识包含 NUL 字节"))
            .transpose()?;
        // `0` means automatic in the config. The shim used to translate it
        // to a fixed four threads, which left the CPU underused during
        // tokenisation and prompt preparation on larger machines. Use all
        // available logical CPUs while preserving an explicit user value.
        let effective_threads = if n_threads > 0 {
            n_threads
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get().min(i32::MAX as usize) as i32)
                .unwrap_or(4)
                .max(1)
        };
        let mut err = vec![0u8; 512];
        let raw = create_handle(
            c_path.as_ptr(),
            n_ctx,
            effective_threads,
            device_name.as_ref(),
            &mut err,
        );
        if raw.is_null() {
            let msg = error_message(&err);
            let target = selected.as_ref().map(|d| format!("GPU {} ({})", d.name, d.backend)).unwrap_or_else(|| "GPU".into());
            return Err(format!("在 {target} 上加载 GGUF 失败：{msg}"));
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
            context_tokens: n_ctx.max(256) as usize,
            model_name: name,
            device: selected.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| "GPU".into()),
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
            detail: format!("llama.cpp GGUF · {} · 全部模型层运行于 GPU", self.device),
        }
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_text(&self, text: &str, query: bool) -> Result<Vec<f32>, String> {
        if query {
            return self.embed_one(text, true);
        }
        self.embed_text_batch(&[text.to_owned()])?
            .into_iter()
            .next()
            .ok_or_else(|| "llama.cpp 没有返回文本向量".into())
    }

    fn embed_text_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        const OVERLAP_CHARS: usize = 128;
        let max_chars = self.context_tokens.saturating_div(2).max(128);
        let mut result = Vec::with_capacity(texts.len());
        for text in texts {
            let chars: Vec<char> = text.chars().collect();
            let mut chunks = Vec::new();
            if chars.is_empty() {
                chunks.push(String::new());
            } else {
                let mut start = 0usize;
                while start < chars.len() {
                    let end = (start + max_chars).min(chars.len());
                    chunks.push(chars[start..end].iter().collect::<String>());
                    if end == chars.len() {
                        break;
                    }
                    start = end.saturating_sub(OVERLAP_CHARS);
                }
            }
            let mut merged = vec![0.0f32; self.dim];
            for chunk in &chunks {
                let vector = self.embed_one(chunk, false)?;
                for (dst, value) in merged.iter_mut().zip(vector) {
                    *dst += value;
                }
            }
            let count = chunks.len() as f32;
            for value in &mut merged {
                *value /= count;
            }
            l2_normalize(&mut merged);
            result.push(merged);
        }
        Ok(result)
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

#[cfg(test)]
mod tests {
    use super::gpu_devices;

    #[test]
    fn accelerator_devices_are_named_and_sorted_by_memory() {
        let devices = gpu_devices();
        assert!(devices.iter().all(|device| !device.name.is_empty() && !device.id.is_empty()));
        assert!(devices.windows(2).all(|pair| pair[0].total_memory >= pair[1].total_memory));
        for device in devices {
            println!(
                "GPU: {} | {} | total={} free={}",
                device.name, device.backend, device.total_memory, device.free_memory
            );
        }
    }
}
