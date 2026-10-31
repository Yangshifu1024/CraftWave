# 对话流渲染规范（gpui-rewrite）

> 本规范是阶段 2.C 全部部件的验收依据。来源：Wake 精读（workbench.rs 消息渲染全链路）+ `docs/ZCode桌面端UI-UX优化方案.md`（B 部分验收条目）+ 三条纪律（用户拍板的痛点根治）。
> Wake 参考一律指 `crates/wake/src/workbench.rs`（除非另注），行号基于 2026-10-31 精读版本。

## 三条纪律

### 纪律一：折叠只有两种东西，且不嵌套

- `thinking` → 一行摘要（chevron 旋转 90° 展开 + "Thinking" + 单行斜体摘要），展开为左导线纯文本（**不用 Markdown**，性能自觉），默认折叠；
- 工具调用 → 每条消息**一张簇卡**（不是一个工具一张卡），折叠头 + Input/Output 分节展开，默认折叠；
- **废除 run 过程折叠组**——工具簇直接平铺在消息流里；
- **废除一切自动收起**——展开状态用户说了算，跨重渲染记忆（`HashSet<行号>`），行高变化手工 `msg_list.splice(ix..ix+1, 1)` 重测。
- 现状对照（为什么）：现栈 `useRunCollapse` 的 3 秒兜底收起与 final-answer 触发收起让过程在用户眼皮底下消失，回看要连点三层——这是结构性问题，样式修不动。

### 纪律二：过滤优于折叠

- 运行日志型消息（Assistant 且无正文、无工具、无图、只有 thinking）→ 渲染层直接滤掉（Wake 7128-7135 返回空 div），源数据保留；
- System / CompactSummary → 居中小胶囊（`one_line` 单行截断）；
- 流里剩下的都是值得读的。

### 纪律三：只有元数据可以截断，且按实际宽度

- **正文、错误信息、工具结果永不截断**：错误卡默认全文可选可复制，长报错在详情内滚动；
- 工具 Input/Output：展示前 600 字符 + 明示"First 600 characters" + **复制按钮永远复制完整原文**；不使用嵌套滚动容器（会抢主阅读区滚轮，Wake 10037 注释）；
- 路径、参数等元数据：按容器实际宽度反算可显示格数（`(阅读区宽 - 预留px) / 等宽字符宽`，Wake 7103-7104），截断用 `clip_display`（见下），hover 出完整值 + 点击复制；
- 侧栏标题等溢出用渐隐 mask 替代省略号（UX 方案 C1）。

## 数据层

| 机制 | 规范 | Wake 参考 |
|---|---|---|
| 消息源 | `Rc<Vec<Message>>` 克隆共享、逐行借用，渲染路径不做深拷贝 | 7120 |
| 虚拟列表 | `ListState::new(0, ListAlignment::Bottom, 512px overscan)`，不等高惰性测量 | 2082 |
| 展开状态 | `HashSet<行号>` 两张表（thinking / 工具簇），跨重渲染保持 | 7117 |
| 行高变更 | 展开折叠后 `splice(ix..ix+1, 1)` | 7191 |
| 图片三态 | Ready / Unsupported（可下载原图）/ Omitted（明示"转录已达上限"）——不假装有图 | 9486-9588 |

## 行分发

四形态 + 过滤器先跑（见纪律二）：

- **User**：右对齐气泡，`max_w(540px)`、muted 底、行高 1.85、内容段直挂气泡不加包装层（避免 max-content 探测问题）；
- **Assistant**：全宽列 = thinking 面板 → 正文（行高 1.9）→ 工具簇，平铺；
- **System / CompactSummary**：居中胶囊。

阅读轴：全消息 `justify_center + max_w(READER_MAX_WIDTH)`，与详情头共用同一竖直轴；首条 `pt`、末条 `pb` 呼吸边距。搜索跳转高亮：淡 primary 底 + **负 margin/等量 padding**——背景外扩不推挤内容（7266-7273）。

## Markdown 正文

`TextView::markdown` + 四个定制点（Wake 9615-9694）：

1. **标题字号是"基准 × 系数"函数**：h1=1.45×、h2=1.28×、h3=1.14×、h4=1.05×、h5+=基准（`heading_font_size` 闭包）——标题克制，天然支持全局缩放；
2. 段间距参数化：user 0.5rem / assistant 0.6rem；
3. 代码块一个 StyleRefinement（muted 底 + 1px border + 圆角）+ `code_block_actions`（语言名 + 复制钮）；
4. `.selectable(true)` 全文可选。

两个关键细节：
- **主题写进元素 id**（`{id}-dark/-light`）：语法高亮色解析期固化，id 带主题让亮暗切换走同步重解析，代码块不闪色（9631）；
- **链接分流**（markdown_links.rs `resolve_link` 判序照搬）：空/`#` 忽略 → Windows 盘符先于 scheme 识别 → 裸文件名 `main.rs:11` 不当 scheme → 非 file scheme 外部浏览器 → 远程/不存在给提示 → `file:` URI 严格解析 → 裸路径先按字面查（保留 `% # ?`）、失败且含 `%` 才单次百分号解码重试 → `:N` 行号后缀文件不存在时去掉再找。

## thinking 面板

- 折叠行：chevron（展开 rotate 90°）+ "Thinking" + 单行斜体摘要（`one_line(400)` 再 `clip_display(_, 88)`）；
- 展开：左缩进 30px、11.5px 小字号、muted 色、行高 1.7、纯文本；
- 视觉降级完整：颜色淡、字号小、斜体——正文是主角；
- 流式期间：摘要行滚动显示最后一行（ZCode 方案 B4 形态）。

## 工具簇卡

- **一簇 = 一条消息的全部工具调用一张卡**；
- 折叠头：工具名（单）/ "N tool calls"（多）+ 等宽字体参数摘要（宽度自适应格数）+ 失败数红 badge（"2 failed"）+ 派生子代理时直接给跳转钮；
- 展开区：border-t 分隔，每调用 Input/Output 分节；错误 Output 用 danger 淡底 + danger 字色；
- 600 字符 + 完整复制契约（见纪律三）。

## 文本工具函数（整函数照搬）

- `clip_display`（Wake format.rs）：**显示单元宽度**截断——CJK 按 2 单元（unicode-width）、**字素簇不可拆分**（组合符/ZWJ emoji 不劈半）、省略号宽度计入预算。中文 UI 截断的完整答案，含 CJK/组合字符/emoji 单测；
- `one_line`（wake-core text.rs）：先 `split_whitespace().join(" ")` 压成一行（不是取首行——首行可能是空的），再按字符数截断加 `…`；
- 顺手搬：`smart_time`（刚刚 → N 分钟前 → 今天时刻 → 昨天 → 日期）、`tilde_path`（`~` 折叠必须在分隔符上断）。

## 图片

- 缩略图 104px，**按宽高比决定约束方向**：竖图定宽、横图定高；
- 点击开 zoom（记消息+图索引）；Unsupported → 下载原图卡；Omitted → 明示卡。

## 选择与滚动

- 全文可选（`.selectable(true)`），跨消息选择、Shift-click 扩展为验收项；
- **选择拖到视口边缘自动滚动**：gpui-component `AutoScroll`（`compute_delta` 16px 触发区 + 80px 坡道，12→64 px/s 对称曲线）；`MouseMoveEvent.dragging() && has_text_selection` 驱动，滚动条拖动不误触；
- 新消息到达的滚动跟随与"上滚暂停"作为 composer 附件切片的一部分实现并测试。

## UX 方案 B 部分验收条目（从 ZCode 方案迁移）

- B1 空状态：时段问候语 + 建议 chips；
- B2 Markdown 排版：标题克制（上式已覆盖）、strong 不与标题抢层级、链接点状下划线、行内代码半透明底；
- B3 工具卡：运行态不用 spinner（扫光/细进度条）、展开状态记忆、失败自动展开一次且尊重手动开合、失败原因可复制；
- B4 思考过程：流动摘要行 + 左导线 + 思考计时（流式期间）；
- B5 用户气泡：右上折角 + 1px 边框；
- B6 错误卡：按错误码分流（模型不可用→设置入口）、一键复制完整错误；
- 动效：统一时长与曲线 token，`reduce_motion` 全局降级（gpui 测试里 `cx.set_reduce_motion(true)` 一帧到位）。

## 测试装置模式

`#[gpui::test]` + 最小 `Render` 装置（测试放独立 `_test.rs`，遵守项目规范）：

- `VisualTestContext::simulate_click(position)` 像素级点击（对话框内外点击关不关）；
- 断言选择跨 TextView、Shift-click 扩展、边缘自动滚动激活与滚动前进；
- `cx.set_reduce_motion(true)` 让动画一帧到位；
- 每切片附假数据驱动薄装置，切片测试不依赖其他切片（原则 6）。
