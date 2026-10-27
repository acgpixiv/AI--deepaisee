//! 极简 ONNX Runtime 绑定。
//!
//! 不引第三方 crate：直接用 `LoadLibraryW` + `OrtGetApiBase` 手写 FFI。
//! 好处是打包只需附一个 `onnxruntime.dll`，编译期零依赖，也不用把几十兆
//! 头文件塞进仓库。
//!
//! 字段索引沿用 `onnxruntime_c_api.h` 里 `struct OrtApi` 的顺序（ABI v20，
//! 即 ONNX Runtime 1.20+）。只取用得到的十几个。
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void, CStr, CString};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
}

type Status = *mut c_void;
type OrtPtr = *mut c_void;

const IDX_GET_ERR_MSG: usize = 2;
const IDX_CREATE_ENV: usize = 3;
const IDX_CREATE_SESSION: usize = 7;
const IDX_RUN: usize = 9;
const IDX_CREATE_SESSION_OPTIONS: usize = 10;
const IDX_SET_GRAPH_OPT: usize = 23;
const IDX_SET_INTRA_THREADS: usize = 24;
const IDX_CREATE_TENSOR_WITH_DATA: usize = 49;
const IDX_GET_TENSOR_DATA: usize = 51;
const IDX_GET_DIMS_COUNT: usize = 61;
const IDX_GET_DIMS: usize = 62;
const IDX_GET_TENSOR_SHAPE: usize = 65;
const IDX_CREATE_CPU_MEM: usize = 69;
const IDX_RELEASE_ENV: usize = 92;
const IDX_RELEASE_STATUS: usize = 93;
const IDX_RELEASE_MEM_INFO: usize = 94;
const IDX_RELEASE_SESSION: usize = 95;
const IDX_RELEASE_VALUE: usize = 96;
const IDX_RELEASE_TENSOR_SHAPE: usize = 99;
const IDX_RELEASE_SESSION_OPTIONS: usize = 100;

macro_rules! api_fn {
    ($api:expr, $idx:expr, $ty:ty) => {{
        let p = *(($api) as *const usize).add($idx);
        std::mem::transmute::<usize, $ty>(p)
    }};
}

fn wide(s: &Path) -> Vec<u16> {
    s.as_os_str().encode_wide().chain(Some(0)).collect()
}

unsafe fn check(api: OrtPtr, st: Status) -> Result<(), String> {
    if st.is_null() {
        return Ok(());
    }
    let get_msg: unsafe extern "system" fn(Status) -> *const c_char = api_fn!(api, IDX_GET_ERR_MSG, _);
    let rel: unsafe extern "system" fn(Status) = api_fn!(api, IDX_RELEASE_STATUS, _);
    let msg = CStr::from_ptr(get_msg(st)).to_string_lossy().into_owned();
    rel(st);
    Err(msg)
}

#[repr(C)]
struct OrtApiBase {
    get_api: unsafe extern "system" fn(u32) -> *const c_void,
    get_version_string: unsafe extern "system" fn() -> *const c_char,
}

/// 输入张量数据：整数（token id）或浮点（像素 / 音频特征）。
pub enum Data<'a> {
    F32(&'a [f32]),
    I64(&'a [i64]),
}

impl Data<'_> {
    fn as_ptr(&self) -> *mut c_void {
        match self {
            Data::F32(v) => v.as_ptr() as *mut c_void,
            Data::I64(v) => v.as_ptr() as *mut c_void,
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Data::F32(v) => v.len() * 4,
            Data::I64(v) => v.len() * 8,
        }
    }
    fn elem_type(&self) -> i32 {
        match self {
            Data::F32(_) => 1,
            Data::I64(_) => 7,
        }
    }
}

struct Inner {
    api: OrtPtr,
    env: OrtPtr,
    mem: OrtPtr,
    threads: i32,
    // 保持 DLL 句柄存活：最后一个引用释放前不能被卸载。
    _dll: *mut c_void,
}

// 函数表与 env/mem 都是只读共享的，跨线程用安全。
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe {
            let release_mem: unsafe extern "system" fn(OrtPtr) = api_fn!(self.api, IDX_RELEASE_MEM_INFO, _);
            let release_env: unsafe extern "system" fn(OrtPtr) = api_fn!(self.api, IDX_RELEASE_ENV, _);
            if !self.mem.is_null() {
                release_mem(self.mem);
            }
            if !self.env.is_null() {
                release_env(self.env);
            }
        }
    }
}

/// ONNX Runtime 运行时句柄。进程内建一个，`Session` 共享它。
#[derive(Clone)]
pub struct Ort {
    inner: Arc<Inner>,
}

impl Ort {
    /// `dll` 指向 onnxruntime.dll；`threads <= 0` 时按 CPU 并行度取。
    pub fn load(dll: &Path, threads: i32) -> Result<Ort, String> {
        unsafe {
            let h = LoadLibraryW(wide(dll).as_ptr());
            if h.is_null() {
                return Err(format!("加载 onnxruntime.dll 失败：{}", dll.display()));
            }
            let p = GetProcAddress(h, b"OrtGetApiBase\0".as_ptr());
            if p.is_null() {
                return Err("onnxruntime.dll 里找不到 OrtGetApiBase".into());
            }
            let f: unsafe extern "system" fn() -> *mut c_void = std::mem::transmute(p);
            let base = f() as *const OrtApiBase;
            let api = ((*base).get_api)(20) as OrtPtr;
            if api.is_null() {
                return Err("onnxruntime 版本过低（需要 ABI v20，即 1.20 及以上）".into());
            }
            let mut env: OrtPtr = ptr::null_mut();
            let create_env: unsafe extern "system" fn(i32, *const c_char, *mut OrtPtr) -> Status =
                api_fn!(api, IDX_CREATE_ENV, _);
            let name = CString::new("DeepDark_Oracle").unwrap();
            check(api, create_env(3, name.as_ptr(), &mut env))?;

            let mut mem: OrtPtr = ptr::null_mut();
            let create_cpu: unsafe extern "system" fn(i32, i32, *mut OrtPtr) -> Status =
                api_fn!(api, IDX_CREATE_CPU_MEM, _);
            check(api, create_cpu(0, 0, &mut mem))?;

            let n = if threads > 0 {
                threads
            } else {
                std::thread::available_parallelism().map(|v| v.get()).unwrap_or(4).min(8) as i32
            };
            Ok(Ort { inner: Arc::new(Inner { api, env, mem, threads: n, _dll: h }) })
        }
    }

    pub fn threads(&self) -> i32 {
        self.inner.threads
    }

    /// 打开一个模型文件。
    pub fn session(&self, model: &Path) -> Result<Session, String> {
        unsafe {
            let api = self.inner.api;
            let mut opts: OrtPtr = ptr::null_mut();
            let create_opts: unsafe extern "system" fn(*mut OrtPtr) -> Status =
                api_fn!(api, IDX_CREATE_SESSION_OPTIONS, _);
            check(api, create_opts(&mut opts))?;
            let set_threads: unsafe extern "system" fn(OrtPtr, i32) -> Status =
                api_fn!(api, IDX_SET_INTRA_THREADS, _);
            let set_graph: unsafe extern "system" fn(OrtPtr, i32) -> Status = api_fn!(api, IDX_SET_GRAPH_OPT, _);
            let create_sess: unsafe extern "system" fn(OrtPtr, *const u16, OrtPtr, *mut OrtPtr) -> Status =
                api_fn!(api, IDX_CREATE_SESSION, _);
            let release_opts: unsafe extern "system" fn(OrtPtr) = api_fn!(api, IDX_RELEASE_SESSION_OPTIONS, _);

            let mut r = check(api, set_threads(opts, self.inner.threads));
            if r.is_ok() {
                r = check(api, set_graph(opts, 99));
            }
            let mut sess: OrtPtr = ptr::null_mut();
            if r.is_ok() {
                let mp = wide(model);
                r = check(api, create_sess(self.inner.env, mp.as_ptr(), opts, &mut sess));
            }
            release_opts(opts);
            r?;
            Ok(Session { inner: Arc::clone(&self.inner), sess })
        }
    }
}

/// 一个已加载的 ONNX 模型。
pub struct Session {
    inner: Arc<Inner>,
    sess: OrtPtr,
}

unsafe impl Send for Session {}
unsafe impl Sync for Session {}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let release: unsafe extern "system" fn(OrtPtr) = api_fn!(self.inner.api, IDX_RELEASE_SESSION, _);
            release(self.sess);
        }
    }
}

impl Session {
    /// 跑一次推理，返回每个输出的原始 f32 数据。
    ///
    /// `inputs` 为 `(名字, 形状, 数据)`。模型输出若固定 batch=1，就一定
    /// 一条一条喂——批量会报 "Clashing resolution for expression"。
    pub fn run(&self, inputs: &[(&str, &[i64], Data<'_>)], outputs: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        unsafe {
            let api = self.inner.api;
            let create_tensor: unsafe extern "system" fn(
                OrtPtr, *mut c_void, usize, *const i64, usize, i32, *mut OrtPtr,
            ) -> Status = api_fn!(api, IDX_CREATE_TENSOR_WITH_DATA, _);
            let run: unsafe extern "system" fn(
                OrtPtr, OrtPtr, *const *const c_char, *const OrtPtr, usize, *const *const c_char, usize, *mut OrtPtr,
            ) -> Status = api_fn!(api, IDX_RUN, _);
            let get_shape: unsafe extern "system" fn(OrtPtr, *mut OrtPtr) -> Status = api_fn!(api, IDX_GET_TENSOR_SHAPE, _);
            let get_dims_count: unsafe extern "system" fn(OrtPtr, *mut usize) -> Status = api_fn!(api, IDX_GET_DIMS_COUNT, _);
            let get_dims: unsafe extern "system" fn(OrtPtr, *mut i64, usize) -> Status = api_fn!(api, IDX_GET_DIMS, _);
            let get_data: unsafe extern "system" fn(OrtPtr, *mut *mut c_void) -> Status = api_fn!(api, IDX_GET_TENSOR_DATA, _);
            let release_value: unsafe extern "system" fn(OrtPtr) = api_fn!(api, IDX_RELEASE_VALUE, _);
            let release_shape: unsafe extern "system" fn(OrtPtr) = api_fn!(api, IDX_RELEASE_TENSOR_SHAPE, _);

            let in_names: Vec<CString> = inputs.iter().map(|(n, ..)| CString::new(*n).unwrap()).collect();
            let in_ptrs: Vec<*const c_char> = in_names.iter().map(|c| c.as_ptr()).collect();
            let mut tensors: Vec<OrtPtr> = Vec::with_capacity(inputs.len());
            for (_, dims, data) in inputs.iter() {
                let mut t: OrtPtr = ptr::null_mut();
                let st = create_tensor(
                    self.inner.mem,
                    data.as_ptr(),
                    data.bytes(),
                    dims.as_ptr(),
                    dims.len(),
                    data.elem_type(),
                    &mut t,
                );
                if let Err(e) = check(api, st) {
                    for t in tensors.drain(..) {
                        release_value(t);
                    }
                    return Err(format!("创建输入张量失败：{e}"));
                }
                tensors.push(t);
            }

            let out_names: Vec<CString> = outputs.iter().map(|n| CString::new(*n).unwrap()).collect();
            let out_ptrs: Vec<*const c_char> = out_names.iter().map(|c| c.as_ptr()).collect();
            let mut outs: Vec<OrtPtr> = vec![ptr::null_mut(); outputs.len()];
            let st = run(
                self.sess,
                ptr::null_mut(),
                in_ptrs.as_ptr(),
                tensors.as_ptr(),
                tensors.len(),
                out_ptrs.as_ptr(),
                out_ptrs.len(),
                outs.as_mut_ptr(),
            );
            let r = check(api, st);
            for t in tensors {
                release_value(t);
            }
            if let Err(e) = r {
                for o in outs.iter().filter(|o| !o.is_null()) {
                    release_value(*o);
                }
                return Err(format!("推理失败：{e}"));
            }

            let mut result = Vec::with_capacity(outs.len());
            for o in outs {
                let mut sh: OrtPtr = ptr::null_mut();
                check(api, get_shape(o, &mut sh))?;
                let mut nd = 0usize;
                check(api, get_dims_count(sh, &mut nd))?;
                let mut dims = vec![0i64; nd];
                check(api, get_dims(sh, dims.as_mut_ptr(), nd))?;
                let mut pdata: *mut c_void = ptr::null_mut();
                check(api, get_data(o, &mut pdata))?;
                let n: usize = dims.iter().product::<i64>().max(0) as usize;
                result.push(std::slice::from_raw_parts(pdata as *const f32, n).to_vec());
                release_shape(sh);
                release_value(o);
            }
            Ok(result)
        }
    }
}
