# 主题 token 表（gpui-rewrite 阶段 2.A「接口三件套」之三）

> 定稿（2026-10-31）。来源：`ui/src/styles.css`（现行主题定义，亮/暗两套定稿值）、
> `docs/ZCode桌面端UI-UX优化方案.md` 方案 A（A1 字阶 / A2 透明度边框 / A3 圆角层级 /
> A6 动效 token / A7 图标档）、`rendering.md`（行高与 Markdown 系数）。
> 消费方：阶段 2.B 的 `ui/theme.rs`——本文是「直接照抄实现」清单，精确到数值，
> 不留「待定」；改任何值走显式提交（spec 定稿原则 6）。

## 1. 消费方式与映射总则

- **颜色类 token** 优先映射 gpui-component 0.8.1（gpui-kit 门面内）`ActiveTheme`
  的现成槽位（`ThemeColor` 字段，gpui-base `theme.rs` 的 `Theme: Global`）；
  槽位清单核对日期 2026-10-31（registry 源码 `theme/theme_color.rs`）。
  映射 = 阶段 2.B 启动时按本文值构造 `ThemeColor`（亮/暗两套）注入 Global。
- **ActiveTheme 没有的槽位**（三档背景面、前景淡档、dim 等）落「自建 Global
  主题表」：`CraftwaveTheme: Global`（阶段 2.B theme.rs 定义，全部字段见 §3
  标注「自建」的行）。
- **字号/间距/圆角/动效/行高** 是常量档（非颜色），全部落自建表或 `ui/theme.rs`
  常量；字号类必须经 §2 缩放公式取值，禁止写死 px。

## 2. 缩放规则（`ui_zoom`，spec 2.3）

**公式：`Z(px) = round(14 × zoom)`**——Z 为全局字号基准，`zoom` 是缩放档位。

**四档缩放**（spec 2.3「缩放四档」，端点落在 ZCode 滑杆 12–20px 范围内）：

| 档位 | zoom | Z（基准） |
|---|---|---|
| 小 | 0.85 | 12px |
| 标准（缺省） | 1.00 | 14px |
| 大 | 1.15 | 16px |
| 特大 | 1.30 | 18px |

**字号档公式：`font(档) = round(Z + offset)`**——offset 为字阶偏移（§5），
所有 UI 字号（含设置页标题、thinking 展开小字、clip 截断预算等）必须以本
公式表达，实现里只存 offset 常量；运行时改 zoom（Z 值）全局生效，变高列表
随之 remeasure（spec 2.3）。traffic light 位置随 zoom 档联动由壳负责。

**Markdown 标题系数**（rendering.md 定稿，乘法体系，同样以 Z 为基）：
`h1 = 1.45Z、h2 = 1.28Z、h3 = 1.14Z、h4 = 1.05Z、h5+ = Z`（round 取整）。
标准档（Z=14）实值：h1=20、h2=18、h3=16、h4=15、正文=14。

## 3. 颜色 token 表

值格式：hex（`#rrggbb`）或 rgba（含 alpha）；「映射」列 = gpui-component
`ThemeColor` 槽位名（横线转下划线），**自建** = `CraftwaveTheme` Global 字段。

### 3.1 面与前景

| token | 亮色 | 暗色 | 映射 | 说明（消费面） |
|---|---|---|---|---|
| background | `#f3f3f2` | `#272728` | `background` | 窗口整体底 / 侧栏底（亮暗均与侧栏一体） |
| foreground | `#1f2328` | `#e8e8e9` | `foreground` | 主前景；**边框/悬停透明度混合的基色**（§4） |
| main_surface | `#f8f8f8` | `#161616` | **自建** | 中部会话区底（比外围浅/深一档的定稿层次） |
| sidebar_surface | `#f3f3f2` | `#272728` | `sidebar` | 侧栏底（=background 同值，槽位独立以便分化） |
| composer_surface | `#ffffff` | `#272728` | **自建** | composer 卡底（亮=卡白，暗=同侧栏 #272728） |
| card | `#ffffff` | `#161616` | `popover` | 弹框/卡片底（gpui-component 无 card 槽，用 popover） |
| muted | `#eef0f3` | `#262626` | `muted` | 次级底：**代码块底色**、表格头、user 气泡底 |
| muted_foreground | `#57606a` | `#9b9b9d` | `muted_foreground` | 次级前景（=cw-text-2，同值双名，实现取 muted_foreground） |
| foreground_subtle | fg @60% | fg @60% | **自建** | 项目名等淡一档前景 |
| foreground_subtlest | fg @40% | fg @30% | **自建** | 最淡前景（组标题、列表 marker；暗色 30% 是 ZCode 同款差异） |
| dim | `#8a939e` | `#7c7c7e` | **自建** | 设置组标题 / 次级图标（cw-dim） |
| input | `#cfd5db` | `#3f3f41` | `input` | 输入框边 / 虚线按钮边 |
| selected | `rgba(13,13,13,0.05)` | `rgba(255,255,255,0.1)` | `list_hover`（侧栏选中态另用 `sidebar_accent`） | 列表/侧栏悬停与选中底 |

### 3.2 强调与语义

| token | 亮色 | 暗色 | 映射 | 说明 |
|---|---|---|---|---|
| primary | `#1f2328` | `#e8e8e9` | `primary`（primary_foreground：`#ffffff` / `#191919`） | 主按钮色（前景同源的黑/白反转，非蓝色） |
| accent | `#e8eaee` | `#313133` | `accent`（accent_foreground：`#1f2328` / `#e8e8e9`） | 选中卡底 / 高亮面（cw-highlight 同值） |
| focus（强调蓝） | `#2563eb` | `#3b82f6` | `ring` + `link`（同值双槽） | 唯一强调色：聚焦环、脏点、链接、选中态（cw-dot/cw-focus） |
| danger | `#dc2626` | `#ef4444` | `danger` | 错误/失败 badge/危险按钮（destructive；cw-err 另值见下） |
| danger_text | `#d03066` | `#f87171` | **自建** | 表单错误红 / diff 统计红（cw-err，亮色偏品红的定稿值） |
| warning | `#b45309` | `#f59e0b` | `warning`（warning_foreground：`#ffffff` / `#17191c`） | 警告 |
| success | `#16a34a` | `#22c55e` | `success` | 成功 / **diff 统计绿**（diff-added/diff-removed 取语义槽） |
| diff_removed | `#dc2626` | `#ef4444` | `danger`（同 danger 值） | diff 红（=danger，独立 token 名便于语义追溯） |

### 3.3 代码块与特殊面

| token | 亮色 | 暗色 | 映射 | 说明 |
|---|---|---|---|---|
| code_block_bg | `#eef0f3`（=muted） | `#262626`（=muted） | `muted` | Markdown 代码块底（rendering.md：muted 底 + 1px border + 圆角） |
| code_inline_bg | muted @50% | muted @50% | `muted` 派生（alpha 0.5） | 行内码半透明底（B2 验收条目） |
| selection | 系统色 | 系统色 | `selection` | 文本选区（gpui 原生选区色，不覆盖即系统蓝；如需定制随 focus 蓝） |
| caret | 跟随 foreground | 跟随 foreground | `caret` | 输入光标 |
| title_bar | `#f3f3f2` | `#272728` | `title_bar`（border：`title_bar_border`） | 自绘标题栏底（=background） |

## 4. 透明度混合规则（A2，边框与淡档的生成式）

边框与淡档**不做两套实色**，全部由 foreground 透明度混合生成（亮暗一套
alpha 逻辑，前景反相自动成立）：

| 规则 | alpha | 消费面 |
|---|---|---|
| border = foreground @ **10%** | 0.10 | 全部 1px 边框、分隔线、滚动条 thumb |
| border_hover = foreground @ **15%** | 0.15 | 悬停边框（choice-card hover 等） |
| hover_face = foreground @ **5%** | 0.05 | 亮色悬停面（selected 亮色即此值） |
| hover_face（暗色）= foreground @ **10%** | 0.10 | 暗色悬停面（selected 暗色） |

gpui 里以 `foreground.with_alpha(0.10)` 形态落 theme.rs（Hsla alpha 通道）；
`border` 槽位注入时即按本规则混好，组件不自行计算。

## 5. 字号档（八档，全部经 §2 公式）

| 档名 | offset | Z=12（小） | Z=14（标准） | Z=16（大） | Z=18（特大） | 消费面 |
|---|---|---|---|---|---|---|
| ui-2xs | −5 | 7px | 9px | 11px | 13px | 徽标极小字（当前无消费方，保留体系完整） |
| ui-xs | −4 | 8px | 10px | 12px | 14px | 同上（保留） |
| ui-sm | −2 | 10px | 12px | 14px | 16px | 辅助说明、代码块内、组标题 |
| ui-caption | −1 | 11px | 13px | 15px | 17px | 时间戳、凭据 radio、表单 hint |
| ui-base | 0 | 12px | **14px** | 16px | 18px | 正文基准（消息正文、设置行） |
| ui-lg | +2 | 14px | 16px | 18px | 20px | Markdown h2、section 标题 |
| ui-xl | +4 | 16px | 18px | 20px | 22px | Markdown h1（旧栈 h1 档；新壳 h1 走 §2 系数 1.45Z） |
| ui-settings-title | +10 | 22px | 24px | 26px | 28px | 设置页操作条标题 |

**thinking 展开小字**：`Z − 2.5`（标准档 11.5px，rendering.md 钉值）——按
公式表达为 offset −2.5，theme.rs 以 `Z as f32 - 2.5` 取值，round 渲染。

## 6. 行高档（八档，固定倍数，不随 zoom 变）

| 档名 | 值 | 消费面 |
|---|---|---|
| leading-tight | 1.3 | 设置页大标题 |
| leading-heading | 1.4 | Markdown 标题 |
| leading-row | 1.45 | 设置行描述 / section 标题 |
| leading-code | 1.5 | 代码块 |
| leading-thinking | 1.7 | thinking 展开区（左导线纯文本） |
| leading-prose | 1.75 | 旧栈 Markdown 容器（对照基准；新壳正文用下档） |
| leading-user | 1.85 | User 气泡正文（rendering.md 钉值） |
| leading-assistant | 1.9 | Assistant 正文（rendering.md 钉值） |

## 7. 间距档（px 常量，4 基为主）

| 档名 | 值 | 消费面（现行 UI 实测值照抄） |
|---|---|---|
| space-1 | 4 | 文案与控件小缝（row-copy gap、chip 间） |
| space-2 | 6 | 图标与文字、导航头 gap |
| space-3 | 8 | 面板 padding 基数、按钮组 gap、行卡间距 |
| space-4 | 10 | 设置行纵向 padding、端点块 padding |
| space-5 | 12 | 卡片网格 gap、代码块体 padding、操作条 gap |
| space-6 | 16 | Markdown 段落 margin、列表行 gap、代码块 margin |
| space-7 | 20 | 设置卡横向 padding、列表缩进 |
| space-8 | 24 | 设置行控件 gap、弹框 mask padding、组间分隔 |
| space-9 | 28 | 离开确认弹框 padding |
| space-10 | 32 | 设置页体横向 padding、操作条 padding |
| space-11 | 36 | 设置页体顶部 padding |
| space-12 | 48 | 窄容器页体底部 padding |
| space-13 | 72 | 设置页体底部 padding（滚动呼吸） |

阅读轴定值（rendering.md，非档位、钉死）：User 气泡 `max_w 540px`；
全消息阅读区 `READER_MAX_WIDTH` 居中；首条 `pt` / 末条 `pb` 呼吸边距；
工具参数摘要预留格数 = `(阅读区宽 − 预留px) / 等宽字符宽`。

## 8. 圆角档（A3，随嵌套递减，禁止跨级混用）

| 档名 | 值 | 白名单/消费面 |
|---|---|---|
| rounded-2xl | 16px | **仅白名单**：composer 输入壳、弹框外壳（dialog/toast/状态浮板） |
| rounded-xl | 12px | 首层面板：会话大圆角卡、设置 section 卡、三选一卡片 |
| rounded-lg | 8px | 菜单/下拉外壳、行卡、代码块、虚线按钮 |
| rounded-md | 6px | 菜单项、按钮、行内小件 |
| rounded-sm | 4px | 行内最小件（badge、marker） |
| full | 9999px | 单选圆点、滚动条 thumb、脏点 |

## 9. 动效时长档（A6 + B3/B4）

**曲线**（全部过渡统一）：`ease_cw = cubic-bezier(0.22, 0.61, 0.36, 1)`；
gpui 以 `ease_out_quart` 近似或自写贝塞尔（阶段 2.B 落常量 `EASE_CW`）。

| 档名 | 时长 | 消费面 |
|---|---|---|
| duration-fast | 120ms | 浮层入场（淡入 + 95% 缩放上浮）、hover 过渡 |
| duration-base | 160ms | 通用状态过渡（卡片边框/底色） |
| duration-enter | 200ms | 面板/抽屉进入 |
| duration-rise | 260ms | 空状态建议 chips 瀑布入场（逐个 delay） |

**循环动画**（单列，非过渡档）：工具卡运行态扫光 1.2s/循环（linear）；
运行态文案流光 4s/循环（300% 背景往返）；标题走马灯 40px/s（hover 1s
后启动，副本间隔 24px）。

**reduce_motion**：全局降级开关（gpui 测试 `cx.set_reduce_motion(true)`
一帧到位）——开启时全部过渡/循环动画时长归零，保留可读静态状态
（流光文案回退实色前景）。

## 10. 字体与图标档

| token | 值 | 说明 |
|---|---|---|
| font-ui | 系统栈：macOS SF / Windows 雅黑，中文回落 PingFang SC / Microsoft YaHei | UI 正文；GPUI 用 `.system_ui` 族 + 中文回落声明 |
| font-mono | ui-monospace / SFMono / Menlo / Consolas + 中文回落 | 代码/路径/徽标（工具参数摘要、代码块） |
| icon-sm / icon-md / icon-lg / icon-xl | 12 / 14 / **16（默认）** / 20px | 图标尺寸档（A7）；次级图标用 dim 色降档（lucide 线条统一 1px 的决策由图标源承载） |

## 11. 阶段 2.B theme.rs 落地清单（照抄单）

1. `CraftwaveTheme: Global`：§3 全部「自建」字段 + §2 zoom 状态
   （`zoom: f32`，setter 触发全局字号失效与列表 remeasure）；
2. `fn theme_colors(mode) -> ThemeColor`：§3 亮/暗两套按 gpui-component
   `ThemeColor` 逐槽注入（未列槽位保持 gpui-component 默认或由派生规则
   自动补：`*_hover/*_active/*_foreground` 族 gpui-component 自算回退）；
3. 透明度规则 §4 作为 `border`/淡档的生成函数（单点实现，组件不手混）；
4. 字号/行高/间距/圆角/动效常量按 §5–§9 落 `pub const`（字号只存 offset，
   运行时经 `Z = round(14 × zoom)` 换算）；
5. Markdown 定制四点（rendering.md）用 §2 标题系数 + §3 code_block_bg +
   §8 rounded-lg + §6 leading-prose；
6. 验收对照：亮暗切换不闪色（主题写进元素 id）、缩放四档全局生效、
   traffic light 随动（spec 2.2/2.3）。
