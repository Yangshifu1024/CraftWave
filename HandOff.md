# CraftWave 工作交接（HandOff）

> 生成于 2026-10-28（第五次更新）。追加：17 号票（桌面体验）合入 main、全仓注释整理（实习生向代码导览）。
> 规则：本文件只做索引与状态快照，细节一律引用票文件/文档/提交，不复述。

## 一、必读顺序（新会话开场）

1. `AGENTS.md`——提交纪律、检查链、工具链（有本地补充再读 `AGENTS.local.md`，当前与全局指令相同）。
2. `docs/specs/m1-mvp/map.md`——wayfinder 地图：目标，决策索引（Decisions so far），剩余雾区（Not yet specified）。
3. `docs/技术设计文档.md`——开发直接依据（P0 字段级，v2.7 + 17 号票 §10.6 键位表增补）；附录 C 为未决问题清单。
4. 票目录 `docs/specs/m1-mvp/tasks/`——一票一文件，Answer 节是各票结论。
5. **全仓源码注释已按"新入职实习生可独立读懂"标准整理过一轮**（153 文件，`c6d0953`）——领域术语首次出现处都有一句话解释，读代码即读文档。

## 二、当前状态快照（main = `c6d0953`，17 号票 + 注释整理全部在内；本地领先 origin/main 未 push，见四）

- **地图进度**：01–17 号票全部关闭。四张 spike + 三张实现切片 + 核心链路（13）+ 工具系统接线（14）+ run 生命周期落盘（15）+ 多会话与项目（16）+ **桌面体验（17，本次）**。
- **测试基线**：Rust 233 lib + 25 集成全绿（clippy/build/fmt 干净）；前端 vitest 106 例；tsc/Biome 全绿；manifest 双向契约 27 命令 10 事件键。
- **17 号票**（`2a99c59`）：宿主四件套（单实例 single-instance 插件 / 托盘 + 关闭到托盘 / 退出拦截 resolve_exit_request(#23 参数 accepted) / 系统通知 notification 插件——TauriEventSink 对 run:done/error/cancelled + ask:opened 单一旁路，防打扰=通知偏好域+焦点/活跃会话抑制，on_action 点击回跳 notify:activate 路由）+ run 终态三键补实装；前端主题三态（html.dark + 设置页即时预览 + 全量语义令牌迁移中性强调色）、折叠分组（**一 run 一消息的组件内折叠定式**；崩溃 run 渲染层标「已中断」——存储 status 不动）、Composer 全套（@ 菜单/↑↓ 历史/Esc/Shift+Tab 权限档（惰性 runConfig）/草稿 ui-state 持久化；IME/Enter 由 ComposerPrimitive.Input 原生承担）、右栏 Inspector 用量页、滚动跟随 + 回底按钮、项目改名；键位表回写 §10.6。15/16 号票 shortcut 全部收敛。
- **注释整理**（`c6d0953`，用户指令专项）：153 文件全仓注释（Rust //! 模块文档 + 全项注释 + 关键逻辑「为什么」；前端同口径），零代码行为改动。
- **分支记录**：`feat/run-persistence`、`feat/sessions-projects`、`feat/desktop-experience`、`docs/comments-full-repo` 均 fast-forward 并入 main，未删除。

## 三、下一步（按地图雾区顺序）

1. **三平台打包与 1 周真实使用验收**：打包矩阵（dmg/nsis/AppImage+deb）、验收剧本（真实项目、记录方式）；**C-8（临时会话 roots 边界）安全复判在此毕业成票**；17 号票遗留 P1 随手收敛（macOS 通知权限引导、托盘左键单击平台差异、退出确认无超时）。
2. 小项：rate 的 TextDelta 自算、steer/followUp、错误分类 §11.3、ts-rs 引入（C-2 后半）、session_prefs 域命令（#24/#25）、pending_intent UI 接线、设置页与预设打磨（含 17 号票 shortcut：通知正文带会话标题、权限档初始档与 §10.6 confirm_each 口径复核、关闭到托盘改直接退出的配置项、主题启动防亮闪）。
3. **待核实的疑似笔误**（注释任务发现、铁律未动）：`ui/src/settings/SettingsPage.tsx` 的 SecurityPage 中 `entry.indexOf("")` 恒为 0，白名单条目切分疑似漏了分隔符实参，随设置域票核实。

## 四、工作方式（本图约定）

- **wayfinder 模式**：一票一会话；动工先 `Status: claimed`；完成写 `## Answer` + `resolved` + 地图 Decisions 追加一行；新决策进雾区按 tracker 约定立票。
- **并行 worktree 模式**（实证有效）：`git worktree add ../<name> -b <branch> main` + 后台子代理各领一票；合并前先在 scratch 分支试合并并跑全链；多代理改同一文档时各自只动自己章节/行。注释类全仓任务也可直接同仓多代理分文件集并行（17 号后实证：7 代理 153 文件零冲突）。
- **grilling 票**（HITL）留在主会话与用户做，子代理不得代答。

## 五、环境与坑（新设备必看）

- **前端检查从仓库根跑**：`pnpm typecheck/test/lint` 定义在根 package.json（lint 只在根，Biome 在根 devDependencies）；在 `ui/` 下跑 `pnpm lint` 会命中 PATH 上无关同名工具。
- 检查链：`src-tauri/` 下 `cargo fmt --check && cargo build && cargo clippy --all-targets -- -D warnings && cargo test`；根下前端三项。`MACOSX_DEPLOYMENT_TARGET=13.0` 已在 `.cargo/config.toml [env]`。
- 工具链：`rust-toolchain.toml` 钉 1.97.1（rig MSRV）；CARGO_HOME 在 `~/Developer/Rust/cargo`（crates 走 rsproxy 镜像）；GitHub 抖动时命令前缀 `https_proxy=http://127.0.0.1:7890`。
- **多 Tab 架构（16 号票定式）**：每 Tab 一个 `SessionTab`（keep-mounted，非活跃 `display:none`）+ 每会话独立 `sessionAdapter(id)` runtime 与 **独立 `AssistantRuntimeProvider`**（config 共用模块级 `cwAuiConfig`）——任何用到 Thread 原语/`useAuiState` 的组件必须在 Provider 内，否则启动即抛「requires an AuiProvider」。
- **17 号票前端定式**：右栏 Inspector 只挂**活跃 Tab 内**（随会话取数）；折叠分组落在 AssistantMessage 组件内部（`useAuiState(s => s.message)`），收起态过程 part 返回 `<span hidden/>`（非 null）抑制工具卡 fallback——Parts 的 children 返回 null/undefined 会走 `DefaultPartFallback`（源码实证）；折叠摘要数据 = marker 收口摘要（`deriveRunSummaries` 经 `RunSummariesContext` 注入）优先、run-metrics lastRun 兜底。
- **Rust 通知旁路**：`TauriEventSink::emit` 是通知唯一旁路点（四个事件键）；`AppState.config` 已 Arc 化（`&Arc<ConfigStore<T>>` 传参自动 deref 强转，调用点零改动）；`set_ui_state` 顺带写 `active_session`（防打扰精判共享）。
- **tauri-plugin-notification 2.7 能力面**：`Notification::on_action`（含 tap 点击）+ `builder().extra(key, value)` 载荷 + `ActionPerformed.notification().extra()` 回读；回调注册先于首次 show 才有上报。
- **会话相关 store 一律按 sessionId 分桶**：persist（`scopes`）、run-metrics（`bySession`，`useSessionMetrics` 返回稳定零值引用防渲染循环）、ask（`bySession`+`hints`）；ask:closed 载荷无 sessionId，按 askId 全桶匹配关闭。组件内取所属会话用 `useSessionScope()`（`chat/session-scope.ts`）。
- **@ 补全路径口径**：项目主目录下条目返回相对路径（read 工具解析基准），数据目录专属条目绝对路径；macOS `/var` ↔ `/private/var` 符号链接——相对路径判定必须对 canonicalize 后的根做 `strip_prefix`。
- **rfd select_dir**：同步命令在主线程执行（macOS NSOpenPanel 线程约束），不可改 async。
- **vitest**：`toBe` 无第二参数（失败消息写注释）；**Biome**：`obj["字面量"]`→`obj.键`；`role="button"` 的 div 会要求改真按钮（button 不嵌 button——操作按钮放行容器做兄弟）；测试里按 role 取按钮要带 `name`（Composer 区有多个 button：Send/回底/菜单）。
- **Rust 测试**：归档类行序勿依赖 HashSet 迭代序（按时间序显式写入）；大索引测试直接构造 JSON 一次落盘再单次 upsert 触发归档（逐条 upsert 是 O(n²)，27s→2s）。
- **测试方法命名一律英文**（snake_case，2026-10-27 用户指令，全仓已统一）；断言消息保持中文。
- **rig 非流式坑**：`Model::call`（单次补全，suggest 用）期待 `chat.completion` JSON，mock_openai 已支持流式/非流式两形态。
- **assistant-ui 0.15.31 API 坑**：① `BranchPickerPrimitive.Count` 零 props（样式套外层 span）；② `ThreadMessage` 不带 parentId（活动路径父子关系按传入线性序推导——save 协议建立在此前提上）；③ `ComposerPrimitive.Input` 原生承担 Enter 发送/Shift+Enter 换行/IME isComposing 防护（17 号票实证，勿重复手写）；④ `ThreadPrimitive.Viewport autoScroll` 内建「跟随最新 + 上滚暂停」。
- **真实帧序是 Usage 先于工具执行**——RunRecorder 落盘顺序依赖它，写帧序测试别把 ToolResult 放 Usage 前。
- **前端 vi.mock("@/ipc/client") 要补全依赖链导出**（session.ts → runtime.ts → startChat 等），mock 缺导出整文件报错。
- 新依赖（累计）：`anstream`（ANSI 清洗）、`rfd`（select_dir，同步命令）、`@types/json-schema`（ui devDep）、`tauri-plugin-single-instance` 2.5.4、`tauri-plugin-notification` 2.7.0、tauri features += `tray-icon`；`sha2` 全仓统一 **0.11**。
- **智谱 GLM Coding Plan 端点三协议分流**：Anthropic `…/api/anthropic`；OpenAI Chat Completions `…/api/coding/paas/v4`；OpenAI Responses `…/api/v1`。rig 的 openai_chat 拼 `{baseUrl}/chat/completions`，协议不匹配时服务端回 5xx/403 而非明确报错。
- **前端纯浏览器黑屏是既有行为**（无 Tauri IPC 不渲染）；UI 视觉调试用临时 `preview.html` 入口预置 store 数据绕开 IPC（用完即删，勿提交）。
- 测试不进源码文件：Rust 用 `*_test.rs` / `tests/`，前端用 `*.test.ts(x)`。
- 提交：Conventional Commits 中文主题、按路径精确 add（禁 `add -A`）、禁 `--no-verify`、重构与功能分开；代码合并需用户点头，tracker/文档记账类提交按既有实践直提 main。

## 六、建议技能（新会话按需调用）

- `wayfinder`——以地图继续找路/开工票（Work through the map 模式，传 map 路径）。
- `implement` / `implement-spec`——实现票执行。
- `grilling` + `domain-modeling`——决策票（HITL）。
- `research`——外部调研票。
- `handoff`——下次交接（仍按用户要求写到仓库根 `HandOff.md`，技能默认是系统临时目录）。

## 七、敏感信息

API Key 明文存 `~/.craftwave/config.json`（v2.5 用户决策废除 keyring，文件权限 0600）；**仓库内不含任何密钥**。用户本机已配 GLM Coding Plan Key，排障时可用其 curl 但不得写入仓库/日志。
