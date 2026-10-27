# CraftWave 工作交接（HandOff）

> 生成于 2026-10-28（同日三次更新）。追加：设置界面重写（照抄 CodeWave + shadcn 手写基件，`29d4f90`）与 GLM Coding Plan 端点排障（用户配置修复）。
> 规则：本文件只做索引与状态快照，细节一律引用票文件/文档/提交，不复述。

## 一、必读顺序（新会话开场）

1. `AGENTS.md`——提交纪律、检查链、工具链（有本地补充再读 `AGENTS.local.md`）。
2. `docs/specs/m1-mvp/map.md`——wayfinder 地图：目标、决策索引（Decisions so far）、剩余雾区（Not yet specified）。
3. `docs/技术设计文档.md`——开发直接依据（P0 字段级）；附录 C 为未决问题清单。
4. 票目录 `docs/specs/m1-mvp/tasks/`——一票一文件，Answer 节是各票结论。

## 二、当前状态快照（main = `b7ee963`；15 号票工作在分支 `feat/run-persistence` 待用户点头合并）

- **地图进度**：01–15 号票全部关闭。四张 spike + 三张实现切片 + 核心链路（13）+ 工具系统接线（14）+ **run 生命周期落盘与历史恢复（15，本次）**。
- **测试基线**：Rust 210 lib + 集成 25（另 1 例真钥匙串冒烟 `#[ignore]` 已随 keyring 废除清理面）；前端 vitest 89 例；biome/tsc 全绿。
- **已通的能力**：在 14 号之上，15 号新增——§7.3 时机表全量落盘（RunRecorder 帧观察器：user/turn 文本/工具批/runMarker 逐点 fsync）、sessions/index.json + `list_sessions`（#14）、toolres 边车 + `load_tool_outcomes`（#29）、崩溃恢复读时修复（canonical 形态判定，只认 `{runId}:` 前缀条目）、前端启动收养（load→import）、发送前 save 协议、分支/编辑/重生成 UI（官方原语操作条）、工具卡边车恢复（折叠 detail）。**canonical 双写协议定稿**（文档 v2.6）：条目 id 两侧同源（`{runId}:{turn}`/`.tools`/`:marker`）、运行态节点不进 save 载荷（marker 引用改写）、E_SESSION_REV load 校正重试。SessionStore Arc 化（AppState.store 与命令层/RunRecorder 共锁表）；start_chat 的 ToolsContext/边车根统一 `app.store.root()`（测试不再污染真实 `~/.craftwave`）。
- **同日追加（v2.5 用户决策）**：① **keyring 已废除**——API Key 明文存 config.json（0600，intent.json 协议移除）；② dev 模式原始错误输出双通道。
- **设置界面重写**（`29d4f90` 已并回）：shadcn 风格基件 12 件（零新依赖）+ 亮暗令牌；设置页照抄 CodeWave 布局；供应商页完整复刻 ProvidersPanel 交互。未接线：MCP/技能/写入后检查/工作区与智能体四页占位待票；暗色主题仅编辑落盘。
- **GLM Coding Plan 500 排障**（用户配置修复，无代码改动）：端点已改 `https://open.bigmodel.cn/api/coding/paas/v4` + `glm-5.3-flash`（三协议分流详见五）。

## 三、下一步（按地图雾区顺序）

1. **多会话与项目**：create/delete/rename_session、临时/项目会话双形态、项目注册表、workspace 读写与路径搜索命令（#27/#28）；顺带收敛 15 号票 shortcut（index 2000 条归档、崩溃 run 尾条 incomplete 回填）。
2. **桌面体验**（三栏/托盘/快捷键/折叠分组 §10.4；工具卡专属排版在此票打磨）→ **打包验收**（C-8 安全复判在此毕业成票）。
3. 小项：rate 的 TextDelta 自算、steer/followUp、错误分类 §11.3、ts-rs 引入（C-2 后半）。

## 四、工作方式（本图约定）

- **wayfinder 模式**：一票一会话；动工先 `Status: claimed`；完成写 `## Answer` + `resolved` + 地图 Decisions 追加一行；新决策进雾区按 tracker 约定立票。
- **并行 worktree 模式**（实证有效）：`git worktree add ../<name> -b <branch> main` + 后台子代理各领一票；合并前先在 scratch 分支试合并并跑全链；多代理改同一文档时各自只动自己章节/行。
- **grilling 票**（HITL）留在主会话与用户做，子代理不得代答。

## 五、环境与坑（新设备必看）

- **前端检查从仓库根跑**：`pnpm typecheck/test/lint` 定义在根 package.json（lint 只在根，Biome 在根 devDependencies）；在 `ui/` 下跑 `pnpm lint` 会命中 PATH 上无关同名工具。
- 检查链：`src-tauri/` 下 `cargo fmt --check && cargo build && cargo clippy --all-targets -- -D warnings && cargo test`；根下前端三项。`MACOSX_DEPLOYMENT_TARGET=13.0` 已在 `.cargo/config.toml [env]`。
- 工具链：`rust-toolchain.toml` 钉 1.97.1（rig MSRV）；crates 走 rsproxy 镜像；GitHub 抖动时命令前缀 `https_proxy=http://127.0.0.1:7890`。
- **测试方法命名一律英文**（snake_case，2026-10-27 用户指令，全仓已统一）；断言消息保持中文。
- **rig 非流式坑**：`Model::call`（单次补全，suggest 用）期待 `chat.completion` JSON，mock_openai 已支持流式/非流式两形态——新写后台小调用测试直接复用。
- **assistant-ui 0.15.31 两个 API 坑**（15 号票实证）：① `BranchPickerPrimitive.Count` 零 props（`Props = Record<string, never>`），样式要套外层 span；② `ThreadMessage` **不带 parentId**（api-surface 查证），活动路径的父子关系只能按传入线性序推导——15 号票 save 协议（payload parentId 线性链）建立在这个前提上。
- **真实帧序是 Usage 先于工具执行**（run_loop：throttler.flush → usage 帧 → 才进 CallTools）——RunRecorder 落盘顺序依赖它（文本条先落、工具条后落），写帧序测试时别把 ToolResult 放在 Usage 前。
- **前端 vi.mock("@/ipc/client") 要补全依赖链导出**：session.ts 引 toWire（runtime.ts）→ runtime.ts 又引 startChat/cancelRun 等，mock 缺导出直接整文件报错（不只是缺的那个函数）。
- 新依赖：`anstream`（ANSI 清洗，Rust）、`@types/json-schema`（ui devDep，toolkit 参数类型）；`sha2` 全仓统一 **0.11**，勿回 0.10。
- **智谱 GLM Coding Plan 端点三协议分流**（官方 quick-start）：Anthropic 协议 `…/api/anthropic`；OpenAI Chat Completions 协议 `…/api/coding/paas/v4`；OpenAI Responses 协议 `…/api/v1`。rig 的 openai_chat 方言拼 `{baseUrl}/chat/completions`，端点协议不匹配时服务端回 5xx/403 而非明确报错——排障先拿用户 Key curl 三个端点对照。
- **前端纯浏览器黑屏是既有行为**：主入口与 `?mirror=1` 在无 Tauri IPC 的浏览器里都不渲染（React 挂载后整树卸载）；UI 视觉调试用临时 `preview.html` 入口预置 store 数据绕开 IPC（用完即删，勿提交）。
- 测试不进源码文件：Rust 用 `*_test.rs` / `tests/`，前端用 `*.test.ts(x)`。
- 提交：Conventional Commits 中文主题、按路径精确 add（禁 `add -A`）、禁 `--no-verify`、重构与功能分开；代码合并需用户点头，tracker/文档记账类提交按既有实践直提 main。

## 六、建议技能（新会话按需调用）

- `wayfinder`——以地图继续找路/开工票（Work through the map 模式，传 map 路径）。
- `implement` / `implement-spec`——实现票执行。
- `grilling` + `domain-modeling`——决策票（HITL）。
- `research`——外部调研票。
- `handoff`——下次交接（注意：本次仍按用户要求写到仓库根 `HandOff.md`，技能默认是系统临时目录）。

## 七、敏感信息

API Key 明文存 `~/.craftwave/config.json`（v2.5 用户决策废除 keyring，文件权限 0600）；**仓库内不含任何密钥**。用户本机已配 GLM Coding Plan Key，排障时可用其 curl 但不得写入仓库/日志。
