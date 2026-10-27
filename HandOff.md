# CraftWave 工作交接（HandOff）

> 生成于 2026-10-28（第四次更新）。追加：15 号票（run 生命周期落盘）与 16 号票（多会话与项目）合入 main、多 Tab 启动报错修复。
> 规则：本文件只做索引与状态快照，细节一律引用票文件/文档/提交，不复述。

## 一、必读顺序（新会话开场）

1. `AGENTS.md`——提交纪律、检查链、工具链（有本地补充再读 `AGENTS.local.md`）。
2. `docs/specs/m1-mvp/map.md`——wayfinder 地图：目标、决策索引（Decisions so far）、剩余雾区（Not yet specified）。
3. `docs/技术设计文档.md`——开发直接依据（P0 字段级，v2.7）；附录 C 为未决问题清单。
4. 票目录 `docs/specs/m1-mvp/tasks/`——一票一文件，Answer 节是各票结论。

## 二、当前状态快照（main = `7787dbf`，含 15/16 号票全部工作；本地领先 origin/main 未 push）

- **地图进度**：01–16 号票全部关闭。四张 spike + 三张实现切片 + 核心链路（13）+ 工具系统接线（14）+ run 生命周期落盘（15）+ **多会话与项目（16，本次）**。
- **测试基线**：Rust 232 lib + 25 集成全绿（clippy/build 干净）；前端 vitest 99 例；tsc/Biome 全绿；manifest 双向契约 26 命令。
- **15 号票**（`e53d9b1` + retro 修复 `d9a87b3`）：§7.3 时机表全量落盘、index.json、toolres 边车、崩溃恢复、前端启动收养/发送前 save/分支编辑 UI；**canonical 双写协议定稿**（文档 v2.6）。
- **16 号票**（`0d9251c`，修复 `7787dbf`）：项目注册表（一目录一项目）、会话 CRUD（删除项目会话转临时）、workspace 只读（#27/#28）、ui_state 域（#30/#31）、select_dir（#5，rfd）、start_chat roots 按项目泛化、**多 Tab 前端**（keep-mounted 每会话独立 runtime + 左栏会话树 + ui-state 布局恢复）、persist/metrics/ask 按 sessionId 分桶、index 2000 条归档（15 号票 shortcut 收敛）；#15 load_session 不单独注册（文档 v2.7）。
- **分支记录**：`feat/run-persistence`、`feat/sessions-projects` 均 fast-forward 并入 main，未删除；未 push（用户未要求）。

## 三、下一步（按地图雾区顺序）

1. **桌面体验票**：三栏布局、Composer、自绘标题栏、托盘+后台运行+系统通知、快捷键体系、主题、单实例、滚动跟随（§10.4）；16 号票 shortcut 在此收敛——**草稿持久化**（§10.6）、**项目改名入口**、工具卡专属排版、崩溃 run 尾条 incomplete 回填（15 号票 shortcut）。
2. **三平台打包与 1 周真实使用验收**：打包矩阵（dmg/nsis/AppImage+deb）、验收剧本；**C-8（临时会话 roots 边界）安全复判在此毕业成票**。
3. 小项：rate 的 TextDelta 自算、steer/followUp、错误分类 §11.3、ts-rs 引入（C-2 后半）、session_prefs 域命令（#24/#25）、pending_intent UI 接线、设置页与预设打磨。

## 四、工作方式（本图约定）

- **wayfinder 模式**：一票一会话；动工先 `Status: claimed`；完成写 `## Answer` + `resolved` + 地图 Decisions 追加一行；新决策进雾区按 tracker 约定立票。
- **并行 worktree 模式**（实证有效）：`git worktree add ../<name> -b <branch> main` + 后台子代理各领一票；合并前先在 scratch 分支试合并并跑全链；多代理改同一文档时各自只动自己章节/行。
- **grilling 票**（HITL）留在主会话与用户做，子代理不得代答。

## 五、环境与坑（新设备必看）

- **前端检查从仓库根跑**：`pnpm typecheck/test/lint` 定义在根 package.json（lint 只在根，Biome 在根 devDependencies）；在 `ui/` 下跑 `pnpm lint` 会命中 PATH 上无关同名工具。
- 检查链：`src-tauri/` 下 `cargo fmt --check && cargo build && cargo clippy --all-targets -- -D warnings && cargo test`；根下前端三项。`MACOSX_DEPLOYMENT_TARGET=13.0` 已在 `.cargo/config.toml [env]`。
- 工具链：`rust-toolchain.toml` 钉 1.97.1（rig MSRV）；crates 走 rsproxy 镜像；GitHub 抖动时命令前缀 `https_proxy=http://127.0.0.1:7890`。
- **多 Tab 架构（16 号票定式）**：每 Tab 一个 `SessionTab`（keep-mounted，非活跃 `display:none`）+ 每会话独立 `sessionAdapter(id)` runtime 与 **独立 `AssistantRuntimeProvider`**（config 共用模块级 `cwAuiConfig`）——任何用到 Thread 原语/`useAuiState` 的组件必须在 Provider 内，否则启动即抛「requires an AuiProvider」（本次踩坑）。
- **会话相关 store 一律按 sessionId 分桶**：persist（`scopes`）、run-metrics（`bySession`，`useSessionMetrics` 返回稳定零值引用防渲染循环）、ask（`bySession`+`hints`）；ask:closed 载荷无 sessionId，按 askId 全桶匹配关闭。组件内取所属会话用 `useSessionScope()`（`chat/session-scope.ts`）。
- **@ 补全路径口径**：项目主目录下条目返回相对路径（read 工具解析基准），数据目录专属条目绝对路径；macOS `/var` ↔ `/private/var` 符号链接——相对路径判定必须对 canonicalize 后的根做 `strip_prefix`。
- **rfd select_dir**：同步命令在主线程执行（macOS NSOpenPanel 线程约束），不可改 async。
- **vitest**：`toBe` 无第二参数（失败消息写注释）；**Biome**：`obj["字面量"]`→`obj.键`；`role="button"` 的 div 会要求改真按钮（button 不嵌 button——操作按钮放行容器做兄弟）。
- **Rust 测试**：归档类行序勿依赖 HashSet 迭代序（按时间序显式写入）；大索引测试直接构造 JSON 一次落盘再单次 upsert 触发归档（逐条 upsert 是 O(n²)，27s→2s）。
- **测试方法命名一律英文**（snake_case，2026-10-27 用户指令，全仓已统一）；断言消息保持中文。
- **rig 非流式坑**：`Model::call`（单次补全，suggest 用）期待 `chat.completion` JSON，mock_openai 已支持流式/非流式两形态。
- **assistant-ui 0.15.31 API 坑**：① `BranchPickerPrimitive.Count` 零 props（样式套外层 span）；② `ThreadMessage` 不带 parentId（活动路径父子关系按传入线性序推导——save 协议建立在此前提上）。
- **真实帧序是 Usage 先于工具执行**——RunRecorder 落盘顺序依赖它，写帧序测试别把 ToolResult 放 Usage 前。
- **前端 vi.mock("@/ipc/client") 要补全依赖链导出**（session.ts → runtime.ts → startChat 等），mock 缺导出整文件报错。
- 新依赖：`anstream`（ANSI 清洗）、`rfd`（select_dir，同步命令）、`@types/json-schema`（ui devDep）；`sha2` 全仓统一 **0.11**。
- **智谱 GLM Coding Plan 端点三协议分流**：Anthropic `…/api/anthropic`；OpenAI Chat Completions `…/api/coding/paas/v4`；OpenAI Responses `…/api/v1`。rig 的 openai_chat 拼 `{baseUrl}/chat/completions`，协议不匹配时服务端回 5xx/403 而非明确报错。
- **前端纯浏览器黑屏是既有行为**（无 Tauri IPC 不渲染）；UI 视觉调试用临时 `preview.html` 入口预置 store 数据绕开 IPC（用完即删，勿提交）。
- 测试不进源码文件：Rust 用 `*_test.rs` / `tests/`，前端用 `*.test.ts(x)`。
- 提交：Conventional Commits 中文主题、按路径精确 add（禁 `add -A`）、禁 `--no-verify`、重构与功能分开；代码合并需用户点头，tracker/文档记账类提交按既有实践直提 main。

## 六、建议技能（新会话按需调用）

- `wayfinder`——以地图继续找路/开工票（Work through the map 模式，传 map 路径）。
- `implement` / `implement-spec`——实现票执行。
- `grilling` + `domain-modeling`——决策票（HITL）。
- `research`——外部调研票。
- `handoff`——下次交接（本次仍按用户要求写到仓库根 `HandOff.md`，技能默认是系统临时目录）。

## 七、敏感信息

API Key 明文存 `~/.craftwave/config.json`（v2.5 用户决策废除 keyring，文件权限 0600）；**仓库内不含任何密钥**。用户本机已配 GLM Coding Plan Key，排障时可用其 curl 但不得写入仓库/日志。
