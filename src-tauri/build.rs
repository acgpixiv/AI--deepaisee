fn main() {
    // ---------------------------------------------------------------- llama.cpp
    //
    // 用 CMake 把 vendor/llama.cpp 与 native/llama_shim.cpp 一起编成静态库。
    // 只在开启了 `llama` feature 时才编译（默认关闭，这样纯 Rust 部分也能先跑通）。
    let llama_enabled = std::env::var("CARGO_FEATURE_LLAMA").is_ok();
    if llama_enabled {
        let mut cfg = cmake::Config::new("native");
        cfg.define("CMAKE_BUILD_TYPE", "Release")
            .define("LLAMA_BUILD_TESTS", "OFF")
            .define("LLAMA_BUILD_EXAMPLES", "OFF")
            .define("LLAMA_BUILD_SERVER", "OFF")
            .define("LLAMA_BUILD_TOOLS", "OFF")
            .define("LLAMA_CURL", "OFF")
            .define("BUILD_SHARED_LIBS", "OFF");

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
        for name in ["ggml", "ggml-base", "ggml-cpu", "vendor-hash"] {
            let src = dst.join("lib").join(format!("{name}.a"));
            let alt = dst.join("build").join("llama.cpp").join("ggml").join("src").join(format!("{name}.a"));
            let from = if src.exists() { src } else { alt };
            if from.exists() {
                let _ = std::fs::copy(&from, compat.join(format!("lib{name}.a")));
            }
        }
        for sub in ["native-libs", "lib", "build", "build/llama.cpp/src", "build/llama.cpp/ggml/src", "build/llama.cpp/vendor/hash"] {
            let p = dst.join(sub);
            println!("cargo:rustc-link-search=native={}", p.display());
        }
        println!("cargo:rustc-link-lib=static=llama_shim");
        println!("cargo:rustc-link-lib=static=llama");
        // ggml 三个库互相依赖，用 group 包起来避免链接顺序问题
        println!("cargo:rustc-link-lib=static=ggml");
        println!("cargo:rustc-link-lib=static=ggml-base");
        println!("cargo:rustc-link-lib=static=ggml-cpu");
        println!("cargo:rustc-link-lib=static=vendor-hash");
        // GNU 工具链下 C++ 静态库需要显式链 libstdc++
        println!("cargo:rustc-link-lib=dylib=stdc++");
        println!("cargo:rerun-if-changed=native/llama_shim.cpp");
        println!("cargo:rerun-if-changed=native/CMakeLists.txt");
    }

    tauri_build::build()
}
