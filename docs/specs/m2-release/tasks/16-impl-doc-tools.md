# 实现：文档读取 docx / xlsx / pdf（read 工具文档管线）

Type: task
Status: open

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

- [ ] 三类真实文档（含中文）端到端读出文本，表格输出为结构化 Markdown
- [ ] docx-rs 已知缺陷样本触发降级路径且有 warnings 标注
- [ ] 畸形 PDF 不崩（catch_unwind 生效）返回友好错误
- [ ] >10 页 PDF 无显式页码被拒，页码窗口 ≤20 页生效
- [ ] 超限响应按 512KB 截断并给续读指引；§8.3/§8.9 出入回写完成
