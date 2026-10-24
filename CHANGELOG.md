# deepaisee 版本说明（CHANGELOG）

版本号规则：对外显示 `0.01` 起、每次改动 +0.01；内部语义化版本同步为 `0.0.N`。
本项目由 DiskSeek（Python + PySide6）重构而来，见文末「从 DiskSeek 到 deepaisee」。

---

## v0.20 — 2026-10-24

**跨模态检索：用一句话找图片 / 音频 / 视频 + 同模态找相似**

### 一、为什么单独接两个 ONNX 模型

v0.17 试过用 EmbeddingGemma 2 的视觉塔：mtmd 能导出、能编码、向量也是 512 维，但视觉塔与文本塔**不在同一语义空间**（实测图文余弦 0.03~0.09），真正做对齐的 `embed_vision.embedding_projection` 层没被 `convert_hf_to_gguf.py` 导出。所以「文字找图」在 GGUF 路线上走不通。

本版改为：**ONNX Runtime + 两个专门做跨模态对齐的量化模型**，与主嵌入模型完全解耦。

| 模型 | 文件 | 体积 | 输入 | 输出 |
| --- | --- | --- | --- | --- |
| Chinese-CLIP ViT-B/16（Xenova，U8 量化） | `models\clip\model.onnx` + `vocab.txt` | 182MB | 图片 `1×3×224×224` / BERT 文本 `1×64` | 512 维 |
| CLAP HTSAT（Xenova，U8 量化） | `models\clap\audio_model.onnx` + `text_model.onnx` + `vocab.json` + `merges.txt` | 154MB | 音频 `1×1×1001×64` log-mel / RoBERTa BPE 文本 | 512 维 |

### 二、自写 ONNX Runtime 绑定（`src-tauri/src/ai/ort.rs`，约 380 行）

不引 `ort` crate（rsproxy 上没有），直接 `LoadLibraryW("onnxruntime.dll")` + `OrtGetApiBase()->GetApi(20)` 拿函数表，用 `transmute` 索引字段。踩过的坑：

- `OrtApiBase.get_api` 拿到的函数指针必须先 `(*base).get_api` **解引用**再调用，否则报 `E0599: no method named get_api`；
- 字段索引错一位就是崩溃或乱值，本版实测可用的一组：`CREATE_ENV=3` / `CREATE_SESSION=7` / `RUN=9` / `CREATE_SESSION_OPTIONS=10` / `SET_GRAPH_OPT=23` / `SET_INTRA_THREADS=24` / `CREATE_TENSOR_WITH_DATA=49` / `GET_TENSOR_DATA=51` / `CREATE_CPU_MEM=69` / `RELEASE_SESSION=95`；
- ONNX Runtime 1.20.1 只支持到 **API v20**，传 23 直接返回空指针。

实测性能：session 创建 561ms，单次前向 27~35ms（8 线程），图片与文本同价。

### 三、CLIP：文字 ↔ 画面

- 预处理按 CLIP 标准：短边缩到 224 → 中心裁剪 → CHW → `mean=[0.48145466,0.4578275,0.40821073]`、`std=[0.26862954,0.26130258,0.27577711]`。
- 分词是极简 BERT WordPiece（中文按字、英文贪心最长匹配，`[CLS]`/`[SEP]` 包裹，64 token）。
- **量化模型质量实测可用**：4 张真实照片（峡湾/黑狗/海象/狮子）对 4 句话，4×4 矩阵对角全胜，正确对 0.43~0.47、错配全 < 0.35。曾担心 U8 量化会毁掉语义（tract 跑同一模型时纯色图排序完全随机），换 ORT 后正常。
- 视频取**首帧**进同一个 CLIP 空间。

### 四、CLAP：文字 ↔ 声音

这里踩了三个大坑，都是「看起来能跑、结果全是噪声」：

1. **分词器完全错**：CLAP 文本塔是 RoBERTa，不是 BERT。用 BERT 词表编出来的 token 全错。补了 `src-tauri/src/ai/bpe.rs`（GPT-2 字节映射 + merges 里的 BPE merge）。修好后 Rust 编出的 token 与 Python transformers 逐 token 一致。
2. **音频 mel 归一化错**：一开始用 HTK mel + `log10`，4 段完全不同的音频两两余弦 **0.9986~0.9996**，等于没有区分度；换成 **Slaney 刻度 + Slaney 归一化三角滤波 + `10*log10(max(1e-10,x))`**（照 transformers.js 的 `ClapFeatureExtractor` 实现）后降到 **0.23~0.66**。
3. **中文直接喂 CLAP 不可靠**：该模型英文语料训练，纯中文 prompt 命中 13/36。加了一张 40 条的「中文音效词 → 英文提示」映射表（`clap.rs::prompt_for`）后命中 **13/15**。

预处理细节（全部自己实现，无第三方音频库）：48kHz 单声道 → repeatpad 到 480000 采样 → reflect 填充 512 → Hann(1024, periodic) → `rfft` → 513 bin → 64 个 Slaney 三角滤波 → `10*log10` → `[1001, 64]`。

### 五、检索与打分

- `SearchQuery` 新增 `cross_modal: Option<bool>` 与 `clip_query` / `audio_query` 两个 `#[serde(skip)]` 预编码向量。
- `AppState::search` 在 `cross_modal == Some(true)` 且查询 ≥ 2 字时，现场用 CLIP/CLAP 文本塔编出查询向量。
- 新增 `search.rs::cross_modal_segment`：CLIP 余弦 < 0.25 丢弃、`0.25→0.55` ~ `0.45→0.90` 线性；CLAP 余弦 < 0.16 丢弃、`0.16→0.52` ~ `0.46→0.88` 线性。**只加分不降分**，不会把已经由文件名命中的结果拉低。
- **两个空间互相串味**：只靠绝对阈值调不好 —— CLIP 对不相干描述天生给的余弦高（随便配都 0.30），CLAP 只有 0.13，两边永远此消彼长。改为按「突出版」仲裁：`peak = top1 - 中位数`，抬升不足 **0.05** 就认为这个空间对这句话无话可说，整段丢弃；再加上意图分流 —— 查询里含声音词表（`clap.rs::sound_intent`）时 CLAP 说了算、CLIP 让路，纯画面描述里 CLAP 要突出到 CLIP 的 2 倍才准插话。实测「钢琴」不再带出图片、「一只黑色的狗」不再带出音频。
- `similar()` 升级为按源文件类型挑空间：图片/视频 → CLIP 优先，音频 → CLAP 优先，文本 → 主向量库优先，找不到再跨空间兜底。
- 四个向量空间：`vectors\`（768 维）、`vectors_media\`（256 维兜底）、`vectors_clip\`（512）、`vectors_audio\`（512）。

### 六、真机实测（本地测试目录，6 图 + 6 音频 + 2 视频）

| 查询 | 结果 |
| --- | --- |
| 「一只黑色的狗」 | p2.jpg **0.90**（其余 0.65~0.76） |
| 「钢琴」 | piano.wav **0.88** |
| 「下雨的声音」 | rain.wav **0.63** |
| 「打鼓」 | drum.wav **0.79** |
| 「鸟叫」 | birds.wav **0.72** |
| 「弹吉他」 | guitar.wav **0.85** |
| 「一只黑色的狗」（含视频） | clip_dog.mp4 **0.90** |
| 「峡湾和湖泊」 | clip_fjord.mp4 **0.87** |
| 图片 p1 找相似 | p6.jpg 0.71、p4 0.59、p3 0.56 |

加入突出度仲裁后复测（`_xtest` 库，6 图 + 6 音频 + 2 视频）：`钢琴` → piano.wav 0.88（前 6 条全是音频）、`一只黑色的狗` → p2.jpg 与 clip_dog.mp4 并列 0.90（前 6 条全是图片/视频）、`峡湾和湖泊` → p1.jpg 0.87 / clip_fjord.mp4 0.87（无音频）、`下雨的声音` → rain.wav 0.62（无图片）。`--selftest` 仍是 `[OK] 文字找图 — 「一只黑色的狗」→ p2.jpg (0.90)`、`[OK] 文字找声音 — 「钢琴」→ piano.wav (0.85)`。

`--selftest` 新增两项（文字找图 / 文字找声音），全绿时 **通过 14 项，失败 0 项**。

### 七、顺手修掉的三个真问题

1. **`%TEMP%` 不可写导致视频抽帧全部静默失败**：`media.rs` 原来直接用 `std::env::temp_dir()`，本机在该目录下建子目录失败，视频向量 0 条、用户只看到「视频搜不到」却没有任何提示。改为依次尝试 `exe/data/tmp`、`cwd/data/tmp`、`%TEMP%/deepaisee`，并在抽帧为空时把原因写进 `last_error`（「视频 xxx: 抽帧失败（需要 ffmpeg）」）。顺带清理历史 `frames-*` 残留目录。
2. **`selftest` 的「画面标签检索」示例标签是空串**：`tags` 里第一项可能是「图片」「视频」这类通名，被跳过之后没有兜底。改为按 `length(tags) DESC` 取标签最多的那条文件，取不到有辨识度的标签就 `[SKIP]` 而不是显示「示例「」」。
3. **`search.rs` 被 `Set-Content -NoNewline` 压成一行**（编辑事故）：`//` 注释把后面整段代码注释掉。已手写重写回 636 行并编译通过 —— 教训是别再对含注释的源码用 `-NoNewline` 拼接。

### 八、轻量化

- 新增的 `bpe.rs` / `ort.rs` / `clip.rs` / `clap.rs` 全部自写，**没有新增任何 crate 依赖**（Cargo.toml 未变），音频 FFT / mel / BPE / ORT 绑定都是手写。
- 代码净增约 1200 行，但外部依赖与构建产物不增（onnxruntime.dll 13.5MB 随包）。
- 临时目录 `_clip`（1.6GB 下载与探针）、`_xtest` / `_mmtest` / `_shot`（测试库、探针与截图）、废弃的 `src-tauri\target`（1.4GB，旧 ACL 已不可用）与 `build-target\debug`（3.7GB）在验收后清理，**项目体积 14.0GB → 6.2GB**；`dist-app\crossprobe.exe` 探针也一并删掉。

---


**模型管理页 + 全文/拼音搜索增强 + 轻量化**

### 一、模型管理页（新增）

顶栏多了一个「模型」模式，模型不再需要手改 `config.json`：

- 扫描 `<exe>\models\`（不存在则 `data\models\`）里的所有 `.gguf`，显示大小、修改时间、类型、是否为当前模型
- 读取 GGUF 头里的 `general.name` 作为展示名，读不到才退回文件名
- 按文件名自动归类：`mmproj`/`projector`/`vision` → 视觉投影，`embed`/`bge`/`gte`/`minilm` → 嵌入，`chat`/`instruct`/`qwen`/`llama-3` → 对话
- 一键「设为嵌入模型」即时加载并刷新状态栏；「切回哈希基线」不需要任何模型文件
- 「导入模型」从任意位置复制进来（重名自动加序号）；「删除」有保护：**当前正在使用的模型不允许删**
- 后端新增 `model_list` / `model_dir_path` / `model_delete` / `model_import` 四个命令，`src-tauri/src/models.rs` 是独立实现

### 二、全文搜索：Word / Excel / PDF / TXT 全部支持

修了一个让全文搜索形同虚设的 bug：`index.rs` 里判断「要不要抽正文」写的是 `matches!(kind, "text" | "doc")`，而 `ext_kind()` 返回的是 `"document"`，导致 **所有 PDF/DOCX/XLSX 都被跳过**，全文索引里只有 `.txt`。改为 `"text" | "document"` 后文档类才真正入库。

`textract.rs` 整体重写（216 → 约 700 行），格式覆盖：

| 类别 | 扩展名 | 实现 |
| --- | --- | --- |
| 纯文本 | txt md log csv tsv json yaml html css js ts vue … (50 种) | `decode_bytes` 依次试 BOM / UTF-8 / GB18030 |
| Word | docx、**doc** | docx 解 XML；doc 自写 OLE2 + Word 97 分片表（PlcPcd，含 fc 压缩位判断 UTF-16/单字节） |
| Excel | xlsx、**xls** | xlsx 读 sharedStrings + 各 sheet；xls 解 BIFF8：SST(0x00FC) / LABELSST(0x00FD) / NUMBER(0x0203) / RK(0x027E) |
| PPT | pptx、**ppt** | pptx 取各页 `a:t`；ppt 解 OLE2 记录树里的 TextCharsAtom(0x0FA0) / TextBytesAtom(0x0FA8) |
| PDF | pdf | `pdf-extract` |
| RTF | rtf | 自写控制字解析：`\uN`、`\'hh`（双字节按 GBK 组字）、`\par`/`\line` → 换行 |
| EPUB / ODF | epub、odt/ods/odp | EPUB 解 zip 内 xhtml 去标签；ODF 读 `content.xml` |

**没有引入新依赖**：老式 Office 的 OLE2 容器解析是自己写的（约 120 行），比引入 `cfb` + `fnv` + `uuid` 更符合「轻量化」。单元测试 `plain_exts_decode` / `rtf_strips_controls` 随代码一起提交。

### 三、全文命中片段（高亮）

原先 `Hit.snippet` 字段存在但后端从不填，全文命中只有一个分数、看不到命中在哪里。现在：

- `search.rs` 记录哪些 id 是全文命中的，映射结果时调 `make_snippet(raw, text, 60)` 从 `docs.raw` 里截前后各 60 字上下文，两端补 `…`
- 用整串小写查找；查不到时退回「用查询里的 2-gram 去 raw 里找最早出现的位置」，保证中文长查询也能定位
- 前端 `ResultTable.vue` 用 `<mark>` 高亮，两行截断

### 四、搜索页的三个开关

搜索框下方新增三个 toggle，直接决定发给后端的 `use_fts` / `use_pinyin` / `use_semantic`：

- **全文**：在 Word / Excel / PDF / TXT 等文档正文里检索
- **拼音**：`huiyijilu` / `hyjl` 都能找到「会议记录」
- **语义**：用一句话描述找相近的文件

### 五、修掉的其它 bug

- **筛选与全文冲突**：筛「图片」时全文命中的 PDF 也会冒出来。现在带结构化筛选时，全文命中必须在候选集内才计入
- **原因标签被覆盖**：文件名精确命中的结果，原因曾被改写成「全文内容匹配」。现在只升级不降级
- **`doc` 类型键不一致**：前端 `KIND_COLORS` / `Treemap.vue` 图例用的键是 `doc`，后端 `ext_kind()` 返回 `document`，导致文档类型永远显示成默认灰色。已统一为 `document`

### 六、轻量化

删除的文件（合计约 1.6 GB 冗余 + 19.8 GB 可重建缓存）：

| 对象 | 大小 | 判断依据 |
| --- | --- | --- |
| `models\embeddinggemma2-mm.gguf` | 532 MB | SHA256 与 `embeddinggemma2.gguf` 完全相同 |
| `models\mmproj-embeddinggemma2.gguf` | 937 MB | 全仓库 grep `mmproj`/`clip`/`multimodal` 零命中 |
| `dist-app2\` | 695 MB | 旧绿色发布版，README 只引用 `dist-app` |
| `tmp\` | 32 MB | 测试残留 |
| `vendor\llama.tar.gz` | 37 MB | `vendor/llama.cpp` 已解包 |
| `tools\msys2-…exe` / `node-…zip` / `rustup-init.exe` | 126 MB | 安装包，工具链已装好 |
| `tools\patch_*.py` / `fix_*.py`（14 个） | 30 KB | 开发期一次性补丁脚本，改动早已落到源码 |
| `acl-recovery\` | 0.2 MB | 本轮 ACL 修复的备份与报告 |

代码精简：

- `fileops.rs` 297 → 95 行：删掉 `copy_paths` / `move_paths` / `delete_paths` / `rename_paths` / `rename_pattern` / `unique_target` / `retry` / `OpResult` —— 这些只被前端从不调用的 `op_copy` / `op_move` / `op_delete` / `op_rename` / `op_rename_pattern` 使用
- `cmd.rs` 删掉 11 个零引用命令：`facets` / `index_wait` / `op_copy` / `op_move` / `op_delete` / `op_rename` / `op_rename_pattern` / `categories_list` / `log_add` / `embed_preview` / `now_label`
- `db.rs` 删掉 `Category` 结构与 `categories` 表及其 CRUD（只服务于已删的 `categories_list`）
- `api.ts` 262 → 224 行：同步删掉对应的 8 个方法与 `OpResult` 接口；`ModelEntry` 去掉从未使用的 `arch` / `quant` / `referenced`
- `models.rs` 删掉 `gguf_meta` 里读 `general.size_label` 的分支、`now()`；`ModelsView.vue` 删掉本地重复的 `humanSize`（改用 `store.ts` 的）
- `Cargo.toml` 去掉 `trash` 依赖（唯一使用者 `delete_paths` 已删除）
- `src-tauri/src/bin/selftest.rs` 删除：与 `src/selftest.rs` 是两份重复的自检实现，`main.rs` 只走后者

**未删**：`dist-app\`（唯一发布版）、`tools\bin\ffmpeg.exe`（`media.rs` 会找）、`src-tauri\src\bin\smoke.rs`（README 引用的端到端验证）、`visdemo.rs`（视觉标签调试）。

### 七、真机实测后追加修复

打完包用 8 个真文档（docx / xlsx / pdf / txt / rtf / pptx / odt / 名字带 `.doc` 的纯文本）做端到端验收，又暴露了几个问题：

- **RTF 抽不出正文**：`textract.rs` 里 RTF 只有在「纯文本分支落空」时才走解析，而 `.rtf` 又在 `PLAIN_EXTS` 里，于是被 `read_plain` 按 latin1 读成花括号控制字、正文全丢。现在 RTF 从纯文本组移出，直接走 `rtf_text`
- **删掉的文件还能从全文里搜出来**：`db.rs::search_fts` 只查 `docs` 虚表，已被标记 `missing`/删除的文件依旧返回 id。SQL 里加了 `JOIN files ON files.id = docs.rowid WHERE files.missing = 0`，删文件时也同步删 `docs` 行
- **`docs_map` 映射串号**：FTS5 虚表的 `last_insert_rowid()` 在批量插入 / id 复用时会错位，表现为「搜到 A 文件、片段却是 B 的内容」。直接删表，改成 **`docs.rowid` 恒等于 `files.id`**，`FTS_SCHEMA_VERSION` 升到 3 并在启动时自动重建索引
- **OOXML 段落没换行**：`w:p` / `a:p` 之间的换行原来被吃掉了，整篇 docx 抽出来是一长串。现在段尾补 `\n`
- **`--selftest` 空库误报**：新装用户库里没文件时，全文/拼音/文件名/语义四项必然 0 条，原来记 FAIL，现在改成 `[SKIP]` 行，结尾单独统计「跳过 N 项」，不再混进成功数里冒充满绿
- **`launcher.ps1` 丢了 BOM**：本轮编辑把 UTF-8 BOM 弄掉了，Windows PowerShell 5.1 会按 GBK 解析中文，启动器菜单直接报解析错误。已按 UTF-8 with BOM + CRLF 重新保存并实测 0 解析错误

### 八、二次轻量化（构建产物）

- 删除 `src-tauri\target\`：16 GB 旧构建缓存（含一个起不来的 test harness 和 3 个 libllama.a 副本）
- 删除 `build-target\`：7.6 GB（本轮为绕开旧 target 的 ACL 问题另建的 CARGO_TARGET_DIR，源码与 Cargo.lock 都在，随时 `cargo build` 重建）
- 删除 `_fttest\`（端到端测试用的临时 app/文档/脚本）、`acl-recovery\`（ACL 修复备份，已无用）
- 删除 `src-tauri\Cargo.lock.bak`、`Cargo.lock.test`、`src-tauri\data\smoke\`（smoke 测试产物）
- 删掉 `dist-app\data\` 里的测试索引（`index.db` 64MB + `vectors*` + `thumbs`），恢复出厂态：目录为空、`scan.roots` 清空，用户第一次打开只需挑自己的目录
- 验证截图归档到 `docs\screenshots\`

**项目体积：约 32.4 GB → 4.0 GB**（其中 `tools\` 2.6 GB 是编译工具链，`dist-app\` 632 MB 含模型 533MB + ffmpeg 84MB，`vendor\llama.cpp` 182 MB）

---

## v0.18 — 2026-10-22

**带菜单的启动器 + 清理旧项目**

### 启动器（新增）

`dist-app\deepaisee 启动器.cmd` + `dist-app\launcher.ps1`，六个入口：

- `[1]` 正常启动：起主界面，并在启动前回显上一轮模型加载耗时（读 `data\autoload.log` 末三行）
- `[2]` 自检：跑 `--selftest`，按行着绿/着红回显 12 项结果
- `[3]` 重建索引：`--cli index --what auto --force --wait`，执行前二次确认
- `[4]` 扫描目录：交互输入路径，校验存在性后 `--cli scan`
- `[5]` 命令行：`--cli help`
- `[6]` 打开数据目录：Explorer 定位到 `data\`

实现要点：

- **批处理文件里不放中文**：`.cmd` 是纯 ASCII 引导层（0 个非 ASCII 字节），所有中文文案在 `launcher.ps1`，由 PowerShell 渲染。
- **`launcher.ps1` 必须 UTF-8 with BOM + CRLF**：Windows PowerShell 5.1 读无 BOM 的 `.ps1` 会按 GBK 解析，中文注释直接让脚本报「字符串缺少终止符」。已用 `[Parser]::ParseFile` 在 PS 5.1 与 pwsh 7 上分别校验 0 错误。
- **`[Console]::IsInputRedirected` 时跳过 `ReadKey`**：否则在管道/自动化场景下 `ReadKey` 会卡死，无法做回归测试。
- 顶部版本号从 exe 的 `ProductVersion`（`0.0.18`）裁成 `0.18`，与界面显示一致。

### 清理

- 删除旧项目 `D:\软件\123`（DiskSeek，Python + PySide6）：释放 **10.5 GB**（`.venv` 5.4G / `dist` 4.4G / `build` 480M / `data` 201M + 源码）。
- 目录内容已全部清空并核实（`ls -A` 为 0 项）；空目录本身残留一个内核句柄（`Device or resource busy`，RestartManager 也查不到用户态进程），重启后即可删除，不影响任何功能。
- 删除前已确认：`tools\bin\ffmpeg.exe`、`models\embeddinggemma2.gguf` 在新项目里都有副本，且 `grep -rn "软件/123"` 在 `src-tauri/src`/`src`/`tools` 下零命中。

### 验证

- 启动器在 PS 5.1 实跑：菜单渲染正常、`[2]` 自检 12 项全绿、`[4]` 扫描 1 文件入库成功、`[5]` 帮助正常、`[1]` 起进程正常（425MB 常驻）。
- `deepaisee.exe --selftest` 直接跑：通过 12 项，失败 0 项，`llama.cpp · 768 维`。

---

## v0.17 — 2026-10-22

**补回「用文字找图片」：视觉标签 + mtmd 路线失败的完整记录**

重构到 llama.cpp 之后丢了一项原始需求（m00002）：「可以通过文字描述查找图片」。这一版先花大力气尝试用 llama.cpp 的多模态投影（mtmd）把图片编码进向量空间，失败了；随后换了一条可解释的路线补回来。

### 尝试过但放弃的路线（mtmd）

- 用 `convert_hf_to_gguf.py --mmproj` 成功导出视觉塔：`mmproj-embeddinggemma2.gguf`（963 张量 / 982MB，含 `v.blk.0..9.*`、`v.patch_embd.weight {16,16,3,768}`）。
- 用独立探针工程（`tools/mtmd-probe/`，CMake 直连 `vendor/llama.cpp`）验证：图片 tokenize 成 130~1000 个 image token、`mtmd_encode_chunk` 成功、视觉向量 512 维且**不同图片确实不同**（head 前几维差异明显）。
- 卡在向量空间对齐：
  1. 直接用 `llama_get_embeddings_seq` 的 768 维 pooled 向量 → 三张不同图几乎相同（范数 3453.20 / 3453.76，head 差异仅 1e-4），证明池化只吃了文本前缀。
  2. 用文本 GGUF 的 `output.weight [512,768]` 把 512 投到 768 → 余弦 −0.19 / −0.19 / −0.17，全是负数且几乎相同，说明那是词表投影，不是视觉→文本的共享投影。
  3. 绕开池化，比「文本塔最后一层 hidden 512 维均值」vs「视觉塔 512 维均值」→ 0.03~0.09，四种描述、两张图全部无区分度。
- 根因：HF 版靠 `embed_vision.embedding_projection.weight [512,768]` + 对比学习把两个塔对齐，而 `convert_hf_to_gguf.py` 没有导出这一层，mtmd 的 clip 图里也没有它。补齐需要改转换脚本 + 改 clip 计算图 + 重新训练对齐，风险与工作量都超出范围。
- **结论：回退主构建的 `LLAMA_BUILD_MTMD`，保留探针工程与 mmproj 文件作为证据。**

### 实际采用的方案：可解释的视觉标签

新增 `src-tauri/src/vision.rs`（约 330 行）：

- `analyze()` 把图缩到 64×64 网格采样，算出这些指标：平均色相（用 `cos/sin` 加权平均避免 0/360 断裂）、饱和度、亮度、上下半区亮度、上下半区蓝/暖色占比、蓝/绿/暖/白/黑/灰各色系占比、边缘密度。
- `tags_of()` 把指标翻译成**用户真的会输入的中文词**：蓝天、蓝天白云、白云、天空、海洋、水、湖、倒影、绿色、森林、树木、草地、草原、自然、户外、风景、雪景、冰雪、雪山、夕阳、黄昏、晚霞、日落、夜景、暖色、红色、白色、黑色、灰色、黑白、单色、纯色、背景图、壁纸、截图、文档、照片、色彩鲜艳。
- `describe_file()` 直接读图出标签；索引时**优先分析已生成的缩略图**（几百像素，快且稳，视频也能复用中间帧）。

接入点：

- `src-tauri/src/index.rs` 的 `process_one` 在缩略图之后（2.5 步）写 `files.tags`；只对 `image` / `video` 做，且已有标签不重复算。
- `src-tauri/src/search.rs` 的名称打分段新增「画面标签」匹配：命中一个标签 0.62 起，命中越多越高，上限 0.82；低于文件名精确匹配（1.0），高于路径匹配（0.5~0.65）。
- `Hit` 与 `src/api.ts` 增加 `tags` 字段；结果表在文件名后显示前 3 个标签（青色小胶囊），预览面板新增「画面标签」区块列出全部。

实测（临时目录 `ds-test`，13 张图 + 视频）：

| 图片 | 自动标签 |
| --- | --- |
| `forest.png` | 图片 / 壁纸 / 户外 / 纯色 / 绿色 / 背景图 / 自然 / 草地 |
| `ocean.png` | 图片 / 壁纸 / 纯色 / 背景图 / 蓝色 |
| `sunset.png` | 图片 / 壁纸 / 暖色 / 红色 / 纯色 / 背景图 |
| `img102.jpg`（日落雪山湖） | 图片 / 夕阳 / 晚霞 / 暖色 / 黄昏 |
| `img100.jpg` | 倒影 / 图片 / 壁纸 / 天空 / 湖 / 纯色 / 背景图 / 蓝色 |
| `img105.jpg`（纯蓝壁纸） | 图片 / 壁纸 / 纯色 / 背景图 / 蓝色 |

搜索验证：

```
search 夕阳   → img102.jpg 0.66（画面里确实是落日晚霞）
search 纯色   → img101.jpg 0.66、ocean.png 0.66
search 壁纸   → img105.jpg 0.66、ocean.png 0.66
search 草地   → forest.png 0.64
search 蓝色   → img104.jpg 0.72、img0_1920x1200.jpg 0.67
```

### 能力边界（务必如实告知）

- 这套标签是**颜色/构图统计**，不是语义理解。它能回答「有没有大片蓝天」「是不是暖色调的日落」「是不是纯色壁纸」，**不能**回答「有没有一只狗」「是不是在开会」「画面里有没有文字」。
- 覆盖的词表是固定的四十多个（见上），超出词表的概念搜不到。
- 旧 Python 版靠 PyTorch 四模态模型做过真正的跨模态检索，改用 llama.cpp 后失去了这个能力 —— 这是重构的技术代价，不是回归 bug。

### 其它

- `Hit` 新增 `tags`，`search` 结果里带出，前端表格与预览面板都会展示。
- 新增调试工具 `src-tauri/src/bin/visdemo.rs`（`visdemo.exe <图片或目录> [--profile]`），可直接打印标签与底层指标，方便调阈值。

## v0.16 — 2026-10-22

**补回命令行接口（CLI）**

重构到 Tauri 之后，程序变成了纯 GUI，`--selftest` 之外没有任何命令行能力，而用户最初的需求里明确要求「批量操作与命令行接口」。这一版把它补回来，直接编译进发布版 exe。

- 新增 `src-tauri/src/cli.rs`，入口 `deepaisee.exe --cli <子命令>`：
  - `scan <目录>...` 扫描入库，结束时打印目录/文件/新增/更新/跳过/错误/已消失/体积/耗时，并把目录写回配置。
  - `search <关键词> [--kind K] [--limit N] [--no-semantic]` 混合检索：文件名、路径、拼音、全文、语义一起打分。
  - `sem <一句话描述>` 纯语义检索。
  - `similar <文件路径>` 找同类型相似文件。
  - `stats` 库统计（按类型分布、正文/向量完成数、两套向量库条数）。
  - `index [--what auto|text|embed] [--wait] [--force]` 建索引，`--wait` 阻塞到队列跑完。
  - `rules` 列分类规则；`serve [--port N]` 前台启动局域网服务。
- 打包版是 windowed 程序（没有控制台），CLI 用 `AttachConsole(ATTACH_PARENT_PROCESS)` 挂到父进程控制台，因此 **cmd / PowerShell 里能直接看到输出，双击运行则什么都不发生**。
- 参数解析区分「开关型 flag」与「取值型 flag」（`--kind image` 的 `image` 不会被当成关键词，`--limit 3` 的 `3` 也不会）。
- 自愈：`index` 发现文本向量库为空但记录状态仍是 `ok`（换模型导致维度变化时会出现）会自动重置并重新入队；`--force` 可强制全部重建。
- 实测（debug 版，19 个文件）：
  - `--cli stats` → 图片 13 / 文本 3 / 视频 1 / 音频 1 / 其他 1，媒体库 15 条 / 256 维。
  - `--cli search 会议` → `会议纪要-2026Q3.md 1.24`；`--cli search 余弦` → `vector_search.py 0.31`；`--cli search hyjy` → `会议纪要-2026Q3.md 0.77`（拼音首字母）。
  - `--cli sem 怎么比较两段文字的相似程度` → `vector_search.py 0.63`、`会议纪要 0.28`、`项目计划书 0.25`。
  - `--cli similar .../forest.png` → 10 条，全部是图片（同类隔离生效），首条 `img104.jpg 0.50`。
  - `--cli index --what embed --wait` → 「检测到文本向量库为空，已重置 18 条记录重新入队」→ `向量 18（失败 0）`。
- 修复：CLI 在 PowerShell 管道 / 重定向输出时一个字都收不到。原因是 `attach_console()` 无条件 `AttachConsole` + `SetStdHandle`，把调用者已经给好的管道句柄覆盖掉了。现在先 `GetStdHandle(STD_OUTPUT_HANDLE)`，句柄有效就完全不碰（直接输出与 `> file` 重定向都验证过）。
- 新增：`npm run tauri build` 现在能完整产出 **NSIS 安装包** `deepaisee_0.0.16_x64-setup.exe`（9.62 MB），不再只靠绿色免安装目录分发。
- 发布版实测（绿色便携目录，16 个文件）：
  - `--selftest` → **通过 11 项，失败 0 项**，模型 `llama.cpp · 768 维`。
  - `--cli stats` → 16 个文件 / 6.12 MB，文本库 3 条 / 768 维，媒体库 196 条 / 256 维。
  - `--cli search 余弦 --limit 2` → `vector_search.py 0.46`、`会议纪要-2026Q3.md 0.29`。
  - GUI 连续运行 30 秒稳定（416MB 常驻），无 `panic.log`，界面顶栏 `deepaisee v0.16`、状态栏 `llama.cpp · 768 维 · cpu | 16 个文件 · 6.12 MB · 正文 3 · 向量 16`。

## v0.15 — 2026-10-22

**修复一个会导致程序启动后自动退出的严重缺陷**

- 修复（严重）：EmbeddingGemma 2 的 GGUF 在 llama.cpp 里同时有 `embedding_length = 512`（隐层）和 `embedding_length_out = 768`（投影输出）。`llama_shim.cpp` 原来用 `llama_model_n_embd()` 取维度，按 512 拷贝 `llama_get_embeddings*()` 返回的 768 维缓冲，**每次编码都越界写坏堆**。表现是程序能正常启动、能搜索，但运行一段时间后以 `0xc0000409`（`ucrtbase.dll` 里的堆损坏快速失败）无提示退出。现在改用 `llama_model_n_embd_out()`，维度正确报 **768**。
  - 连带修复：向量库维度因此变化，旧的 512 维向量会被自动作废并触发重建；`vectors/meta.json` 现在写的是真实维度。
- 修复（严重）：索引队列遇到「记录还在库里、文件已经不在磁盘」的条目时直接 `return Ok(())`，不改任何状态 —— 这条记录永远停在 `pending`，下次启动又被排进队列。库里积累几万条这种记录时（例如扫过整盘后大量临时文件消失），程序表现为**开机就满载空转、几十秒后自行退出**。现在会调 `mark_missing` 把它们标记掉。
- 修复：启动阶段补索引会无条件把库里所有 `pending` 全部入队。新增 `enqueue_all_pending_limited(what, limit)`，启动时最多补 5000 条，其余留给用户在「扫描 / 索引」页手动重建；达到上限会在日志页留一条提示。
- 修复：向量库每处理一条文件就全量 `save()`（几百 MB 量级），索引大目录时硬盘持续狂转。改成每 64 条落盘一次，并在队列空闲时 `flush()`。
- 新增：发布版装 panic hook，未捕获的 panic 会写入 exe 同级 `panic.log` 与 `data\panic.log`（带 backtrace）。以前这类问题只会表现为「双击后窗口闪一下就没了」。
- 索引线程每处理 200 条写一次「索引进度」心跳到活动日志，方便排查卡在哪一步。
- 实测：干净数据目录启动后连续运行 5 分钟稳定（414MB 常驻）；768 维向量库 `meta.json` = `{"count":3,"dim":768}`；`--selftest` 模型维度项通过。

## v0.14 — 2026-10-22

**打包链路修复：发布版变成真正的生产构建**

- 修复：用 `cargo build --release` 出的 exe 打开后是 Edge 的「无法访问此页面 / localhost 拒绝连接」。原因是 Tauri 只有在 `tauri build` 下才会把 `frontendDist` 的静态资源嵌进二进制，直接 `cargo build` 编出来的程序仍去连 `devUrl: http://localhost:1420`。发布必须走 `npm run tauri build`。
- 修复：`tauri build` 报 `failed to find main binary`，因为 crate 里还有 `smoke` / `selftest` 两个辅助 bin。`Cargo.toml` 增加 `default-run = "deepaisee"`。
- 修复（严重）：`[features] default = []` 导致 `tauri build` 不带 `--features llama` 时编出**没有本地推理能力**的版本，运行时不报错、静默退回哈希基线（状态栏显示 `hash · 256 维`）。现在 `default = ["llama"]`，忘写参数也不会降级。
- 修复：扫描统计计数器未清零，连续扫描两次会出现 `files: 31 → 46` 的假增长；扫描结束后对「本次没遇到、但路径在扫描根下」的记录标 `missing`，根目录整体消失时整批标记；扫描页新增「已消失」计数。
- 修复：同一文件因 `C:/a/b` 与 `C:\a\b` 两种写法重复入库（实测 35 条记录只有 16 个真实文件）。`FileRow::new` 统一 `norm_path`，`migrate()` 对历史数据做去重。
- 修复：重命名模板已含 `{ext}` 时又自动追加扩展名，得到 `forest.png.png`。
- 发布版实测（`dist-app\deepaisee.exe`，14MB，内嵌前端 + llama.cpp）：`--selftest` **通过 11 项，失败 0 项**；语义检索「怎么计算余弦相似度」→ `vector_search.py (0.72)`；界面状态栏 `llama.cpp · 512 维 · cpu`；索引 18 个文件 8.97 MB；文本向量库 3 条 / 512 维、媒体向量库 15 条 / 256 维。

## v0.13 — 2026-10-22

**扫描结果与磁盘真实状态对齐**

- 修复：删掉的文件会永远留在库里，扫描几次后统计越滚越大（实测 16 个真实文件被记成 35 条）。现在扫描结束会把「本次没遇到、但路径在扫描根下」的记录标成 `missing`，文件恢复后再次扫描会自动回到正常状态。
- 修复：扫描根目录本身被删除时，它下面的记录全部标记为缺失并计入「已消失」。
- 修复：扫描统计计数器没有清零，连续扫描两次会出现 `files: 31 → 46` 的假增长。现在每次开扫前 `reset()`。
- 修复：同一文件因斜杠写法不同（`C:/a/b` vs `C:\a\b`）重复入库。`FileRow::new` 统一走 `norm_path`，`migrate()` 增加历史数据去重（`UPDATE OR IGNORE ... replace(path,'/','\')` + `DELETE ... NOT IN (SELECT MIN(id) ...)`）。实测 35 条 → 19 条、重复组 0。
- 修复：重命名模板已含 `{ext}` 时又自动追加 `.png`，得到 `forest.png.png`。
- 界面：扫描页新增「已消失」计数，大于 0 时用琥珀色提示。
- 实测：连扫两次统计稳定在 16 个文件 / 5 个目录；删掉 `forest.png` 再扫描 → 库内 19→18 条、`missing=1`；恢复文件再扫描 → `missing=0`。

## v0.12 — 2026-10-22

**打包版自检 + 换模型后自动重建向量**

- 新增 `--selftest`：无界面自检 11 项（读配置、开库、加载 GGUF、文本向量化、开向量库、全文/拼音/文件名检索、语义检索、图片找相似、目录统计），结果写 exe 同级 `selftest.txt`。打包版没有控制台，这是用户确认「装好了能用」的唯一途径。
- 修复：换嵌入模型导致维度变化时，`VectorStore` 会丢弃旧向量文件，但数据库里 `embed_state` 仍是 `ok`，于是「重建索引」排队永远是 0 个，用户以为重建了其实一条都没跑。现在打开向量库发现「原来有条目、现在变成 0 条」就把所有文件标回 `pending` 并立刻排队。
- 修复：`model_load` 命令里还有一处 `cfg.save(&state.app_dir)`（把目录当文件写），保存配置会失败。
- 发布版实测：`deepaisee.exe --selftest` **通过 11 项，失败 0 项**；语义检索「怎么计算余弦相似度」→ `vector_search.py (0.72)`；全文「会议」→ `会议纪要-2026Q3.md`；拼音 `hyjy` → 同一文件。

## v0.11 — 2026-10-22

**四合一界面与后端联调完成**

- 界面：搜索/相似、树状图、扫描/索引 三个模式合并进单一窗口，右侧设置抽屉承载模型、分类规则、局域网、日志、设置五个分页。
- 修复：空查询时结果列表永远为空（现在可以「只按类型/时间浏览」全部文件）。
- 修复：`save_config` 把目录当文件路径写，导致保存配置报「拒绝访问 (os error 5)」。
- 修复：找相似结果跨模态混入（图片源会返回音频/视频）。现在默认只在同类型里比较——图片的颜色直方图、音频的频谱、视频的帧均值物理含义不同，跨类型相似度没有意义。
- 修复：`StatsOut` 前后端字段不一致导致状态栏崩溃。
- 验证：界面验收 11 项全过；扫描 `C:\Windows\Web\Wallpaper\Windows` 新增 2 个文件；局域网服务 `http://<本机IP>:8770` 可访问，`/api/search?q=会议` 返回正确命中。

## v0.10 — 2026-10-22

**llama.cpp 真正接上**

- 转换 `E:\quake\EmbeddingGemma 2\model` → `models/embeddinggemma2.gguf`（542.2MB，413 张量，架构 `gemma-embedding2`）。
- 新增 `native/llama_shim.cpp`：用 C++ 包一层，只暴露 4 个 C ABI（create/dim/embed/free），避免在 Rust 里镜像 llama.cpp 的 struct（版本一变内存布局就错）。
- 构建：CMake + Ninja 编译 llama.cpp 静态库，`build.rs` 把 `ggml.a` 复制成 `libggml.a` 供 rustc 链接。
- 效果：模型加载 0.5s、单条编码 50~63ms、同文本余弦 1.0000、不同文本 0.7275。
- 嵌入前缀纠正：查询加 `task: search result | query: `、文档加 `title: none | text: `（llama.cpp 路径原来把 `is_query` 丢弃了）。
- **能力边界**：llama.cpp 只处理文本塔，输出 768 维（GGUF 的 `embedding_length_out`）；图片/音频/视频走 `HashEmbedder`（256 维）。两套向量分库存放（`vectors/` 与 `vectors_media/`），不会互相污染。因此「文字描述找图片/音频/视频」在 llama 后端下**不支持**，界面已标注。

## v0.09 — 2026-10-22

**四合一前端**

- Vue 3 + Vite：`App.vue` 顶栏三个模式按钮 + 保活切换，`SearchView` / `TreeView` / `ScanView` / `SettingsDrawer`。
- 组件：`ResultTable`（6 列 + 匹配进度条）、`PreviewPane`（缩略图/视频/元信息）、`Treemap`（TS 手写 squarify + ResizeObserver）。
- 修复：`watch` 变量名遮蔽 Vue 的 `watch()` → `TypeError: watch2 is not a function`。
- 修复：`open` 导入遮蔽 prop `open` → 设置抽屉永远开着。
- 修复：Tauri 2 状态类型不匹配（`.manage(Arc<AppState>)` 而命令签名 `State<AppState>`）→ **所有命令全部失败**，界面能渲染但拿不到任何数据。
- 修复：`AppConfig::load` 传目录而非文件 → 配置静默回退默认值，模型永远不自动加载。

## v0.08 — 2026-10-21

**Rust 后端骨架**

- 模块：`config` / `db`（SQLite WAL + FTS5 unicode61 + 中文 2-gram）/ `scan`（walkdir）/ `textract`（PDF/DOCX/XLSX/PPTX/EPUB/TXT）/ `py`（拼音）/ `search`（名称+路径+拼音+模糊+FTS+语义）/ `vector`（暴カ点积）/ `index` / `fileops` / `watch` / `server`（axum）/ `state` / `cmd`（40+ 命令）。
- 修复：`Db::update_fields(old_path, ...)` 参数名被 `path=` 撞掉，移动/重命名后库里路径不同步。
- 修复：Windows 上 memmap 文件无法覆盖，向量库改纯内存 + 落盘时先写 tmp 再 replace。

## v0.07 — 2026-10-21

**工程骨架**

- Tauri 2 + Vue 3 + Vite 脚手架，深色主题 CSS 变量，应用图标（32/64/128/256 + ico）。
- 工具链：MSYS2 UCRT64 gcc 16.2 + rustc 1.99.1（`x86_64-pc-windows-gnu`）+ Node 22 + CMake/Ninja，走清华/rsproxy 镜像。

---

## 从 DiskSeek 到 deepaisee

旧版 DiskSeek（`D:\软件\123`，Python 3.12 + PySide6）功能完整并已实测交付：

| 能力 | DiskSeek | deepaisee |
| --- | --- | --- |
| 框架 | Python + PySide6 | **Rust + Tauri 2**（安装包 8MB 级，运行时用系统 WebView2） |
| 前端 | Qt Widgets | **Vue 3 + Vite** |
| AI 引擎 | PyTorch + sentence-transformers | **llama.cpp（GGUF）** |
| 文本向量 | 768 维（EmbeddingGemma 2 四模态） | 768 维（llama.cpp 文本塔投影输出） |
| 图片/音频/视频向量 | 768 维（同一个四模态模型） | 256 维哈希特征（llama.cpp 不支持这些模态） |
| 跨模态「文字找图」 | 支持 | 不支持（降级为文件名/路径匹配，已在界面标注） |
| 全文/拼音/模糊/名称检索 | 支持 | 支持 |
| 图找相似图 / 音找相似音 / 视频找相似视频 | 支持 | 支持（同类型内比较） |
| 树状图 | 支持 | 支持 |
| 扫描 / 索引 / 实时监控 | 支持 | 支持 |
| 批量操作 / 导出 | 支持 | 支持 |
| 局域网访问 | 支持 | 支持 |
| 安装包体积 | 4.4 GB（含 CUDA PyTorch） | 约 20 MB（不含模型） |
