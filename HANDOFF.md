# DeepDark_Oracle 交接说明

## 当前快照

- 截止日期：2026-10-31。产品版本号仍为 `0.0.23`，仓库名 `AI--deepaisee`，应用名 `DeepDark_Oracle`。
- 当前 Git 工作区包含大量未提交的用户和开发改动，且部分本地工具、模型、llama.cpp checkout、构建产物受 `.gitignore` 忽略。接手前先运行 `git status --short` 并阅读相关文件；严禁使用 `git reset --hard`、`git clean -fd`、全盘清理或批量覆盖来“清理”工作区。
- 2026-10-31 的搜索不卡顿、数据目录恢复和相似度修复已经写入源码并通过测试；**最新安装包尚未重新生成**。不要把旧 `0.0.23` 安装包当作包含这些修改的版本发布。
- 项目是离线桌面磁盘检索工具，不自带或分发模型权重。用户自行导入 GGUF 或 safetensors；ONNX Runtime 和 Python sidecar 是可选扩展，安装包默认不带它们。

## 这几天的主要工作

### 产品与界面

- 首页以搜索为主：打字机标题、`DeepDark_Oracle` 品牌、黑白扁平图库结果；搜索后搜索区域收至上方，结果按相关度呈现。保留工作区范围、文件类别筛选、相似检索、排序、选择与导出。
- 主页移除了最近文件下拉框和搜索耗时毫秒数。结果首批显示 500 条，更多匹配项在后台静默分页获取；滚动时再逐批挂载卡片，继续保留全量结果和全选导出语义。
- 缩略图使用异步、低优先级和懒加载；PDF 缩略图通过可见性观察触发，渲染并发受限。结果卡片采用 `v-memo`，后台追加数据不会使已经显示的卡片全部重新绘制。
- 设置内容按文件索引、模型配置、局域网配置、可选扩展和日志分区；保留工作区记忆、目录树和树状图入口。可选扩展不改变原生模型路径。

### 扫描与索引

- 扫描根目录可多选，并按目录保存启用状态、文件数、体积、扫描错误、正文进度和向量进度；点击建立索引会先扫描并增量处理新增、变更或待重试项目。
- 索引支持取消、正文/向量分别补建、检查点保存与断点续传；取消后不应把尚未完成项计为已完成。扫描、解析、推理和持久化错误写入日志。
- `index.rs` 使用 Rayon CPU 预处理与单模型动态批处理，按文本/媒体 token 和 GPU 可用资源调整批次；节能/自动策略限制 CPU worker 数，不再依赖启动多个同权重模型实例。
- 已接入 PDF 正文和嵌入图片处理，扫描件没有文字层时不会把空文本送入嵌入模型；PDF、DOCX、PPTX 有分页/页码数据路径。其他格式的页码能力不应未经验证就对外承诺。
- 视频使用 FFmpeg 抽帧和音频解码；EmbeddingGemma2 相关默认参数为共享 8192-token context、独立图片 1120 tokens、PDF 内嵌图片 280、视频帧 140、视频 1 FPS，视觉/音频塔可配置关闭。实际模态能力仍取决于模型、projector 和所选推理后端。

### 模型与检索架构

- GGUF 由 `ai/llama.rs` 调用 `native/llama_shim.cpp` 和 llama.cpp；多模态 GGUF 需要可配对的 `mmproj`，经 mtmd 输入图像/音频/视频。只有主 GGUF 而没有兼容 projector 时，不应显示对应多模态能力。
- Safetensors 使用 vendored Candle/EmbeddingGemma 实现；模型导入逻辑解析配置、tokenizer、权重分片索引和模型文件，不要求用户手动指定 Candle 或 llama.cpp。
- 普通用户只选择 GPU 硬件；后端负责运行时适配。ONNX Runtime 不会直接读取 safetensors，需 ONNX 模型；Python sidecar 同样是可选扩展，不是核心依赖。
- 检索由 `search.rs` 融合文件名、路径、拼音、模糊匹配、SQLite FTS5/BM25、文本向量和可用的 CLIP/CLAP/共享多模态向量。EmbeddingGemma 的跨语言能力是多语言语义匹配，不是自动翻译；没有已编码的内容仍然搜不到。
- 相似度数值是模型向量余弦相似度/各检索通道的排序分数，不是“相关概率”。现在语义分数显示余弦原值；短于等于 2 个字符的语义查询最低阈值为 `0.62`，一般语义阈值来自配置（默认 `0.45`），最终列表另有 `0.40` 总过滤线。旧分数映射曾给弱命中加基线，因此像 `0.53` 看起来比实际更可信；单字“枪”的 `0.53` 语义项现在应被过滤。

### 2026-10-31 最近修复

- Tauri `search`、`search_semantic`、`similar` 命令改成异步入口，将同步数据库、向量和模型工作放到 `spawn_blocking`，避免检索长期占用前台 IPC/UI 调度线程。
- `SearchView.vue` 的后台结果累计改用非响应式数组和按类型分桶，只有当前视图需要的首 500 条进入响应式卡片列表；滚动再渲染后续卡片。后台分页仍会继续取回最多后端单次查询上限的结果，选择全部后导出会等待分页完成。
- `ResultGallery.vue` 和 `PdfThumbnail.vue` 降低缩略图加载优先级并推迟不可见 PDF 渲染。已移除左上角 ms 数显示。
- `config.rs` 会检查便携数据目录、`%LOCALAPPDATA%\DeepDark_Oracle\data` 和 `%APPDATA%` 中的 SQLite 文件是否有有效 `files` 表，并比较索引行与数据库/向量文件体积，减少升级后误选空数据库的情况。
- `vector.rs` 在打开向量库遇到维度不匹配或长度不匹配时不再删除磁盘文件。切换模型后按模型实际维度重新打开向量库，保护之前的向量数据。

## 代码入口

| 区域 | 文件 | 职责 |
| --- | --- | --- |
| 应用与 IPC | `src/App.vue`、`src-tauri/src/lib.rs`、`src-tauri/src/cmd.rs` | 页面路由、Tauri 命令注册与桌面 API |
| 搜索界面 | `src/views/SearchView.vue`、`src/components/ResultGallery.vue` | 搜索状态、分页、类别过滤、缩略图图库 |
| 扫描/索引 UI | `src/views/ScanView.vue`、`src/components/SettingsIndexTab.vue` | 目录、类型、索引进度与操作 |
| 模型 UI | `src/views/ModelsView.vue`、`src/components/SettingsModelTab.vue` | 导入模型、GPU、推理参数和模型能力 |
| 业务状态 | `src-tauri/src/state.rs`、`config.rs` | `AppState`、配置、运行时与数据路径 |
| 数据持久化 | `src-tauri/src/db.rs`、`vector.rs` | SQLite/FTS5、正文分页、索引状态、向量文件 |
| 扫描与索引 | `src-tauri/src/scan.rs`、`index.rs` | 目录扫描、增量队列、批处理、检查点 |
| 检索 | `src-tauri/src/search.rs`、`workspace.rs` | 混合排序、语义/跨模态、未索引工作区 |
| AI | `src-tauri/src/ai/`、`src-tauri/native/llama_shim.cpp` | GGUF、Candle、CLIP/CLAP/ORT 适配 |
| 内容提取 | `src-tauri/src/textract.rs`、`media.rs` | 文档/PDF 正文、页面媒体、缩略图 |

## 数据与兼容性

- 数据目录解析位于 `src-tauri/src/config.rs::data_dir/resolve_data_dir`。通常便携版使用 exe 同级 `data/`；安装在不可写目录时使用用户数据目录。现在会在便携、LOCALAPPDATA、APPDATA 候选中检查 SQLite 表与记录并按数据量择优。
- 这是兼容性启发式选择，不是完整迁移协议：当多个目录都含有效数据时，仍以目录质量和文件大小排序，没有核验数据库与全部向量是否属于同一索引快照。长期改进方向是固定稳定用户数据根目录，并做可审计、保留源文件的显式迁移。
- 主数据库为 `index.db`（WAL）；向量分别保存在 `vectors/`、`vectors_media/`、`vectors_clip/`、`vectors_audio/`。配置、日志、缩略图也在应用数据目录下。不同模型空间或维度不可混用。
- 应用启动时会先以轻量 HashEmbedder 打开基础向量空间；模型按需加载后按实际维度重开向量库。全文 FTS 与语义向量是独立能力；模型未加载或不可用时，全文仍可用，语义向量可能要等模型加载。
- 若旧版本已经把用户向量文件删除，代码无法重建原有语义向量；需要重新建立对应向量。升级后先备份整个 data 目录，再查看设置里的索引统计和日志，不要先删除索引或向量文件。

## 项目内依赖与构建

- Windows 项目内工具默认放 `.tools/`：Node、Cargo/Rustup、CMake、Ninja、下载缓存、可选 ONNX Runtime/CUDA 工具链。`.tools` 是本机目录，不提交 Git。
- `vendor/embeddinggemma-2-candle` 和 `vendor/pdf-extract` 是仓库内依赖源码；`vendor/llama.cpp` 由 bootstrap 获取，具体版本可能随本地 checkout/补丁变化。
- 首次准备工具（需要网络）：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/bootstrap-windows.ps1
. .\scripts\activate-local.ps1
```

- 在 VS Code 集成终端调试：打开仓库根目录，使用 PowerShell，运行 `. .\scripts\activate-local.ps1`，再运行 `npm run tauri dev`。脚本将 Cargo/Node 缓存和输出优先放进项目目录；MSVC/Windows SDK 与 NVIDIA 驱动仍是系统组件。
- 验证前端：`npm run build`。Rust 单元测试：`cargo test --manifest-path src-tauri/Cargo.toml --lib --features llama,candle,candle-cuda`。需要检查发布编译时可用 `cargo check --manifest-path src-tauri/Cargo.toml --all-targets`。
- 构建 NSIS 安装包：`npm run tauri:release` 或 `powershell -ExecutionPolicy Bypass -File scripts/build-release.ps1`。脚本自动检测 CUDA Toolkit；CUDA 构建需要兼容 Toolkit/nvcc 与 MSVC。输出通常位于 `src-tauri/target/release/bundle/nsis/`。**模型和索引不打进安装包；当前源码改动要重新构建后才会进入新安装包。**

## 本次验证与待办

- 2026-10-31：`npm run build` 通过。
- 2026-10-31：`cargo test --lib --features llama,candle,candle-cuda` 通过，45 项测试全部通过。
- 2026-10-31：`git diff --check` 通过。`cargo fmt -- --check` 未运行成功，因为当前 stable toolchain 未安装 `rustfmt`。
- 最新三项修复没有进行安装版端到端实机验证，也没有重新生成安装包；应在接手机上验证安装版搜索时滚动/缩略图响应、旧数据目录选择和单字语义过滤。
- 目录择优逻辑仍需更强的数据库与向量配对校验，并最终收敛到稳定数据目录/显式迁移。
- 前端会静默分页，但后端目前搜索候选和单次查询有 `50,000` 安全上限；“后台加载所有”在超过此上限时不代表真正无限结果。未来可改成数据库/向量层游标分页或更高效索引，而不是无界加载全部缩略图。
- GPU 利用率取决于模型后端、驱动、显存和批次形状，现有批处理优化不保证固定百分比或固定倍数；需要在目标机器用真实模型和不同类型数据实测。

## 安全注意事项

- 不要用 `git reset --hard`、`git clean -fd`、递归删除或批量永久删除来处理当前工作区。索引、模型、缓存需要清理时，用户要求的大批量删除全部改为移入回收站。
- 不要覆盖或重置现有 `data/`、模型目录、`.tools/` 或 target 输出；先确认真实路径和备份状态。
- 不要把 GGUF/safetensors 权重、索引数据库、CUDA Toolkit、`.tools` 开发依赖提交到仓库或安装包。
- 文档中的 v0.22/v0.23 安装包尺寸、哈希和验收数据是当时历史记录，不代表本次 2026-10-31 源码状态。发布前重新构建并记录新产物信息。


---

## v0.22 增量（2026-10-27）

### 改名

全局把 `deepaisee` 改成 `DeepDark_Oracle`：productName / identifier（`com.deepdarkoracle.desktop`）/ 窗口标题 / Cargo 包名与 lib 名（`deepdark_oracle_lib`）/ 启动器文件名 / 安装器与 NSIS 钩子 / 数据目录回退路径（`%LOCALAPPDATA%\DeepDark_Oracle`）/ 日志与 panic 文件名。

**有意保留旧名的地方**：`src/theme.ts` 里的 localStorage key `deepaisee.lang` / `deepaisee.theme`（改了会丢用户已保存的语言和主题偏好）；`DEEPAISEE_*` 系列构建期环境变量名（内部变量，不影响用户可见名称）；CHANGELOG 与历史文档里的旧名。

### 右上角「索引目录结构」面板

顶栏原来只读的索引统计按钮（`N 个文件 · X GB`）改成下拉触发器，点开从按钮正下方弹出 400px 面板，**不触发任何磁盘扫描**，只读索引库。

- 新组件 `src/components/IndexTreePanel.vue`：三段式（标题栏 + 搜索框 / 树主体 / 统计底栏），**按层懒加载**，展开哪层才查哪层，已加载层缓存在前端，收起不销毁。
- 视觉：白底 `#FFFFFF`、主文字 `#333333`、次要 `#999999`、边框 `#CCCCCC`、选中 `#F5F5F5`、搜索命中 `#E6F7FF`；行高 28px；暗色主题有独立变量。
- 交互：目录搜索（匹配目录名，命中自动逐级展开祖先并滚动定位）、右键菜单（打开文件位置 / 复制完整路径 / 复制文件名 / 加入工作区）、文件行右侧 6px 状态点（绿=已索引、灰=未索引、红=索引出错）、`Esc` 或点面板外收起、底栏「完整视图 →」跳到原来的 squarify 树状图页。

### 后端新增命令

| 命令 | 作用 |
| --- | --- |
| `tree_browse(path, file_limit)` | 返回某一层的目录与直属文件（`path` 为空时返回索引根列表） |
| `tree_search(query, limit)` | 按目录名在 `dir` 聚合结果里搜索 |
| `workspace_add(dir)` | 把一个目录加入工作区配置 |

数据层没有全量扫表：`db.rs` 新增 `dir_rollup_under(prefix)` 用 `dir >= lo AND dir < hi` 做字符串区间裁剪，并单独 OR 上「目录自身」的等值条件（否则会漏掉该目录的直属文件，因为 `D:\wx` 在排序上排在 `D:\wx\` 之前）。实测 24 万文件的库上全量聚合 140ms、单层展开 40~50ms。

### 顺手修掉的既有问题

1. **llama.cpp encoder ubatch 断言导致进程静默退出**：`native/llama_shim.cpp` 把 `cparams.n_batch/n_ubatch` 写死成 512，新版 llama.cpp 对 encoder 模型加了 `GGML_ASSERT(cparams.n_ubatch >= n_tokens)`，索引长文档时直接 abort（没有 panic 日志、没有报错弹窗，窗口直接消失）。改成按 `n_ctx` 取 `batch_cap`，并在 embed 时把超长 token 序列截到 `n_batch`。
2. **中文路径下 windres 打不开 `icon.ico`**：`tauri-build` 会把图标的绝对路径写进 `resource.rc`，MinGW `windres` 不能处理非 ASCII 路径 → 构建 panic。`build.rs` 里改成先把 `icon.ico` 复制到 ASCII 的 `OUT_DIR` 再用。
3. **`TreeView.vue` 的两个 bug**：缺 `import { t } from "../i18n"`；模板 `v-for="(t, i) in trail"` 遮蔽了 i18n 的 `t`，导致 `t("全部")` 把 TreeNode 当函数调用直接崩。
4. **`src-tauri/src/bin/` 下 5 个开发探针**：和 `examples/` 里的同名文件重复，但 `tauri-bundler` 会把 `src/bin/*.rs` 全部当 external binary 打进安装包。已删除 `src/bin/`，只保留 `examples/`。

### 构建与安装包

- 当前 v0.23 产物：`src-tauri/target/release/bundle/nsis/DeepDark_Oracle_0.0.23_x64-setup.exe`，498.90 MiB（523,129,853 bytes）；SHA-256：`FB3C98CFF3002D839C8F64D54D6B98BC4CC71349E54A84B4112BCA9170D60032`。该安装包包含 CUDA/MSVC 运行库，但不包含 GGUF、safetensors、CLIP/CLAP 权重或 `onnxruntime.dll`。
- 历史产物：`DeepDark_Oracle_0.0.22_x64-setup.exe`，682 MB（NSIS）。该旧包的内容记录仅供追溯；当前发布必须遵循 v0.23 约束，不捆绑模型权重或 ONNX Runtime。
- 本次使用项目目录环境和 MSVC x64 工具链构建，检测到 CUDA Toolkit 13.1，并将 CUDA/MSVC runtime 复制到发布目录。构建脚本仍会优先使用项目内 `.tools` 目录，避免把依赖写入 C 盘。
- 构建命令：`npm run tauri:release`（脚本自动选择 `llama,candle,candle-cuda,custom-protocol`）。
- 已验证：发布 exe `--selftest` 通过 6 项、失败 0、跳过 10；Safetensors 实机 CUDA 探针加载为 768 维并报告 text/image/video/audio，8 张图批处理约 1.51x 且最小余弦 0.999993；GGUF + 同目录 `mmproj` 探针加载为 768 维并报告四模态。Candle 与 llama.cpp 的批处理失败会递归拆分子批次，避免整批回退逐张。
