# 第三方组件与许可

本项目自身以 [MIT](LICENSE) 发布。下列第三方组件不随仓库分发（模型与二进制需自行获取），
但源码或构建过程中会用到它们，各自的许可以其上游为准。

## 一、编译期链接的库

| 组件 | 版本 | 许可 | 用途 | 来源 |
| --- | --- | --- | --- | --- |
| llama.cpp | 0.7.0-dev | MIT | 本地 GGUF 文本嵌入推理 | https://github.com/ggml-org/llama.cpp |
| ggml | （随 llama.cpp） | MIT | llama.cpp 的张量后端 | 同上 |
| ONNX Runtime | 1.20.1 | MIT | CLIP / CLAP 的 ONNX 推理（`onnxruntime.dll`） | https://github.com/microsoft/onnxruntime |
| WebView2 Runtime | — | 微软分发条款 | 界面渲染（系统自带） | https://developer.microsoft.com/microsoft-edge/webview2/ |
| MSYS2 / UCRT64 运行时 | gcc 16.2 | GPL + 运行时例外 | Windows GNU 工具链（`libstdc++-6.dll` 等） | https://www.msys2.org/ |

## 二、Rust crate 依赖

完整清单见 [`src-tauri/Cargo.lock`](src-tauri/Cargo.lock)。全部为宽松许可（MIT / Apache-2.0 / BSD）。
其中需要留意的两个：

- **`pdf-extract`** — 文本 PDF 解析（内部用 `lopdf`，MIT）
- **`rusqlite` (bundled)** — SQLite 以 bundled 方式静态编入，SQLite 属 **public domain**

## 三、模型权重（不随仓库分发）

| 模型 | 许可 | 用途 |
| --- | --- | --- |
| **EmbeddingGemma 300M**（GGUF 转换版） | Gemma Terms of Use | 文本语义嵌入 |
| **Chinese-CLIP ViT-B/16**（`Xenova/chinese-clip-vit-base-patch16` 的量化 ONNX） | Apache-2.0 | 中文图文对齐（文字找图片/视频） |
| **CLAP HTSAT-unfused**（`Xenova/clap-htsat-unfused` 的量化 ONNX） | Apache-2.0 | 音文对齐（文字找声音） |

模型权重版权归各自作者所有，使用前请阅读对应许可。本项目只调用推理，不修改权重。

## 四、外部工具（不随仓库分发）

| 工具 | 许可 | 用途 |
| --- | --- | --- |
| **FFmpeg 7.x** | LGPL-2.1+ / GPL-2.0+（取决于编译选项） | 视频抽帧、音频解码（`tools/bin/ffmpeg.exe`） |
| **Node.js 22** | MIT | 前端构建 |
| **Rust 工具链** | MIT / Apache-2.0 | 后端编译 |

FFmpeg 若使用带 GPL 组件的构建（如 `--enable-gpl`），分发时需遵守 GPL 条款。本项目通过
独立进程调用 ffmpeg（`std::process::Command`），不静态链接其库。
