# CraftWave UI/UX 优化方案（基于 ZCode 桌面端源码调研）

> 调研对象：`/Users/yzb/Code/Personal/ZCode`（Electron + pnpm monorepo，UI 库 `packages/ui`，设计规范 `DESIGN.md` 655 行，令牌 `packages/ui/src/styles.css` 2279 行）。
> 调研日期：2026-10-28。对照基准：CraftWave 当前三栏定稿 + 字阶 12/13/14/15/17/20/28。
> 每条给：CraftWave 现状 → ZCode 做法（源码出处）→ 建议动作。工作量：S（半天内）/ M（1-2 天）/ L（多天）。

## 总评

CraftWave 与 ZCode 的设计方向一致（浅灰一体底、克制用色、lucide 单色图标、平铺助手消息），布局骨架（三栏、拖宽、折叠分组、composer 三层）也已经对齐。真正的差距在两处：

1. **体系化**：ZCode 的字号是"基准变量 + 偏移档"、边框悬停是"前景色透明度"、动效有统一时长曲线、圆角随嵌套层级强制递减——改一处全局生效；CraftWave 是钉死的 px 值，"字体偏小"这类反馈要改几十处。
2. **细节精致感**：扫光代替转圈、思考过程的流动摘要行、空状态问候语 + 建议 chips、渐隐 mask 替代省略号、定制滚动条——单点都不大，叠起来是"小家子气"与"高级感"的差距。

---

## A. 设计系统升级（高杠杆，一次投入长期受益）

### A1. 相对字阶：基准变量 + 偏移档 ⭐ 直接根治"字体偏小"（工作量 S，优先级 P0）

- **现状**：字阶是固定 px（`text-[15px]` 等 6 档），全局缩放需全仓替换；用户连续两轮反馈"偏小"，第三轮还会来。
- **ZCode**：`:root { --ui-font-size: 14px }`，所有档位是偏移：`text-ui-lg = +2px`、`text-ui-caption = -1px`、`text-ui-sm = -2px`（`packages/ui/src/styles.css:149-166`）。运行时可在 12–20px 间调（`packages/ui/src/lib/uiFontSize.ts`，设置页有缩放项），**明令禁止改 html 根字号**（DESIGN.md:298，避免破坏 rem 布局）。
- **动作**：styles.css 定义 `--ui-font-size: 15px` + 派生档 `--fs-meta/-1 / --fs-secondary/-2 / --fs-body/0 / --fs-title/+2 / --fs-title-lg/+3`（calc 实现），组件里 `text-[15px]` 换成 `text-[length:var(--fs-body)]` 类；设置页"外观"加字号滑杆（12–18px）。
- **收益**：以后"再大一点"= 改一个数字；用户可自助调节。

### A2. 边框与悬停改透明度混合（工作量 S，P1）

- **现状**：亮暗两套实色 hex（border #e2e6ea / #33373d），每加一档灰都要两处手调。
- **ZCode**：border = `rgba(13,13,13,0.1)`、hover 边框 0.15、hover 面 0.05、surface = `rgba(255,255,255,.05)`（`packages/ui/src/styles.css:531-533,695-699`）——亮暗同一套 alpha 逻辑，前景色反相自动成立。
- **动作**：`--border: color-mix(in srgb, var(--foreground) 10%, transparent)`，hover 面同理；暗色亮色只差 foreground 本身。可先只改 border/hover 三档试水。

### A3. 圆角层级成文（工作量 S，P1）

- **现状**：composer `rounded-2xl`、会话卡 `rounded-xl`、设置卡 12px、行卡 10px/8px/6px 混用，无成文规则。
- **ZCode**（DESIGN.md:370-430）：外壳 `rounded-2xl`（白名单：composer 壳/toast/状态浮板）→ 首个面板 `rounded-xl` → 嵌套 `lg → md → sm`；菜单外壳 `lg`、菜单项 `md`；对话框 `rounded-2xl`。
- **动作**：按此规则盘点 CraftWave（会话卡 xl ✓、composer 2xl ✓ 白名单内、设置卡 12px→`rounded-lg` 对齐、菜单壳统一 lg、菜单项 md），落进 styles.css 注释。

### A4. focus 规范：输入框 border 提亮、不加 ring（工作量 S，P2）

- **现状**：Input focus = border 变蓝 + `ring-2 ring/40`，视觉偏重。
- **ZCode**：单行输入 `focus-visible:border-input-border-focused + bg-input-focused + ring-0`（`packages/ui/src/components/ui/input.tsx:7`）；仅多行 Textarea 保留 `ring-2 ring-ring/30`（textarea.tsx:12）。
- **动作**：Input 去 ring、加 `bg-input-focused`（底色微变）；Composer 输入壳用 border 提亮。安静的聚焦态更显高级。

### A5. 全局滚动条定制（工作量 S，P1）

- **现状**：系统默认滚动条（macOS overlay 尚可，Windows 常驻灰条很破）。
- **ZCode**（styles.css:847-872）：webkit 宽/高 14px、track 透明、thumb 用 `border: 3px solid transparent` + `background-clip` 裁出细圆角条（`border-radius:9999px; min-width:32px`），颜色 `var(--color-border)`；另设 `scrollbar-color` 供 Firefox。
- **动作**：整块 CSS 抄过来即可，Win/Linux 立刻精致一档。

### A6. motion token 与浮层动效统一（工作量 S-M，P2）

- **现状**：transition 零散 150/200ms 无约定，浮层出现无动画（tier/model 菜单是瞬间弹出）。
- **ZCode**：fast=120 / base=160 / enter=200ms + 统一曲线 `cubic-bezier(0.22,0.61,0.36,1)`（styles.css:1510-1518）；所有浮层一律 `duration-100 + fade-in-0 + zoom-in-95`（dialog/select/dropdown/tooltip 全一致）；自定义 keyframes 全部带 `prefers-reduced-motion` 降级。
- **动作**：styles.css 定 `--t-fast/base/enter` + 曲线；三个下拉菜单、退出确认弹框补 zoom-in-95 入场。

### A7. 图标尺寸档 + 次级图标降透明（工作量 S，P2）

- **现状**：SVG 图标 12/14/16px 按各文件手感定；次级图标靠手动 `text-[var(--cw-dim)]`。
- **ZCode**：档位 12/14/**16（默认）**/20/24（DESIGN.md:436-442），Button 内 svg 按钮尺寸自动缩放；`lucide-colors` 插件让 subtle 色图标自动降不透明度（styles.css:276）。
- **动作**：定档 12/14/16 并核对现有组件； CraftWave 手写 SVG 少，可只在规范里约定，不做插件。

### A8. 阴影语义档（工作量 S，P3）

- **ZCode**：仅浮层有影——菜单/popover/对话框 `shadow-md`，toast `shadow-lg`，其余全靠边框分层（DESIGN.md:556-563）。
- **动作**：检查 CraftWave 现有 `shadow`/`shadow-sm`（回底钮、composer 壳），统一为"浮层才有影"。

## B. 聊天体验（感知最强，中等成本）

### B1. 空状态重做：问候语 + 建议提示词 ⭐ "小家子气"主因之一（工作量 M，P0）

- **现状**：两行小字「CraftWave / 开始新对话」。
- **ZCode**（`ConversationDraftEmptyState.tsx:200-241`、`ConversationDraftSuggestedPrompts.tsx`）：六档时段问候语（5/9/12/14/18/23 点切换），字号按容器宽自适应 20-30px；下方建议提示词 chips（`h-8 rounded-lg outline`），waterfall 逐个下落入场（260ms，delay=index×65ms）；背景大 Logo 线框底部渐隐。
- **动作**：问候语 + 4-6 个建议 chips（点击填入 composer）+ 简单渐隐 Logo 线框；waterfall 动效可后补。

### B2. Markdown 排版对齐：标题克制、strong 降级（工作量 S，P0）

- **现状**：`.cw-md` h1=1.35em、strong 默认 bold；标题在聊天流里过大过重。
- **ZCode**（`packages/ui/src/ai-elements/message.tsx:432-447,1536-1592`）：h1=18px/h2=16px/**h3-h4=正文大小仅加粗**，`mt-6 mb-4`；**strong 渲染为 font-medium**（不与标题抢层级）；链接蓝色 + 点状下划线 `decoration-dotted underline-offset-4`；行内代码 `bg-*/50` 半透明底。
- **动作**：.cw-md 改 h1 1.2em / h2 1.1em / h3-h4 1em、`strong { font-weight: 500 }`、a 加 dotted 装饰（链接色上轮已改蓝 ✓）。

### B3. 工具卡：扫光代替转圈 + 展开记忆 + 失败 tooltip（工作量 M，P1）

- **现状**：运行中 LoaderIcon 旋转；每次展开状态不记忆；失败只在摘要行标红。
- **ZCode**（`ToolLayout.tsx:139-147`、`ToolCallProgressIndicator.tsx`）：运行态**不用 spinner**（性能注释），kind 标签 `animated-gradient-text` 扫光 + `h-1 w-16` 细进度条扫动；失败状态词点状下划线 + hover 弹 `max-w-96` 错误摘要卡（可复制）；展开状态按 toolId 模块级 Map 全局记忆，收起延迟 300ms 卸载保动画。
- **动作**：三件都值得抄；扫光 keyframes（styles.css:913-932）4s linear 渐变平移，成本很低。

### B4. 思考过程：流动摘要行 + 左导线展开（工作量 M，P1）

- **现状**：`<details>` + 灯泡图标 +「思考过程」；展开后是气泡卡。
- **ZCode**（`ai-elements/reasoning.tsx`）：折叠行「思考中 · Ns」流式期间**滚动显示思考最后一行**（单行 nowrap + 两侧 16px 渐变 mask + 标题扫光）；展开内容 `ml-2 border-l pl-3.5` 左导线 + 纯文本 whitespace-pre-wrap（性能注释：不用 Markdown 渲染）+ `max-h-60` 吸底滚动；完成固定显示时长。
- **动作**：先做「左导线展开 + 纯文本 + 思考中计时」，流动摘要行二步做。

### B5. 用户消息气泡折角（工作量 S，P2）

- **ZCode**（`ConversationUserInputBubble.tsx:25-28`）：`rounded-xl rounded-tr-xs border border-border bg-surface px-4 py-3`——右上角 2px 小折角 + 1px 边框。
- **动作**：气泡 rounded-2xl→rounded-xl + rounded-tr-xs + 加边框；一行类名的事，辨识度立增。

### B6. 错误横幅 actionable（工作量 M，P2）

- **现状**：错误块展示文案，无操作。
- **ZCode**（`ChatErrorBanner.tsx:264-271`）：按错误码分流（模型不可用→设置入口按钮、断连→重连），居中浮条 `rounded-xl bg-surface backdrop-blur-md`，一键复制完整错误。
- **动作**：先加「复制错误」+「打开设置」（E_MODEL_NOT_FOUND 类码），分流后续按错误面扩展。

## C. 会话与外壳（锦上添花）

### C1. 侧栏细节三件（工作量 S-M，P1）

- **渐隐 mask 替代省略号**：标题溢出右侧 16px 渐隐（`TaskTitleOverflowText`，TaskListItem.tsx:652）而非 truncate 截断——观感更柔和。
- **滚动边缘渐隐**：列表视口上下 `mask-image` 渐隐（ScrollFadeViewport），暗示可滚动。
- **行状态槽**：前置 16px 槽位——运行中转圈 / 错误红点 / 未读蓝点 / 悬停变操作钮（TaskListItem.tsx:569-624）。CraftWave 现在只有运行绿点；运行中改转圈、加未读/错误态可与通知系统联动。

### C2. Composer 细节（工作量 S，P2）

- 拖文件入卡：整卡 `border-brand ring-1 ring-brand/30` + 半透明覆盖提示（ChatPromptEditor.tsx:417-424）——M2 附件功能时一起做。
- 发送钮改方形 icon 钮（`size icon-md rounded-lg bg-brand`，ArrowUp 16px，CraftWave 已是 ArrowUp SVG ✓ 只差钮形）。
- 停止钮 tooltip 标注 Esc；Esc 停止已支持。

### C3. 上下文占用 hover 面板（工作量 M，P3）

- **ZCode**（`contextUsage.tsx:929-1101`）：composer 工具栏占用触发器 hover 弹 `w-80` 面板：`used/size (percent)` 等宽字 + 分段进度条 + 分项百分比 + 缓存命中率。
- **动作**：Inspector 已有数据，composer 状态栏加个触发器复用即可；P3。

## D. 远期工程项（记录备查）

| 项 | ZCode 做法 | 说明 |
|---|---|---|
| 虚拟化时间线 | `@tanstack/virtual` + live tail 单独行（ConversationTimeline.tsx:2051） | 长会话性能，等出现卡顿再做 |
| 快捷键系统 | `shortcutCommands.ts` 全可配置 + 冲突检测 | 等快捷键数量破 10 个再做 |
| CJK 流式渲染 | `@streamdown/cjk` 插件 | 中文断行/词距优化，Streamdown 已在用，加插件即可（S） |
| 会话内查找 | Cmd+F 命令面板 + timeline 高亮 | 依赖面大，P3 |
| 控件密度档 | Button h-5/6/7/8，默认 h-7 | ZCode 明显更紧凑；CraftWave 走舒适档是风格选择，不建议跟 |
| 设置页分区 | 17 个分区、设置是 tab、命令中心 | CraftWave 设置页结构已够用 |

## 建议实施顺序

1. **第一批（本周可完成，全是 S）**：A1 相对字阶 → A5 滚动条 → B2 Markdown 排版 → B5 气泡折角 → A3 圆角成文
2. **第二批**：B1 空状态重做 → A2 透明度边框 → B3 工具卡扫光
3. **第三批**：B4 思考过程 → A6 动效 token → C1 侧栏三件 → A4 focus 规范

---

## 2026-10-29 复核与补齐

本轮对照本地 ZCode 源码及已安装依赖，复核第一至第三批和后续像素对齐改动。已有的三栏布局、权限/模型菜单、气泡折角、滚动条、字阶与动效令牌继续沿用；以下是本轮发现并处理的缺口。

| 对应项 | 发现 | 本轮处理 |
|---|---|---|
| A1 字号 | 只有基准变量，没有用户入口 | 设置提供 12–18px 滑杆；`ui.fontSize` 可保存、重启恢复、即时预览，取消回滚；旧配置默认 14px，后端校验范围 |
| A5 滚动条 | 新版 Chromium 的标准滚动条属性会覆盖 WebKit 自定义细节 | 支持 WebKit 伪元素时只启用这套规则；其他引擎保留标准属性 |
| A6 减少动态效果 | 标题走马灯和旋转状态未完整降级 | 同时停用走马灯、扫光、旋转与过渡，保留可读的静态状态 |
| A7 / B3 工具卡 | 正式卡片仍用 emoji；运行转失败时没有自动展开 | 改用已有 lucide 单色图标与动作标签扫光；首次失败自动展开，尊重用户手动开合；失败原因可复制 |
| B3 历史工具结果 | 恢复的结果是字符串，摘要只处理对象，缺少边车时原因消失 | 同时处理字符串和对象结果，历史失败原因仍可读、可复制 |
| B4 思考过程 | 帧已有 status/timing，但 UI 没消费 | 显示思考状态、实时秒数、最后一行摘要与渐隐；展开沿用左导线和纯文本，上翻暂停吸底，结束后固定时长 |
| B1 空状态 | 问候字号仅 18px，跨时段不刷新 | 标题按容器宽度调整到默认 20–30px，建议词窄屏换行；跨小时刷新问候 |
| B2 Markdown | Streamdown 实际用 span 呈现加粗、按钮呈现安全链接，旧 strong/a 选择器未覆盖；依赖内工具类未扫描 | 补 `data-streamdown` 选择器；声明 Tailwind 扫描路径，映射已有颜色令牌；代码容器不重复套底，保留复制控件和安全链接确认 |
| B6 错误恢复 | 横幅无操作，运行时只传文案丢掉错误码 | 完整保留错误码与文案，复制含反馈；模型/认证错误可直达供应商设置 |
| C1 标题与滚动渐隐 | 标题复制后参与自身溢出测量，滚动距离不正确；列表内容变化不刷新渐隐 | 只测原文宽度，按整段宽度循环；短标题不渐隐，支持键盘焦点触发；观察列表内容与尺寸变化 |
| 设置与窄窗口 | 右内容列固定 50%，页体最小宽度挤出横向滚动；主题卡片规则被后面的样式覆盖 | 内容列提供可读下限，窄容器内主题卡片堆叠；Composer 状态栏允许换行 |
| 深色主题 | Tailwind 的 dark 工具类默认跟随系统，与应用手动选择不一致 | dark 变体统一跟随应用的 `.dark` 类 |

### 验证范围

- macOS 上 `MACOSX_DEPLOYMENT_TARGET=13.0`：`cargo fmt --check`、`cargo build`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 通过（243 个库测试、25 个集成测试）。
- 前端 `pnpm typecheck`、`pnpm test`、`pnpm lint`、生产构建通过（20 个测试文件、128 项测试）。
- Chromium 155 和 WebKit 26.5 均使用真实 App、assistant-ui runtime 和正式组件，通过临时 IPC 桩提供配置及帧；核对亮暗主题、1280×800 / 960×640、字号保存重载与取消、建议词填入、思考流、工具失败、错误复制、设置跳转、走马灯、减少动态效果和 Markdown 计算样式，两个引擎均无页面异常。
- 临时预览入口不进入产品代码，也不连接真实模型或写用户配置。浏览器验证不能代替 Windows/Linux 原生安装包验收；本轮没有改动交通灯与原生窗控。

### 仍保留的后续项

- B3 的富文本错误浮层及延迟卸载动画：本轮保留原生提示，错误全文在展开区可选择、可复制。
- B1 的背景品牌水印：尚未采用，避免把 ZCode 的品牌图案带入 CraftWave。
- C1 未读/错误持久状态、C2 附件拖入：等待对应业务状态与附件能力。
- C3 上下文 hover 面板、CJK 插件、虚拟化、快捷键配置与会话内查找：按原方案保留后续优先级。现有会话累计 token 不能准确代替单次请求的上下文占用，不额外包装成精确占比。
- 历史记录未存 reasoning 起止时刻时，只显示“思考过程”，不推算时长。

## 2026-10-29 中间聊天内容区专项复刻

本轮以本地 ZCode 的 `components/ai-elements/message.tsx`、`ToolCallBlocks/ToolSummaryRow.tsx`、`ChatErrorBanner.tsx` 和 `v4/ConversationUserInputBubble.tsx` 为对照，集中调整聊天内容区。

- **字体与排版**：正文使用基准字号和 1.75 行高、0.025em 字距；一级标题为基准 +4px，二级 +2px，三级至六级为基准字号，分别按字重区分。行内代码为基准 −2px，代码块及工具详情为基准 −1px。等宽字体显式补上中文无衬线回落，避免 Windows 中文落到宋体。
- **Markdown**：继续使用现有 Streamdown，接入其官方 [代码高亮插件](https://streamdown.ai/docs/plugins/code) 和 [中文排版插件](https://streamdown.ai/docs/plugins/cjk)。高亮随应用亮暗主题切换，流式期间延迟高亮，结束后启用；保留链接安全确认。列表、任务列表、引用、表格、行内代码和代码块的间距统一，代码块去掉重复的内层边框。复制等控件提供中文文案。
- **用户消息**：与 ZCode 一样保留用户输入的原始换行和 Markdown 标记，避免把提示词里的标题、星号解释为回复排版；气泡最大宽度、边距和右上折角对齐。
- **工具与思考**：摘要改为正文档字号，说明和参数使用次级字号；摘要箭头跟在内容后，悬停或键盘聚焦时出现。详情统一为左侧导线，完整参数、结果与边车数据均可读取，长结果限制高度。参数准备、运行、等待确认、完成、失败与中断有对应呈现，取消后不会一直显示运行扫光。
- **错误卡**：默认显示单行摘要、复制和模型设置入口；完整错误可展开、选择和复制，长报错在详情内滚动。沿用原生 details 保留键盘操作，错误详情不另建模态窗口。
- **移除再次生成**：删除 Reload 入口及适配器专用的尾部消息裁剪逻辑，移除调用 reload 的测试，更新当前需求、技术设计和验收口径。普通发送、失败后的继续发送、复制、编辑重发入口及历史兄弟分支恢复继续保留；只有单分支时隐藏分支切换控件。过去的 spike 任务记录保留为历史证据。

### 本轮验证

- `MACOSX_DEPLOYMENT_TARGET=13.0` 下 Rust 构建、Clippy、测试通过；243 个库测试、25 个集成测试。
- 前端类型检查、Biome、生产构建和 134 项测试通过。
- 临时预览挂载正式 ChatThread、assistant-ui runtime 和八类正式工具卡，通过本地假数据验证，不连接真实模型或修改用户配置。Chromium 与 WebKit 均检查亮暗主题、12/14/18px 字号、960px/600px 聊天列、长路径换行、工具开合与计划内容、长错误展开和设置跳转、流式未闭合围栏及完成后的语法高亮，页面无异常。实际计算样式和高亮颜色均核对。
- 临时预览入口删除；截图和复核脚本留在忽略目录 `.zcode-ref/chat-content-2026-10-29/`，不进入提交。Windows/Linux 原生安装包、真实模型调用和数学公式/Mermaid 专用插件不在本轮验证范围。
