# 实现：小工具包 batch_read / glob / calculate / render_html

Type: task
Status: resolved

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

- [x] 四工具进注册表，ReadOnly 档位全放行（含 plan 档，§8.5）——tools_registry_test 钉死静态段 13→17 与能力/过滤矩阵；backend_e2e small_tools::plan_tier_sees_and_executes_all_four_small_tools 验证 plan 档执行器真实执行四工具成功
- [x] batch_read 一次读 20 文件、单文件失败不炸、截断有续读指引——batch_read_test 覆盖混合成功失败/20 上限/共享 512KB 截断/nextOffset 续读往返
- [x] glob 尊重 .gitignore 与 500 上限；calculate 长浮点链与 Python 对照精确——glob_test（递归模式/gitignore/500 截断/越界拒）；calculate_test（0.1+0.2 与 1/3 的 formatted 与 Python repr 逐字对照、10/3*3=10 舍入一致）
- [x] render_html 预览沙箱逃逸向量全防住，CSP 放行方案回写 §9.5——Rust 侧 PREVIEW_CSP 禁外联断言 + 前端 sandbox 属性契约断言（只带 allow-scripts）；§9.5 增【M2】沙箱预览 CSP 段（跨框动态逃逸行为由 opaque origin 规范语义保证，happy-dom 无法真实执行跨框脚本，静态契约面见决议记录第 5 条）
- [x] 方案批准卡带「查看预览」入口可渲染 HTML 产出——AskPanel approval 形态 previewHtml 附带（write/edit 目标 .html/.htm 时宿主构造）+ small-tools.test.tsx 入口两用例
- [x] 检查链全绿（cargo fmt --check / build / clippy --all-targets / test：Rust 502 lib + 集成全过；pnpm typecheck / test 262+8 / lint）

## 决议记录（2026-10-30 实现）

1. **batch_read 上限可配落地（票面二选一定案：回写 §3.2 再实现）**：AppConfig.tools 增 `batchReadLimit`（缺省 20、合法 1–100、越界拒绝整份提交、旧配置 serde default 回填、save_config 后下一次工具调用生效），执行器经 `with_batch_read_limit` 按 run 装配；已回写 §3.2 与 §8.3。区间上限 100 的定案理由：真正的硬顶是 512KB 共享响应预算，本字段只是文件数闸门。
2. **calculate 选型定案（票面候选定案，已回写 §8.3）**：票面首选 `evalexpr` 13.1 评估通过（纯 Rust/MIT/空上下文无函数调用面，cargo 可解析），但其错误类型**不携带 token 位置**，票面「除零/语法错误 E_INVALID_ARG 带出错位置」无法满足；按票面备选手写递归下降求值器（约 300 行零依赖，词法/解析/求值一体），错误位置原生携带（1-based 字符位置）。语义口径：整数 i64 checked（溢出报错提示浮点改写）、除法永远浮点（Python `/` 语义，6/2 显示 "3"）、`%` 为 Rust 截断取余（与 Python 负数取余不同，description 已注明）、`^` 幂右结合、比较与 `pi/e/true/false`；浮点格式化用 Rust 最短往返（与 Python repr/bc 对照，calculate_test 逐字对照 0.1+0.2/1/3/10/3*3）。
3. **render_html 预览通道定案（票面验证点实证，已回写 §9.5）**：srcdoc/blob: 帧继承主窗 CSP（HTML 规范：本地方案文档继承创建者 policy container），主窗 `script-src 'self'` 拦预览页内联脚本；票面候选「对 blob: 帧放宽 script-src 'unsafe-inline'」在 CSP 文档级作用域下**技术上不可实现**（CSP 无法按帧区分来源，放宽即放宽全窗）——不取。落地走自定义协议 `preview`（lib.rs 注册）：协议响应自带预览专用 CSP（放内联脚本/样式、禁一切外联防内网探测；自定义协议响应不经 tauri 的 CSP 注入，两侧不叠加），主窗 CSP 仅扩 `frame-src` 一项，iframe 叠加 `sandbox="allow-scripts"`（无 allow-same-origin）opaque origin 双重隔离。内容经 #59g `preview_register` 进程级注册表（token = sha256 前 16 hex 幂等、256 条容量按插入序淘汰、不落盘随应用退出清空；历史卡片重展开时前端按参数原文重注册兜底）。
4. **「查看预览」附带条件（票内定案，采纳票面建议）**：write 附 content 全文、edit 附 newText 全文（预览语义 = 编辑后效果），目标扩展名 .html/.htm 且内容 ≤50,000 字符才附带——载荷不膨胀，diff 仍是审批主预览面；AskPayload::Approval 增可选字段 `previewHtml`（ts-rs 生成物同步）。
5. **沙箱逃逸向量测试的诚实边界**：票面「vitest + 真实 iframe 的动态逃逸断言」在 happy-dom 环境不可真实执行（happy-dom 不运行跨框脚本、不实现浏览器 sandbox 安全模型）——落地面为装配契约静态断言（sandbox 属性只带 allow-scripts、地址走 preview 协议、注册失败不挂 iframe）+ Rust 侧 PREVIEW_CSP 禁外联逐指令断言；跨框动态行为（parent 访问/top 导航/cookie 读取抛 SecurityError）由 `sandbox="allow-scripts"` 无 allow-same-origin 的 opaque origin 规范语义保证，真实引擎验证留待 macOS 手工验收（34 号票收口口径）。
6. **预览注册表失效的降级路径**：应用重启后进程内注册表清空，历史卡片的旧 token 加载返回 404 说明页（「预览内容已失效，请重新生成或展开卡片重新加载」）；前端 HtmlPreview 每次挂载自动重注册（html 来自工具参数原文，随会话历史持久），正常路径不触发 404。
7. **watchdog 分段读纠偏提示收编**（37 号票预留占位兑现）：提示文案扩为「一次性读取更大窗口（read 的 offset/limit）、用 batch_read 一次读取多个文件、或改用 grep 定位目标内容」——batch_read 解决跨文件碎片化，单文件大窗口仍指 read 窗口。
8. **顺手修正 13 号票遗留（相邻接线面同款错误）**：#59f `search_test` 的 Rust 命令签名为单结构体参数 `args: SearchTestArgs`，前端 invoke 却平铺传 `{ target, ... }`——Tauri 2 按参数名提取（`CommandItem::get("args")`），平铺形状真实运行报 missing key（vitest mock 不经真实 invoke 故未暴露）；本票 #59g `preview_register` 同款接线时发现并修正（client.searchTest 改为 `{ args }` 包层 + SettingsPage 断言同步），决议记录在案。
9. **glob 匹配语义**：globset 默认 literal_separator=false（`*` 跨目录分隔符）——`*.rs` 与 `**/*.rs` 都能递归命中任意深度；工具 description 已注明，比 shell glob 宽容更契合「找文件」场景。遍历与 list_files 完全同源 ignore 栈（respectGitignore 默认 true、require_git(false)、hidden(true)、滤 .git）。
10. **独立 find 工具不另立**（票面出入提示定案）：glob 的 pattern 支持 `**` 递归语义已覆盖文件名模式检索需求，§8.3 M2 表无独立 find 行——维持单工具承担，不回写 §8.3 增行。
