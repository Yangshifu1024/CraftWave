# 实现：文档读取 docx / xlsx / pdf（read 工具文档管线）

Type: task
Status: resolved

Blocked by: 15

## 要做什么

模型能直接读 .docx/.xlsx/.pdf：文本与表格结构化输出、分页/分 sheet 控制、超限明确提示可续读。按 15 号票选型（calamine/docx-rs/pdf-extract 三首选，均纯 Rust MIT、dmg 增量 1–2MB）落地 §8.9 文档管线读侧，v1 按结构化文本交付（15 号票：纯 Rust 无 PDF 表格重建）。

## 具体做什么

### 后端（src-tauri/）

- 依赖（15 号票定案版本）：`calamine 0.37`（xlsx，`worksheet_range` 天然二维表）、`docx-rs 0.4`（bokuweb `read_docx`）、`pdf-extract 0.12`（`extract_text_from_mem`，内置 CMap 处理）。
- 落点：**`read` 工具扩展**（§8.3 read 行已含「PDF>10 页须显式页码」，故文档读取挂 read 而非独立工具——以 §8.3 现状为准）：`tools/read.rs` 按扩展名 .docx/.xlsx/.pdf 分派进新文件 `tools/doc.rs`（解析与 read 文本/图片路径并列；**§8.3 read 行参数未含文档页码字段，先回写 §8.3（增 `page?`/`pages?` 与扩展名分派语义）再实现**）：
  - **docx**：`read_docx` → 段落文本线性输出 + 表格转 Markdown 表格（含嵌套表格展平标注）；`read_docx` 失败（#873 解 zip 缺陷）或非默认命名空间丢文本（#933）→ **降级路径** zip+quick-xml 手解 `w:document/w:tbl/w:tr/w:tc`（15 号票兜底方案）+ 结果 warnings 标注「降级解析」（warnings 仅 UI 不进模型文本，§8.2）。
  - **xlsx**：calamine 逐 sheet `worksheet_range` → 每 sheet Markdown 表格；`{ sheet?: string }` 选 sheet，缺省输出 sheet 名清单 + 首 sheet 前 N 行；公式单元格取缓存值（calamine `worksheet_formula` 备用）；xls/xlsm/xlsb/ods 顺带覆盖（calamine 免费）。
  - **pdf**：pdf-extract 线性文本（v1 无表格重建，15 号票口径）；**`catch_unwind` 包裹**（#97/#141 对不可信输入可能 panic）→ panic 转 E_INVALID_ARG「文档解析失败（可能损坏或加密）」；输出头部标注「提取可能不完整（部分中文 PDF CMap 限制 #103）」；**>10 页须显式页码**（§8.3 read 行）：`{ page?: number }` 或 `{ pages?: [from, to] }`，单次窗口 ≤20 页（§8.9 口径）。
- 大小口径协调：单次响应 ≤512KB（与 read_workspace_file #28 同口径）超限按整行边界截断 + `truncated=true` + 「用 page/sheet/offset 继续」提示；分 sheet/分页即分页机制；文件本体先 fs.read（超大文件读取前拒并提示），解析在 safe_join roots 内（复用 read 路径守卫）；加密/损坏文件 E_INVALID_ARG 明确话术。

### 前端（ui/）

- ReadCard 增文档态（`chat/ToolCards.tsx`）：扩展名徽标（docx/xlsx/pdf）+ 页数/sheet 数摘要 + 结构化内容折叠预览（Markdown 表格渲染）；truncated 显示「可用页码/sheet 续读」提示行（提示模型，非按钮）；warnings「降级解析」黄条。
- PDF.js 阅读器与 xlsx 结构化预览卡**不做**（§8.9 前端半属 M3）；本票只保证模型通道与基础卡片。

### 测试

- 样例文件入库 `src-tauri/tests/fixtures/docs/`：中文 docx（段落+表格+嵌套表格）、多 sheet 含公式 xlsx、中文文本 PDF（>10 页样例）、加密 PDF、损坏文件各一；**真实世界 docx 样本集 ≥3 份**（不同来源导出，15 号票对 #873/#933 的回归要求）。
- 断言：三类端到端解析（中文不乱码）、表格输出为结构化 Markdown、docx 降级路径可触发（构造 #873 形态样例）且有 warnings、畸形 PDF panic 被 catch_unwind 兜住、>10 页无页码拒绝、页码窗口 ≤20 页、512KB 截断与续读提示。
- 契约：CW_TOOL_PARAMETERS read 行若增字段与 manifest 双向锁定。

## 边界（不在本票）

- 文档**写/改**与保真管线（解包→替换→重打包、docbackups 备份事务、list_document_backups/restore_document_backup 命令——§8.9 M3 半）；PDF.js/xlsx 预览卡（M3）；OCR/扫描件；pdfium-render 质量逃生口（15 号票：遇提不出的复杂版式再按需引入）。

## 验收

- [x] 三类真实文档（含中文）端到端读出文本，表格输出为结构化 Markdown
- [x] docx-rs 已知缺陷样本触发降级路径且有 warnings 标注
- [x] 畸形 PDF 不崩（catch_unwind 生效）返回友好错误
- [x] >10 页 PDF 无显式页码被拒，页码窗口 ≤20 页生效
- [x] 超限响应按 512KB 截断并给续读指引；§8.3/§8.9 出入回写完成

## 决议记录（2026-10-31 实现完成）

- **选型落地**：calamine 0.37.0 / docx-rs 0.4.22 / pdf-extract 0.12.1 均按 15 号票定案版本与首选路径；配套依赖全部复用依赖树内版本（quick-xml 0.42 ← tauri→plist；zip 8.6 ← docx-rs；lopdf 0.42 ← pdf-extract），上游各自锁版不强行归一，新增下载面极小。
- **工具挂载**：按票面以 §8.3 现状为准——read 工具扩展名分派（`tools/doc.rs` 新文件承载解析），不设独立 document_read 工具；注册表静态段 17 个不变；`params_schema` 增 `page?`/`pages?`/`sheet?`（单一来源 `doc::params_schema_properties()`，前端 toolkit.ts 逐字镜像，contract.test 双向锁定；manifest 由 cargo test 自动再生）。§8.3 read 行与 §8.9（改标【M2 读侧已落地／M3 写改与备份】）已回写，变更记录 v2.22。
- **降级路径实测**：#933 形态（手写 `ns0:` 前缀命名空间的中文 docx，含表格）触发「零文本检测 → 降级 quick-xml（按本地名匹配）→ warnings『降级解析』」全链；#873 形态（损坏 zip 字节）首选失败走降级，彻底失败给结构化错误——两条路均不 panic。真实世界样本集共 6 份 docx、3 种生成来源（macOS textutil/Cocoa 引擎导出 3 份、docx-rs 写侧 2 份、手写最小形态 1 份），满足「≥3 份不同来源」回归要求。
- **加密 PDF 的两个事实**：① CoreGraphics 仅设 UserPassword 时会**静默导出明文**（须 User+Owner 口令同时设置才写入 /Encrypt 字典），加密样本已按后者重新生成；② 工具不依赖解析器对加密文件的未定义行为，用 lopdf 读 trailer `/Encrypt` 在解析前给出明确错误（口令不在工具参数面，弱加密解出的内容亦不可信）。
- **catch_unwind 三处**：docx-rs / calamine / pdf-extract 三类解析调用均在工具边界包裹（AssertUnwindSafe），panic 统一转 E_INVALID_ARG「文档解析失败（可能损坏或加密）」；畸形字节样本实测走结构化错误。
- **docx 额外防线**（票面未点名、实现补充）：zip 炸弹防线（document.xml 解压 ≤64MiB）、表格嵌套深度 ≤16 层（防深嵌套 XML 打爆调用栈）、文档本体 ≤50MiB 读取前拒绝。
- **xlsx 口径补充**：缺省预览行数 N=100（票面未定值，平衡预览完整度与响应预算）；公式单元格取文件内缓存值（`worksheet_range` 天然给出，`worksheet_formula` 备而不用，不输出公式文本）；xls/xlsm/xlsb/ods 经 `open_workbook_auto_from_rs` 顺带覆盖。
- **已知限制（如实记录）**：
  - 中文 PDF 提取存在部首变体映射（实测「页」→「⻚」、「文」→「⽂」）——CoreGraphics 导出的 CID 键控中文字体无完整 ToUnicode，属 #103 的真实形态；输出头部已按票面标注「提取可能不完整」，测试只锚定提取结构与 CJK 内容存在性，不锚定逐字还原。
  - PDF 表格无结构化重建（v1 口径，15 号票定案：纯 Rust 无表格识别；pdfium-render 质量逃生口遇复杂版式再议）。
  - docx 无分页续读参数（票面参数面仅 pdf page/pages 与 xlsx sheet）；docx 超限截断时 `resumeHint` 如实说明「暂无分页参数」。
  - 前端 PDF.js 阅读器与 xlsx 结构化预览卡不做（票面边界，M3）；ReadCard 文档态已交付（格式徽标 + 页数/sheet/段落摘要 + Markdown 折叠预览前 80 行 + truncated 续读提示行 + warnings 黄条）。
- **测试**：后端 21 例（`tools/doc_test.rs`，样本 10 份入库 `tests/fixtures/docs/`：真实导出 docx×3、写侧 docx×2、#933 形态 docx×1、多 sheet 公式 xlsx×1、25 页英文 PDF、12 页中文 PDF、加密 PDF、损坏 PDF；生成器 `examples/gen_doc_fixtures.rs` + `examples/gen_pdf_fixtures.swift` 入库可复现）；前端 4 例（`toolcards.test.tsx` 文档态组）。检查链七项全绿（Rust lib 523 + 集成；前端 274）。
