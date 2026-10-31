# CraftWave → GPUI 纯 Rust 全栈重写（方案罗盘·定稿）

> 状态：定稿（2026-10-31）。蓝本：Wake（github.com/iAmCorey/Wake，GPUI + gpui-component 的同型应用，精读结论已内化进各节）。
> 渲染规范独立成文：[rendering.md](rendering.md)。阶段 0 票在 [tasks/](tasks/)。

## 决策与背景

- **决策**：用 GPUI 纯 Rust 技术栈完全取代现有 Tauri + React + assistant-ui + TypeScript 技术栈。不经 nsh（评估结论：nsh+TSX 需维护两层语言与桥接，组件生态更小，无第三方生产案例；GPUI 有 Zed 大规模生产验证 + Wake 同型验证）。
- **蓝本依据**：Wake 精读覆盖了本应用全部关键面——聊天消息渲染、侧栏、⌘K、桌面骨架（菜单/键位/窗口/深浅色）、分发链（三平台 CI）、UI 测试装置。参考位置在各票内标注。
- **组件生态（已定稿，07 号票）**：**gpui-kit 0.8.1 门面**（crates.io，内部为 gpui-pre 家族 + gpui-base/gpui-component 0.8.1）+ gpui-fps 0.8.1（性能 HUD）+ tray-icon/global-hotkey（托盘/热键）。**不直接依赖 crates.io 的 `gpui` 0.2.2**（与 gpui-kit 内部 gpui-pre 家族是双实例，trait 互不相认）；**不 git 依赖 zed 仓库**（Wake 的做法，供应链风险大，已验证不需要）。API 面核对全部通过：TextView::markdown、AutoScroll、Textarea+submit_on_enter、TitleBar、SystemNotification、主题 token。**版本变更一律显式提交。**
- **Spike 结论（2026-10-31，七票全绿零失败）**：性能达标（3000 条混合消息 FRAME 5.5-12.6ms、GPU <2%、MEM ~130MB）、IME 全流程通过、托盘/热键共存实测通过、macOS 13.0 维持、引擎解耦无需 trait（剥壳+挪 AppState+剥 backup.rs，一天内）。关键坑与解法记录于 tasks/01-07 各票 Comments。
- **框架级未知项已清零**：托盘/全局热键走独立 crate（`tray-icon`/`global-hotkey`，框架无关，channel→`cx.spawn` 消费即可）；系统通知 gpui-component 原生；更新器查 gpui-kit Auto Update 页，覆盖则用，否则按自研骨架继续。

## 终态定义

单一 Rust workspace：`crates/engine`（现有引擎模块，零框架依赖）+ `crates/ui`（GPUI 应用）。三平台产物（macOS universal zip / Windows zip / Linux deb+tar.gz）。仓库内 Tauri、React、assistant-ui、TypeScript、vite/vitest/tailwind/biome 全部消失；提交检查链不含 pnpm 步骤。

## 定稿原则（含历次修订）

1. **完全取代**——单向迁移，无新旧共存架构；TS 逻辑资产按《迁移表》平移进 Rust 或确认废弃；
2. **分支重写，旧壳冻结**——重写在 `rewrite/gpui` 长分支进行（默认分支不直接提交）；阶段 1 末旧壳接新 engine 跑一次全绿作为搬移正确性基线，此后旧壳彻底冻结（只修挡住开发路径的问题）；
3. **切片验收不带可用性**——每个切片的验收 = 编译过 + 测试绿 + 规范符合，不要求端到端可用；中间态允许断、允许推倒重来；端到端联通集中在阶段 3 末的**集成周**；
4. **数据不做迁移**——历史会话随旧栈废弃；存储格式允许顺手改（有明确收益时）；wire 格式与 schema 镜像锁定保留，理由是单进程内部契约（防 UI 与引擎 schema 漂移），非历史兼容。切换后旧会话库原地不动，不读取；想留的对话内容在清除前用旧版手动导出，方案不做保障；
5. **每阶段末集成检查点**——全仓编译 + 全部测试通过（可构建性必须，可运行性不要求）；
6. **接口先行**（集成风险后置的缓解）——阶段 2 开工前定死三样：engine 对 UI 的 API 面（会话生命周期/帧流/树持久化）、帧快照 Rust 类型、主题 token 表；所有切片对着接口开发，接口变更走显式提交；每切片附假数据驱动的薄测试装置，切片测试不依赖其他切片完成。

## 阶段 0：Spike 工程验证（1–2 周）

> 独立目录，不碰主仓。性质是工程验证（性能基线、集成方式、pin 决策），非可行性 go/no-go。七张票：tasks/01–07。

步骤概览（详细步骤见各票）：Spike 工程搭建 → 托盘+全局热键集成 → 长会话性能基线 → Composer IME → macOS 13.0 部署目标确认 → engine 解耦预演（4 个 `tauri::` 文件的 trait 化改法）→ Spike 结论。

**退出条件**：02/04 通过、03 数字可接受、07 结论成文。

## 阶段 1：引擎搬移（数天–1 周，可与 0.5–0.7 并行）

> 目标：workspace 化，`crates/engine` 纯库 + 旧壳临时接线上绿（一次性验证，之后冻结）。

| # | 步骤 | 做什么 | 验收 |
|---|---|---|---|
| 1.1 | 建 workspace | 根 Cargo.toml workspace 化；新建 `crates/engine` | 空壳编译过 |
| 1.2 | 按模块搬移 | agent→backend→tools→storage→knowledge→mcp→git→providers→safety→util→mock_chat，每搬一个跑一次 `cargo test`（`MACOSX_DEPLOYMENT_TARGET=13.0`） | engine 全部既有测试绿 |
| 1.3 | 解耦 4 个文件 | 按 06 号票预演改 trait；顺带剥离挡路的死代码（如 storage 内旧格式升级逻辑） | `grep -r "tauri::" crates/engine` 零结果 |
| 1.4 | 旧壳临时接线 | src-tauri host 层改依赖 engine crate | 旧壳 cargo 全套绿（**仅此一次冒烟**，此后旧壳冻结） |

## 阶段 2：GPUI 骨架 + 对话流核心（人月级，最大阶段）

### 2.A 先行：接口与规范（半天）

- 定死接口三件套（原则 6）；
- 渲染规范已在 rendering.md，作为本阶段全部部件的验收依据。

### 2.B 应用骨架

| # | 步骤 | 做什么 | 验收 |
|---|---|---|---|
| 2.1 | main.rs 骨架 | Wake 式：开窗/几何记忆/最小尺寸/深浅色跟随/菜单（os_action）/键位（action 上下文）/on_reopen/on_app_quit/i18n | 空三栏窗，亮暗跟随、菜单齐全 |
| 2.2 | theme.rs | UX 方案 A 部分落语义 token：字阶偏移档、边框透明度、圆角层级、动效时长 | token 表成文，缩放全局生效 |
| 2.3 | UI 缩放 | `ui_zoom` 模式（Zpx × 档位），变高列表 remeasure | 缩放四档，traffic light 随动 |

### 2.C 对话流（渲染规范逐部件验收 + `#[gpui::test]`，测试放独立 `_test.rs`）

| # | 部件 | 要点 |
|---|---|---|
| 2.4 | 消息流底座 | ListState(Bottom,512) + 角色分发（四形态+过滤）；展开态 HashSet<行号> + splice 重测 |
| 2.5 | Markdown 正文 | TextView::markdown：heading 系数、段间距参数、代码块样式+复制、selectable、亮暗不闪色 |
| 2.6 | 链接分流 | resolve_link 判序照搬（盘符/行号后缀/字面优先/单次解码） |
| 2.7 | thinking 面板 | 折叠行（chevron 旋转+斜体摘要）+ 左导线纯文本展开；流式滚动最后一行 |
| 2.8 | 工具簇卡 | 每消息一簇；600 字符+完整复制；宽度自适应格数；失败 badge |
| 2.9 | 图片 | 三态（Ready/Unsupported/Omitted）；按宽高比约束；zoom |
| 2.10 | Composer | Input + Enter/Shift+Enter + @ 补全（at-refs 平移）+ 附件 |
| 2.11 | 帧流 | engine start_chat → 帧消费 → 64ms 节流（frames 逻辑 Rust 化）；abort 桥 cancel。**该切片自身完成即可，端到端联通在集成周** |
| 2.12 | UI 测试装置 | 假数据驱动，各切片共用模式 |

**退出条件**：全部部件按渲染规范验收 + 集成检查点绿。

## 阶段 3：功能切片补齐（人月级）+ 集成周

> 对照《功能需求文档》FR 清单逐项映射，每切片一票，顺序按依赖排。

| # | 切片 | 蓝本/说明 |
|---|---|---|
| 3.1 | 多会话 + 侧栏 | sidebar_groups / sidebar_row / 行状态槽 |
| 3.2 | ⌘K 与搜索 | SearchDelegate 模式；FTS + 跳转高亮（负 margin 手法） |
| 3.3 | 会话树与分支 | save_session_tree 对接、活动路径渲染、分支切换（单分支隐藏）。无旧数据收养语义，只有本会话重载 |
| 3.4 | 权限与确认 | 权限档、Ask 面板、外部授权、命令确认卡 |
| 3.5 | 面板族 | Quota / Changes(diff) / Inspector / Artifacts / HtmlPreview，逐票定布局 |
| 3.6 | 子代理 | 列表 + 详情跳转（open_child_button 蓝本） |
| 3.7 | 知识/MCP/技能 | 对应现有功能面 |
| 3.8 | 会话内查找 | Cmd+F + timeline 高亮 |
| 3.9 | 用量图表 | div 拼格子（Insights 模式），usage-stats Rust 化 |

**集成周（3 末）**：接通帧流 → 历史重载 → 全功能联通；此时第一次要求可用，dogfood 找茬开始并持续到切换。

## 阶段 4：桌面集成（1–2 周）

| # | 步骤 | 说明 |
|---|---|---|
| 4.1 | 托盘 | 02 号票方案落地；托盘菜单（显示主窗/退出） |
| 4.2 | 全局热键 | FR-13-11；global-hotkey |
| 4.3 | 系统通知 | gpui-component SystemNotification（.system() + 点击路由）；set_app_identity；开发期用应用内 toast（macOS 打包态才投递） |
| 4.4 | 单实例 | 自研：锁文件/socket + 二次启动通知首实例开窗 |
| 4.5 | 更新器 | 先查 gpui-kit Auto Update：覆盖 FR-16-03 → 用之；否则按 33 号票自研骨架继续（决策点） |
| 4.6 | 退出确认 | 活跃 run 拦退出——Wake should_close 模式 |

## 阶段 5：切换与清除（1 周内）

| # | 步骤 | 说明 |
|---|---|---|
| 5.1 | CI 三平台 | Wake release.yml 为模板；三端原生 runner（shader 不能交叉）；macOS universal+ad-hoc、Windows zip（windows_subsystem）、Linux deb+tar.gz |
| 5.2 | 功能覆盖审计 | 对照 FR 清单：新壳功能面 ≥ 旧壳，清单签字后方可切换 |
| 5.3 | 切换点（cutover） | `rewrite/gpui` 合入 main；dogfood 一轮真实工作流 |
| 5.4 | 清除清单 | 删 `ui/` 整目录；删 src-tauri 的 tauri 依赖/插件/host；删 package.json 与 pnpm/vite/vitest/tailwind/biome 工具链；删 AGENTS.md assistant-ui 章节并重写为 GPUI 规范（依赖 pin 策略、检查链、_test.rs 规范、Wake 参照索引） |
| 5.5 | 性能与文档 | NFR-03 口径重跑对照；ADR 补「为何 GPUI 完全取代 Tauri+React」；技术设计文档修订 |

**切换门槛（完全取代的验收）**：仓库 `grep -r "tauri\|assistant-ui\|react"` 仅历史文档命中；检查链无 pnpm 步骤；日常工作流完全在新壳完成。

## TS 资产迁移表

| 资产 | 去向 | 说明 |
|---|---|---|
| runtime.ts 的 64ms FrameBuffer/节流 | 平移进 Rust | 帧聚合器进 engine 侧 |
| frames.ts（帧→part 合并） | 平移进 Rust | 无跨语言边界，合并进引擎 |
| tree-persistence.ts / toWire() | 废弃 | wire 权威在 Rust；无跨语言转换 |
| at-refs.ts | 平移进 Rust | composer 调用 |
| run-metrics / usage-stats / export | 平移进 Rust | 记账、统计、导出住引擎 |
| session-prefs / thinking-level / permission-tier / session-model | 并入 engine | 会话偏好 Rust 侧持久化 |
| composer-keys.ts | 重写 | GPUI KeyBinding 原生承载 |
| zustand 8 个 store | 废弃 | GPUI Entity + 事件承载 |
| ToolCards.tsx | 重写为簇卡 | 形态按渲染规范 |
| scroll-anchor / cancel-bridge / event-bridge | 废弃 | gpui 列表滚动、直接调用、同进程事件 |
| contract.test.ts | 转 cargo test | 引擎内 schema 对照 |
| i18n 词表 | 平移进 Rust | Wake i18n.rs 模式 |
| @tauri-apps/*、assistant-ui、streamdown、zustand、lucide-react、tailwind/vite/vitest | 删除 | 由 gpui-component / TextView / Entity / icons / 原生样式与 #[gpui::test] 替代 |

## 贯穿纪律

- **提交**：切片独立分支、Conventional Commits、重构与功能不混提交；依赖升级一律 pin rev 显式提交；
- **双壳期功能流向**：新需求只进 engine，旧壳只修挡路问题；
- **测试**：引擎测试沿既有 `_test.rs` 独立文件约定；UI 行为用 `#[gpui::test]` 装置（同样独立 `_test.rs`）；
- **规范资产**：`docs/ZCode桌面端UI-UX优化方案.md` 已升格为渲染规范验收条目（见 rendering.md）。
