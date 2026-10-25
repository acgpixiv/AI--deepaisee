# deepaisee 交接说明

## 交接范围

这是昨晚到今天对 `AI--deepaisee` 的集中改造结果。项目仍是 Tauri 2 + Rust 后端 + Vue 3/Vite 前端的本地离线磁盘检索工具。当前工作区有大量未提交改动，接手前不要使用 `git reset --hard` 或批量覆盖文件。

## 总体架构变化

### 后端

- Tauri 命令集中在 `src-tauri/src/cmd.rs`，应用状态集中在 `state.rs`，SQLite/FTS5 数据层在 `db.rs`。
- 检索层在 `search.rs`，现在融合文件名、路径、SQLite FTS5、拼音、模糊匹配、文本向量、CLIP/CLAP 跨模态向量。
- 向量空间分开保存：文本主空间、媒体兜底空间、CLIP 空间、CLAP 空间；Candle 多模态模型可将文本和媒体放入共享空间。
- 索引层 `index.rs` 改为批处理和检查点模式，支持 Rayon CPU 并行、模型分片/多实例、GPU 设备选择、取消、断点续传和持久化进度。
- SQLite 继续使用文件表 + FTS5 `docs` 表；新增索引根目录统计、启用/禁用目录、索引状态、错误日志、缩略图状态和重命名同步。

### AI 运行时

- GGUF 模型由 llama.cpp 适配器加载。
- safetensors 模型由 Candle/EmbeddingGemma Candle 适配器加载。
- 模型加载会根据目录内容和文件格式自动选择适配器，不再要求用户手工选择 llama/Candle。
- GPU 设备由后端枚举并按显存排序，运行时选择设备；CUDA/Vulkan/Candle 细节不暴露给普通用户。
- 支持模型并发策略：节能、自动、狂暴。后端按显存和安全余量决定模型副本数量，避免 16 GB 显卡超过约 15 GB 后继续分配。
- ONNX Runtime 和 Python sidecar 属于可选扩展，安装到项目目录 `.tools`，核心原生路径仍是 llama.cpp 和 Candle。

## 搜索功能变化

- 搜索请求不再受 200 条结果限制，单次请求上限为 10,000，并保留 `offset` 分页能力。
- 前端默认结果页调整到 1,000；旧配置中 `page_size=200` 会自动提升，避免旧配置继续限制结果。
- 结果改为全宽图库卡片，移除了右侧预览栏。
- 卡片支持图片、视频、PDF 预览；DOCX/PDF 缩略图可在索引时或首次需要时生成。
- 卡片悬停 1.5 秒后显示浮动预览；PDF 使用 WebView PDF viewer，隐藏工具栏和滚动条，底部显示页码信息。
- 双击卡片打开文件；右键菜单支持打开、重命名、资源管理器定位、复制路径。
- 重命名会同时更新磁盘文件、SQLite 文件表和 FTS 文件名；数据库更新失败会尝试回滚文件系统重命名。
- 相似度显示归一化到 `0.00-1.00`：高分绿色、中分橙黄色、低分深红色。相似度数字自身不会触发卡片选中或双击打开。
- EmbeddingGemma 的跨语言语义检索直接使用同一向量空间，不再使用搜索框翻译设置。真正的文本翻译仍需要 chat GGUF、NLLB 或 Marian 等生成模型。

## 索引和扫描变化

- 建立索引前会重新扫描启用的目录，检查新增、更新、缺失文件。
- 扫描目录可以多选；每个目录有独立启用/禁用状态和独立统计信息。
- 文件类型改为黑白文字标签，包含图片、视频、音频、PDF、文档、压缩包、其他。
- “建立索引”“取消”“仅补正文”“仅补向量”集中在索引进度区域。
- 取消会清空待处理队列，停止当前模型任务的后续工作，并保存已经完成的向量和正文检查点。
- 取消后前端立即退出“处理中”状态，进度条冻结在取消瞬间，避免后台延迟提交造成百分比突然上涨。
- 下次重新建立索引时只处理仍为 pending/fail 的项目，已落盘项目可以断点续传。
- PDF 支持正文和嵌入图像提取；扫描型 PDF 没有可用文字时不会把空内容重复喂给模型，图片仍可单独尝试编码。
- PDF 提取、批次开始/结束、模型失败、检查点失败等错误会写入日志页和数据库活动日志。
- 当前处理文件列表会显示同一批次中多个文件，便于确认是否真的并发执行。

## 界面变化

- 主页是搜索图库模式，搜索前显示打字机标题和三维旋转上浮的半透明黑灰三角形背景。
- 主页标题上方加入黑字白底 `deepaisee` 品牌名。
- 已移除主页“本地离线 · 文件与内容”文案。
- 设置页采用文件索引、模型配置、局域网配置、可选扩展、日志五个区域。
- 设置页再次点击右上角设置按钮会返回主页；设置页左上角搜索按钮也能返回主页。
- 已移除设置页“本地离线 · 数据不出机器”文案。
- 通用设置模块已从界面隐藏/移除，不再显示多语言选项。
- 树状图保留 squarify 面积布局，并增加父目录内部子目录方框和文件类型统计卡片。

## 主要文件

- 前端入口：`src/App.vue`
- 搜索页：`src/views/SearchView.vue`
- 搜索结果卡片：`src/components/ResultGallery.vue`
- 扫描和索引页：`src/views/ScanView.vue`
- 设置容器：`src/components/SettingsDrawer.vue`
- 模型设置：`src/views/ModelsView.vue`
- 树状图：`src/views/TreeView.vue`、`src/components/Treemap.vue`
- Tauri 命令：`src-tauri/src/cmd.rs`
- 应用状态：`src-tauri/src/state.rs`
- 数据库：`src-tauri/src/db.rs`
- 搜索融合：`src-tauri/src/search.rs`
- 索引器：`src-tauri/src/index.rs`
- 模型适配：`src-tauri/src/ai/mod.rs`、`llama.rs`、`candle_backend.rs`
- 模型扫描/导入：`src-tauri/src/models.rs`
- 文档和 PDF 抽取：`src-tauri/src/textract.rs`
- 媒体缩略图：`src-tauri/src/media.rs`

## 依赖和项目内目录

- `.tools`：项目本地 Node、Cargo、Rustup、CMake、Ninja、ONNX Runtime、CUDA 运行库和下载缓存。
- `vendor/embeddinggemma-2-candle`：EmbeddingGemma Candle 源码。
- `vendor/pdf-extract`：本地 PDF 抽取依赖。
- `scripts/bootstrap-windows.ps1`：准备项目内开发工具和依赖。
- `scripts/activate-local.ps1`：激活项目内工具链环境。
- `scripts/build-release.ps1`：自动检测 CUDA 并执行发布构建。

## 构建和验证

开发构建：

```powershell
cd E:\项目\AI--deepaisee
npm run build
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
```

完整发布构建：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build-release.ps1
```

无 CUDA 的轻量构建：

```powershell
npm run tauri build -- --features llama,candle,custom-protocol
```

当前已验证：

- `npm run build` 通过。
- `cargo check --all-targets` 通过。
- `cargo check --no-default-features --all-targets` 通过。
- `cargo test --lib` 通过，25 项测试全部成功。
- `git diff --check` 没有代码空白错误，只有换行格式提示。

## 发布包说明

- Tauri 当前配置目标为 NSIS，输出目录是 `src-tauri/target/release/bundle/nsis/`。
- 当前已经生成过 `deepaisee_0.0.20_x64-setup.exe`，约 526 MB。
- 这个单个 EXE 是安装包，安装时会释放程序文件和运行库；模型、索引数据库不应打进安装包。
- CUDA 版体积主要来自 CUDA 运行库；没有 CUDA 时可使用轻量构建命令。
- WebView2、显卡驱动和系统 MSVC 运行环境仍属于目标机器的外部前置条件。

## 已知限制和后续重点

- 当前“跨语言”是多语言嵌入语义匹配，不是句子翻译；全文精确匹配仍然依赖原文、文件名、拼音和词条。
- PDF 页码总数使用前端 PDF 内容扫描估算，受加密、压缩和 WebView 访问权限影响，少数文件可能显示 `?`。
- PDF 卡片和浮窗使用系统 WebView PDF viewer，详细查看仍应双击打开系统默认程序。
- 现有部分源码历史上存在中文编码显示异常，接手时不要做全项目编码重写，避免引入大范围无关 diff。
- 模型和向量空间切换后需要按日志提示重新建立对应向量；不要直接混用不同维度的向量文件。
- 当前工作区不是干净 Git 状态，所有改动尚未形成正式提交。
- `deepaisee.exe` 在工作区状态中出现过二进制删除/大小变为 0 的记录，交接人提交前应单独确认这是否为预期，不能盲目提交。

## 安全注意事项

- 不要使用 `git reset --hard`、`git clean -fd` 或批量永久删除命令。
- 删除索引数据、模型或缓存前，优先移动到回收站或由用户明确确认。
- 不要把模型、索引数据库、CUDA Toolkit 和开发工具一起提交到 Git。
