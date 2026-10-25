fn main() {
    // ---------------------------------------------------------------- llama.cpp
    //
    // 用 CMake 把 vendor/llama.cpp 与 native/llama_shim.cpp 一起编成静态库。
    // 只在开启了 `llama` feature 时才编译。
    let llama_enabled = std::env::var("CARGO_FEATURE_LLAMA").is_ok();
    if llama_enabled {
        let cuda_mode = std::env::var("DEEPAISEE_CUDA")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let cuda_enabled = match cuda_mode.as_str() {
            "0" | "false" | "off" | "no" => false,
            "1" | "true" | "on" | "yes" => true,
            _ => cuda_toolkit_root().is_some(),
        };
        let vulkan_mode = std::env::var("DEEPAISEE_VULKAN")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let vulkan_enabled = match vulkan_mode.as_str() {
            "0" | "false" | "off" | "no" => false,
            "1" | "true" | "on" | "yes" => true,
            _ => vulkan_sdk_root().is_some(),
        };
        let windows_gnu =
            cfg!(windows) && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");
        if cuda_enabled && windows_gnu {
            panic!("DEEPAISEE_CUDA requires the x86_64-pc-windows-msvc target; NVIDIA CUDA does not support this project's Windows GNU toolchain");
        }
        let mut cfg = cmake::Config::new("native");
        if vulkan_enabled {
            if let Some(out_dir) = windows_ascii_cmake_out_dir() {
                // MSBuild's FileTracker cannot create logs below this workspace's
                // non-ASCII path. Keep CMake intermediates on the same volume at
                // an ASCII path; Cargo/Rust outputs remain in the configured target dir.
                cfg.out_dir(out_dir);
            }
        }
        cfg.define("CMAKE_BUILD_TYPE", "Release")
            .define("LLAMA_BUILD_TESTS", "OFF")
            .define("LLAMA_BUILD_EXAMPLES", "OFF")
            .define("LLAMA_BUILD_SERVER", "OFF")
            .define("LLAMA_BUILD_TOOLS", "OFF")
            .define("LLAMA_CURL", "OFF")
            .define("BUILD_SHARED_LIBS", "OFF")
            .define("DEEPAISEE_CUDA", if cuda_enabled { "ON" } else { "OFF" })
            .define(
                "DEEPAISEE_VULKAN",
                if vulkan_enabled { "ON" } else { "OFF" },
            );

        if vulkan_enabled {
            let sdk = vulkan_sdk_root().expect(
                "DEEPAISEE_VULKAN=1 requires a Vulkan SDK with Include/vulkan/vulkan.h and Bin/glslc.exe; place it in .tools/vulkan or set VULKAN_SDK",
            );
            cfg.define("Vulkan_ROOT", sdk.to_string_lossy().to_string())
                .define("VULKAN_SDK", sdk.to_string_lossy().to_string());
            let spirv_headers = sdk.join("Lib/cmake/SPIRV-Headers");
            if spirv_headers.join("SPIRV-HeadersConfig.cmake").exists() {
                cfg.define(
                    "SPIRV-Headers_DIR",
                    spirv_headers.to_string_lossy().to_string(),
                );
            }
        }

        // Pass the toolkit root to CMake and MSBuild. Visual Studio's CUDA
        // targets need CudaToolkitDir at build time even when CMake found nvcc.
        if cuda_enabled {
            if let Some(cuda_root) = cuda_toolkit_root() {
                let root = std::path::PathBuf::from(cuda_root);
                let nvcc = root
                    .join("bin")
                    .join(if cfg!(windows) { "nvcc.exe" } else { "nvcc" });
                cfg.define("CUDAToolkit_ROOT", root.to_string_lossy().to_string())
                    .define("CMAKE_CUDA_COMPILER", nvcc.to_string_lossy().to_string());
                if cfg!(windows) {
                    // CUDA .targets concatenate path components directly.
                    // Forward slash avoids MSBuild escaping issues.
                    cfg.build_arg(format!("/p:CudaToolkitDir={}/", root.display()));
                }
            }
        }

        println!("cargo:rerun-if-env-changed=DEEPAISEE_CUDA");
        println!("cargo:rerun-if-env-changed=DEEPAISEE_VULKAN");
        println!("cargo:rerun-if-env-changed=VULKAN_SDK");
        println!("cargo:rerun-if-env-changed=DEEPAISEE_VULKAN_ROOT");
        println!("cargo:rerun-if-env-changed=CUDA_PATH");
        println!("cargo:rerun-if-env-changed=CUDA_HOME");
        println!("cargo:rerun-if-env-changed=DEEPAISEE_CUDA_ROOT");
        println!("cargo:rerun-if-env-changed=CUDA_PATH_V12_0");
        println!("cargo:rerun-if-env-changed=CUDA_PATH_V13_1");

        // Windows + GNU 工具链：让 cmake 用 Ninja，并把 ucrt64 的 gcc/g++ 指给它。
        //
        // 编译器目录按优先级找：
        //   1. 环境变量 `DEEPAISEE_UCRT64_BIN`（想自定义就设这个）
        //   2. PATH 里已经有 gcc.exe —— 说明工具链已配好，不干预
        //   3. 常见安装位置（MSYS2 默认路径）
        // 找不到就跳过，让 cmake 自己按 PATH 找编译器（在已激活的 MSYS2 shell 里也能编）。
        if cfg!(windows) && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            let path = std::env::var("PATH").unwrap_or_default();
            let in_path = path
                .split(';')
                .any(|p| std::path::Path::new(p).join("gcc.exe").exists());

            let tools = if let Ok(p) = std::env::var("DEEPAISEE_UCRT64_BIN") {
                Some(std::path::PathBuf::from(p))
            } else if in_path {
                None
            } else {
                [
                    r"C:\msys64\ucrt64\bin",
                    r"C:\msys64\mingw64\bin",
                    r"C:\tools\msys64\ucrt64\bin",
                ]
                .iter()
                .map(std::path::PathBuf::from)
                .find(|p| p.join("gcc.exe").exists())
            };

            if let Some(tools) = tools {
                if tools.join("gcc.exe").exists() {
                    let cc = tools.join("gcc.exe");
                    let cxx = tools.join("g++.exe");
                    cfg.define("CMAKE_C_COMPILER", cc.to_string_lossy().to_string())
                        .define("CMAKE_CXX_COMPILER", cxx.to_string_lossy().to_string());
                    std::env::set_var("PATH", format!("{};{}", tools.to_string_lossy(), path));
                }
            }
            if std::env::var("CMAKE_GENERATOR").is_err() {
                std::env::set_var("CMAKE_GENERATOR", "Ninja");
            }
        }

        let dst = cfg.build();
        // ggml 的 CMake 目标在 Windows 上产出的静态库叫 `ggml.a` / `ggml-base.a`
        // （没有 lib 前缀），而 rustc 的 `-l static=ggml` 只会去找 `libggml.a`。
        // 这里把它们复制一份带前缀的副本到 out/native-libs，再把这个目录加进搜索路径。
        let compat = dst.join("native-libs");
        let _ = std::fs::create_dir_all(&compat);
        for name in [
            "ggml",
            "ggml-base",
            "ggml-cpu",
            "ggml-cuda",
            "ggml-vulkan",
            "llama",
            "llama_shim",
        ] {
            let src = dst.join("lib").join(format!("{name}.a"));
            let alt = dst
                .join("build")
                .join("llama.cpp")
                .join("ggml")
                .join("src")
                .join(format!("{name}.a"));
            let cuda_alt = dst
                .join("build")
                .join("llama.cpp")
                .join("ggml")
                .join("src")
                .join("ggml-cuda")
                .join(format!("{name}.a"));
            let vulkan_alt = dst
                .join("build")
                .join("llama.cpp")
                .join("ggml")
                .join("src")
                .join("ggml-vulkan")
                .join(format!("{name}.a"));
            let from = if src.exists() {
                src
            } else if alt.exists() {
                alt
            } else if vulkan_alt.exists() {
                vulkan_alt
            } else {
                cuda_alt
            };
            if from.exists() {
                let _ = std::fs::copy(&from, compat.join(format!("lib{name}.a")));
            }
        }
        for sub in [
            "native-libs",
            "lib",
            "build",
            "build/llama.cpp/src",
            "build/llama.cpp/ggml/src",
            "build/llama.cpp/ggml/src/ggml-cuda",
            "build/llama.cpp/ggml/src/ggml-vulkan",
            "build/llama.cpp/vendor/hash",
        ] {
            let p = dst.join(sub);
            println!("cargo:rustc-link-search=native={}", p.display());
        }
        println!("cargo:rustc-link-lib=static=llama_shim");
        println!("cargo:rustc-link-lib=static=llama");
        // Link the static backend libraries in dependency order.
        println!("cargo:rustc-link-lib=static=ggml");
        if cuda_enabled {
            println!("cargo:rustc-link-lib=static=ggml-cuda");
        }
        if vulkan_enabled {
            println!("cargo:rustc-link-lib=static=ggml-vulkan");
            if let Some(sdk) = vulkan_sdk_root() {
                let lib_dir = sdk.join("Lib");
                println!("cargo:rustc-link-search=native={}", lib_dir.display());
            }
            println!("cargo:rustc-link-lib=dylib=vulkan-1");
        }
        println!("cargo:rustc-link-lib=static=ggml-base");
        println!("cargo:rustc-link-lib=static=ggml-cpu");
        if cuda_enabled {
            if let Some(cuda_root) = cuda_toolkit_root() {
                let lib_dir = std::path::PathBuf::from(cuda_root).join(if cfg!(windows) {
                    "lib/x64"
                } else {
                    "lib64"
                });
                println!("cargo:rustc-link-search=native={}", lib_dir.display());
            }
            for lib in ["cuda", "cudart", "cublas", "cublasLt"] {
                println!("cargo:rustc-link-lib=dylib={lib}");
            }
        }
        // GNU toolchains need libstdc++; MSVC supplies its own C++ runtime.
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
        println!("cargo:rerun-if-changed=native/llama_shim.cpp");
        println!("cargo:rerun-if-changed=native/CMakeLists.txt");
    }

    tauri_build::build()
}

/// Resolve the CUDA toolkit root consistently for CMake configuration and
/// Rust link search paths. Prefer the canonical CUDA_PATH, then versioned
/// variables used by side-by-side toolkit installations.
fn cuda_toolkit_root() -> Option<std::path::PathBuf> {
    [
        "CUDA_PATH",
        "CUDA_HOME",
        "DEEPAISEE_CUDA_ROOT",
        "CUDA_PATH_V13_1",
        "CUDA_PATH_V12_0",
    ]
    .iter()
    .filter_map(|name| std::env::var_os(name))
    .map(std::path::PathBuf::from)
    .chain(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .into_iter()
            .flat_map(|manifest| {
                let repo = std::path::PathBuf::from(manifest).join("..");
                [repo.join(".tools/cuda"), repo.join(".tools/cuda-toolkit")]
            }),
    )
    .find(|root| {
        root.join("bin").join("nvcc.exe").exists() || root.join("bin").join("nvcc").exists()
    })
}

fn vulkan_sdk_root() -> Option<std::path::PathBuf> {
    ["VULKAN_SDK", "DEEPAISEE_VULKAN_ROOT"]
        .iter()
        .filter_map(|name| std::env::var_os(name))
        .map(std::path::PathBuf::from)
        .chain(
            std::env::var_os("CARGO_MANIFEST_DIR")
                .into_iter()
                .flat_map(|manifest| {
                    let repo = std::path::PathBuf::from(manifest).join("..");
                    [repo.join(".tools/vulkan"), repo.join(".tools/vulkan-sdk")]
                }),
        )
        .find(|root| {
            root.join("Include")
                .join("vulkan")
                .join("vulkan.h")
                .exists()
                && (root.join("Bin").join("glslc.exe").exists()
                    || root.join("bin").join("glslc.exe").exists())
        })
}

/// Return a unique ASCII-only CMake output directory on the Cargo target volume.
/// This is needed for the Vulkan shader-generator subproject on Windows/MSVC.
fn windows_ascii_cmake_out_dir() -> Option<std::path::PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let cargo_out = std::path::PathBuf::from(std::env::var_os("OUT_DIR")?);
    if cargo_out.to_string_lossy().is_ascii() {
        return None;
    }
    let build_id = cargo_out.parent()?.file_name()?;
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("CARGO_MANIFEST_DIR")
                .map(std::path::PathBuf::from)
                .map(|manifest| manifest.join("target"))
        })?;
    let mut volume_root = std::path::PathBuf::new();
    for component in target_dir.components().take(2) {
        volume_root.push(component.as_os_str());
    }
    if volume_root.as_os_str().is_empty() {
        return None;
    }
    Some(volume_root.join(".deepaisee-cmake").join(build_id))
}
