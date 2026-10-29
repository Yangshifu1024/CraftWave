# 调研：Office/PDF 解析 crate 选型

Type: research
Status: resolved

## Question

FR-12 文档工具（§8.9 文档管线）动工前的 crate 调研。目标：从 docx/xlsx/pdf 提取文本（含表格结构化）作为工具输出。约束：三平台编译无系统依赖负担（打包体积敏感：M1 dmg 仅 6.77MiB）、维护活跃、许可兼容（MIT/Apache 生态）。

候选核实（2026 现状）：xlsx→calamine；docx→docx-rs 等读侧方案；pdf→pdf-extract vs pdfium-render（绑定体积/许可/三平台编译成本）vs lopdf 等纯 Rust 路线；对比提取质量与回退策略。

结论给出三格式各一个首选与理由，供 16 号票直接动工。

## Answer

（2026-10-29 调研，数据取自 crates.io API / docs.rs / GitHub API 当日快照）

### 结论

- **xlsx → calamine 0.37**（首选）。纯 Rust、MIT、只读、`Range` 天然就是表格结构。备选：umya-spreadsheet（读+写）。
- **docx → docx-rs 0.4（bokuweb）`read_docx`**（首选）。备选：docx-rust；兜底：zip+quick-xml 手解 `word/document.xml`。
- **pdf → pdf-extract 0.12**（首选）。纯 Rust、MIT、零系统依赖，内置 CMap 处理。备选：pdfium-render（质量最高，但要随包分发 ~3.4–3.7MB/平台 动态库，体积与 CI 成本高）。lopdf 原始路线仅适合对象级操作，不建议直接用于文本提取。

总体判断：三个首选全部满足硬约束——纯 Rust（无 `-sys`、无 C 依赖）、MIT/Apache 兼容许可、2025-2026 均在活跃维护、对 dmg 体积影响在 1–2MB 量级。pdfium-render 是唯一的"质量逃生口"，若后续遇到 pdf-extract 提不出的复杂版式 PDF 再按需引入（代价是三平台各带一份动态库 + dmg 从 6.77MiB 涨到 ~10MiB）。

### 对比表

| 维度 | calamine | umya-spreadsheet | docx-rs (bokuweb) | docx-rust | docx-lite | pdf-extract | pdfium-render | lopdf |
|---|---|---|---|---|---|---|---|---|
| 最新版本 | 0.37.0 | 4.0.0-beta.1 | 0.4.22 | 0.1.11 | 0.2.0 | 0.12.1 | 0.9.4 | 0.46.0 |
| 最后发布 | 2026-10-18 | 2026-10-26 | 2026-07-21 | 2026-01-22 | 2025-09-27 | 2026-09-16 | 2026-09-06 | 2026-10-21 |
| 总下载量 | 1661 万 | 142 万 | 377 万 | 232 万 | 11.4 万 | 719 万 | 353 万 | 2698 万 |
| 近 90 天下载 | 618 万 | 51 万 | 78 万 | 14 万 | 6 万 | 372 万 | 174 万 | 1321 万 |
| 许可 | MIT | MIT | MIT | MIT | MIT/Apache | MIT | MIT OR Apache-2.0 | MIT |
| 系统依赖/unsafe | 纯 Rust 无 | 纯 Rust 无 | 纯 Rust 无 | 纯 Rust 无 | 纯 Rust 无 | 纯 Rust 无（9 个依赖均无 `-sys`） | **需运行时绑定 PDFium 动态库**（约 3.4–3.7MB 压缩包/平台，解压后更大；有 `static` feature 但需自备静态库） | 纯 Rust 无 |
| 维护活跃度 | 极高（2026 年发 5 版） | 高（beta 期） | 高（2026-10-17 仍有提交，star 561） | 中 | 低（单人/机构，发布史仅 1 天） | 高（2026 年 3 版，回归 bug 数日内修复；star 604） | 极高（89 个版本） | 极高（2026-10-29 仍有提交，star 2290） |
| 表格结构化 | `worksheet_range` → `Range`（带起止行列的原生二维表） | 对象模型可读写 | 读侧支持 tables/嵌套 tables（README 自述） | 自述"解析+生成"，读侧文档薄 | 纯文本抽取为主 | **无**（线性文本流，需自行按坐标重建；纯 Rust 派均无表格识别） | **无**（提供字符级坐标/分段，表格需自行重建） | 无 |
| 已知重大缺陷 | 只读（对我们无影响）；xls/ods 打开时全量加载 | 4.0 尚在 beta | 读侧有两个开放缺陷：合法 docx 解 zip 失败 #873/#832；非默认命名空间前缀丢文本 #933 | 读侧成熟度低于 docx-rs | 太新、无生态验证 | ① 非恒等预定义 CMap 未实现 #103（部分无 ToUnicode 的中文 PDF 会乱码）② 对不可信输入可能 panic #97/#141（工具边界需 `catch_unwind`）③ 无表格重建 | ① 包体积 +50% ② 动态库与 crate 版本符号匹配脆弱（#192）③ 三平台 CI 需分发二进制 | 文本提取官方自述有限制，CID/CJK 常乱码（社区报告）；作为 pdf-extract 底层已被封装 |

### 分格式理由

**xlsx：calamine（首选）**
- 事实：纯 Rust 只读 reader，覆盖 xls/xlsx/xlsm/xlsb/ods；`worksheet_range()` 返回带 `start`/`end` 尺寸的 `Range<Data>`，逐行 `rows()` 即结构化表格；另有 `RangeDeserializer` 可反序列化为强类型行、`worksheet_formula()` 取公式、`defined_names()` 取名称。README 明确 "no support for writing excel files, this is a read-only library"——CraftWave 只提取，正合适。
- 中文：单元格文本是 XML 内的 UTF-8，无编码风险。
- 回退：umya-spreadsheet（纯 Rust、读+写、活跃），仅在将来需要"写 xlsx"时启用。

**docx：docx-rs（首选）**
- 事实：生态最大（377 万下载），`read_docx(&bytes)` 解析为与写侧同一套 `Docx` 模型，README 自述解析能力覆盖 "Tables, nested tables, numbering, styles, and theme colors"；MIT、2026-07 刚发版、仓库 2026-10 仍有提交。
- 风险：读侧有两个未关闭的真实缺陷（合法文件 zip 解析失败 #873；非默认命名空间前缀丢文本 #933）。16 号票动工时应：① 用真实世界 docx 样本集跑回归；② 对 `read_docx` 失败做降级路径。
- 回退层级：docx-rust（cstkingkey fork，"parsing and generating"，MIT，2026-01 发版）→ zip+quick-xml 手解 `w:document/w:tbl/w:tr/w:tc`（约两百行，docx 本质是 zip+XML，零新增重依赖，兜底保底）。docx-lite 太新（2025-09-27 单日发布两版，单一机构维护）不采纳。

**pdf：pdf-extract（首选）**
- 事实：纯 Rust，0.12.1 依赖树 9 个 crate 全部无 `-sys`（lopdf、adobe-cmap-parser、encoding_rs 等），零系统库、零跨平台编译负担；自带 `extract_text_from_mem` 一行式 API；通过 adobe-cmap-parser 处理 CJK CMap、encoding_rs 处理遗留编码，中文支持在纯 Rust 路线中最好（高于裸 lopdf）；2025-2026 恢复活跃，0.12 引入的跨页字体缓存回归在数日内修复（#151→#152）。
- 风险与对策：① 无表格结构识别（所有纯 Rust 方案皆无）——16 号票范围按"线性文本"交付，表格重建列为后续增强；② 部分使用非恒等预定义 CMap 且无 ToUnicode 的中文 PDF 会乱码（#103 开放）——工具输出中标注"提取可能不完整"；③ 对畸形 PDF 可能 panic（#97/#141）——工具实现处用 `catch_unwind` 包裹并返回友好错误。
- 回退：pdfium-render（质量天花板：字符级坐标/分段/搜索 API，对复杂中文版式最稳；MIT OR Apache-2.0、极活跃）。但硬伤是体积与分发：crate 不含 PDFium，README 明确 "pdfium-render does not include Pdfium itself"，需按平台分发动态库（bblanchon/pdfium-binaries 最新 release：mac-arm64 压缩包 3.38MB、win-x64 3.72MB、linux-x64 3.65MB，解压后更大），且存在动态库与 crate 版本符号失配问题（#192）。dmg 将从 6.77MiB 涨到 ~10MiB，三平台 CI 均需下载/打包二进制。结论：不作为首选，作为质量升级逃生口。
- 排除项：mupdf-rs 系 MuPDF 绑定，AGPL 许可与 MIT/Apache 生态冲突，直接排除；裸 lopdf 文本提取官方 README 自述有限制且 CID/CJK 乱码问题多发，且其能力已被 pdf-extract 封装，无需直接使用。

### 证据链接

- calamine：https://crates.io/crates/calamine ｜ https://github.com/tafia/calamine ｜ README（formats/Range/read-only/pure Rust 原文）
- umya-spreadsheet：https://crates.io/crates/umya-spreadsheet
- docx-rs：https://crates.io/crates/docx-rs ｜ https://github.com/bokuweb/docx-rs ｜ 缺陷 https://github.com/bokuweb/docx-rs/issues/873 、#832、#933
- docx-rust：https://crates.io/crates/docx-rust ｜ https://github.com/cstkingkey/docx-rs
- docx-lite：https://crates.io/crates/docx-lite
- pdf-extract：https://crates.io/crates/pdf-extract ｜ 依赖树 https://docs.rs/crate/pdf-extract/0.12.1 ｜ 缺陷 https://github.com/jrmuizel/pdf-extract/issues/103 、#97、#141、#151
- pdfium-render：https://crates.io/crates/pdfium-render ｜ https://github.com/ajrcarey/pdfium-render ｜ 符号失配 https://github.com/ajrcarey/pdfium-render/issues/192
- PDFium 预编译库体积：https://github.com/bblanchon/pdfium-binaries（latest release 资产实测）
- lopdf：https://crates.io/crates/lopdf ｜ https://github.com/J-F-Liu/lopdf ｜ 乱码案例 https://stackoverflow.com/questions/78597641/
