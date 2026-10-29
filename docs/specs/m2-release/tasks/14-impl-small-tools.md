# 实现：小工具包 batch_read / glob / calculate / render_html

Type: task
Status: open

## 要做什么

补齐模型高频小工具：一次读多文件、按文件名模式找文件（尊重 .gitignore）、精确算数（防 LLM 算错）、沙箱预览自包含 HTML（FR-12-04）。四工具进注册表、审批面、工具卡片齐全。覆盖 §8.3 M2 表 batch_read/glob/calculate/render_html 四行。

## 具体做什么

### 后端（src-tauri/）

- **`tools/batch_read.rs`**（ReadOnly，§8.3 行「≤20 文件；单文件行窗口同 read」）：参数 `{ paths: string[] }`；上限 ≤20（超限 E_INVALID_ARG）；逐文件复用 read 文本路径（util::encoding 解码、行窗口默认 2000 行）；**512KB 单响应上限跨全批次共享**，截断时逐文件标注 truncated 与续读 offset；结果 data=`{ files:[{ path, ok, content?, error?, truncated }], truncatedFiles }`；单文件失败不失败整批。原票面「上限可配」：承载=AppConfig 增 `tools: { batchReadLimit?: number }` 缺省 20——**字段级定义缺失，先回写 §3.2 再实现**（不回写则按常量 20 交付并在票面记录）。
- **`tools/glob.rs`**（ReadOnly，§8.3 行「文件名模式检索（上限 500）」）：参数 `{ pattern, path? }`；globset 匹配（依赖树已在，显式入 Cargo.toml）；遍历用 `ignore` crate WalkBuilder——**与 list_files 同源 ignore 栈**（respectGitignore 默认 true、跳 .git/隐藏文件，对齐 tools/list_files.rs 现状）；上限 500 条目截断标注；结果按路径排序 `[{ path, kind }]`。**票面出入提示：设计文档 §8.3 M2 表只有 glob 行、无独立 find 行——文件名模式检索由 glob 单工具承担（pattern 支持 `**/*.rs` 递归语义）；若评审要独立 find 工具，先回写 §8.3 再实现。**
- **`tools/calculate.rs`**（ReadOnly，§8.3 行「纯 Rust 表达式求值器（无副作用、无函数调用）」）：参数 `{ expression }`；表达式引擎**票内定案**——候选 `evalexpr`（纯 Rust、MIT、活跃、可关函数调用面只留算术/比较/数学常量）优先，备选手写 Pratt 解析器（约 200 行零依赖）；定案标准=无副作用、无 IO、无任意函数调用、浮点结果与 Python/bc 对照精确；**定案后回写 §8.3 calculate 行**；结果 data=`{ expression, result, formatted }`（formatted 保真显示，防浮点截断误导）；除零/语法错误 E_INVALID_ARG 带出错位置。
- **`tools/render_html.rs`**（ReadOnly*，§8.3 行「*只读但产物可见」）：参数 `{ html }` ≤50,000 字符（超限 E_INVALID_ARG）；结果 data=`{ html, charCount }`，渲染全在前端沙箱，后端不落盘不解析。
- 注册表：`TOOL_NAMES` 静态段扩为四工具全量断言（与 12 号票扩容合并计数，防漂移测试以「静态段全集」为准）。

### 前端（ui/）

- 四张工具卡（`chat/ToolCards.tsx` + `chat/toolkit.ts`，CW_TOOL_PARAMETERS 增四行与 manifest 双向锁定）：batch_read=文件数徽标+逐文件成功/失败列表+内容折叠；glob=命中计数+路径列表（上限展示 100，对齐 grep 卡）；calculate=表达式+结果 monospace 单行；render_html=沙箱预览卡+字符数徽标。
- **render_html 沙箱预览**：iframe `sandbox="allow-scripts"` **无 allow-same-origin**（§8.3）；srcdoc 或 blob: URL 票内实测定案（CSP `frame-src 'self' blob:` 已放行，§9.5）；预览默认折叠手动展开（防闪烁/防误执行）。**票内验证点：srcdoc/blob 帧继承主窗 CSP，`script-src 'self'` 会拦预览页内联脚本——若拦，按 §9.5 纪律最小放行（候选：对 blob: 帧放宽 script-src 'unsafe-inline'，沙箱 opaque origin 兜底隔离）并回写 §9.5。**
- **方案批准卡「查看预览」入口**（FR-12-04）：`chat/AskPanel.tsx` approval 形态增按钮——审批载荷附带 html 时显示「查看预览」，点击以 render_html 同款沙箱渲染目标内容；附带条件票内定案（建议：write/edit 目标扩展名 .html/.htm 时由 host 审批载荷构造附带 diff 后全文）。

### 测试

- 单测：batch_read 混合成功失败/20 上限/共享 512KB 截断/续读 offset；glob 递归模式/gitignore 生效/500 截断/起点越界拒；calculate 四则/括号/浮点链/除零/语法错位置；render_html 50k 限制与 data 形态。
- **沙箱逃逸向量测试**（vitest + 真实 iframe）：预览页尝试 `parent` 访问、top 导航、读 cookie/localStorage 均失败断言（无 same-origin 生效）。
- e2e：plan 档四工具可见（ReadOnly 放行，§8.5 矩阵）；render_html 调用→卡片折叠态→展开预览。

## 边界（不在本票）

- web_fetch/http_request（12 号票）；skill/subagent（09/10 号票）；AST fence 与写后检查（22 号票）；独立 find 工具（如需先回写 §8.3）；文档格式解析（16 号票）；PDF/图片预览（16/17 号票）。

## 验收

- [ ] 四工具进注册表，ReadOnly 档位全放行（含 plan 档，§8.5）
- [ ] batch_read 一次读 20 文件、单文件失败不炸、截断有续读指引
- [ ] glob 尊重 .gitignore 与 500 上限；calculate 长浮点链与 Python 对照精确
- [ ] render_html 预览沙箱逃逸向量全防住，CSP 放行方案回写 §9.5
- [ ] 方案批准卡带「查看预览」入口可渲染 HTML 产出
- [ ] 检查链全绿
