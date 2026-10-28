# CraftWave 工作交接（HandOff）

> 生成于 2026-10-28（第七次更新）。追加：收官雾区毕业立 18/19/20 三票；18 号票（打包与验收剧本）全流程关闭——macOS 决策放弃 Intel、release 面 strip 坑修复、build.yml 首跑三平台全绿；验收剧本 acceptance.md 入库。
> 规则：本文件只做索引与状态快照，细节一律引用票文件/文档/提交，不复述。

## 一、必读顺序（新会话开场）

1. `AGENTS.md`——提交纪律、检查链、工具链（有本地补充再读 `AGENTS.local.md`，当前与全局指令相同）。
2. `docs/specs/m1-mvp/map.md`——wayfinder 地图：目标，决策索引（Decisions so far），剩余雾区（Not yet specified）。
3. `docs/技术设计文档.md`——开发直接依据（P0 字段级，v2.7 + 17 号票 §10.6 键位表增补）；附录 C 为未决问题清单。
4. 票目录 `docs/specs/m1-mvp/tasks/`——一票一文件，Answer 节是各票结论。
5. **全仓源码注释已按"新入职实习生可独立读懂"标准整理过一轮**（153 文件，`c6d0953`）——领域术语首次出现处都有一句话解释，读代码即读文档。

## 二、当前状态快照（main = 票 18 关闭提交，与 origin/main 一致；**检查 CI 六 job 全绿 + build.yml 打包首跑三平台全绿**）

- **地图进度**：01–18 号票关闭；19（平台欠账）/20（C-8 grilling）已立待做；最后一站=用户按 acceptance.md 执行 1 周真实使用验收。
- **测试基线**：Rust 233 lib + 25 集成（macOS 本地）+ release check 零 warning；前端 vitest 107 例；tsc/Biome 全绿；manifest 双向契约 27 命令 10 事件键；**GitHub CI 三平台六 job 全绿；build.yml（tag 触发打包）workflow_dispatch 首跑三平台绿**——产物 dmg 6.77MiB / nsis 5.70MiB / deb 8.70MiB / AppImage 83.27MiB（rpm 顺带产出不收集）。
- **18 号票关键决策与发现**：
  - **macOS 放弃 Intel（2026-10-28 用户决策）**：FR-16-01 universal 口径收窄 arm64-only，PRD/设计文档 v2.8 已回写；build.yml 无需平台分支（三平台统一主机架构构建）。CodeWave 对照结论：其依赖树同样含 aws-lc-sys，「不需要交叉编译」真因是从未跨架构构建（主机=目标）；universal 取消后该问题整体消失。
  - **release 面 strip 坑**（最重要发现，§12.4 已注记）：`[profile.release] strip=true` 剥坏 proc-macro dylib（`dlopen: mis-aligned LINKEDIT string pool` → dyld 拒载 → rustc E0463），修复=`[profile.release.build-override] strip=false`；dev 面检查链（含 CI）探不到，仅 release 构建暴露。
  - 本地实证：arm64 dmg 可安装启动（冒烟 3s 拉起）、release 冷编译 2m40s。
- **验收剧本**：`docs/specs/m1-mvp/acceptance.md`——三平台未签名安装引导、首日 P0 走查、每日强杀核查四场景（运行中/审批挂起/分支操作后/断电）、NFR 口径（NFR-09 按 v2.5 明文 0600）、记录模板（acceptance-log.md）与出口判定（7 自然日+零丢失+无阻断缺陷）。
- **17 号票后 UI 迭代**（提交序列，均已在 main）：
  - `eacd05c` 流式 Markdown（**streamdown 2.7.1**，注意包名不是 react-streamdown——那是其更名前的旧名，npm 404）；styles.css 的 `.cw-md` 排版段；左栏首行下移 40px 避交通灯；「设置/用量」双入口移左栏底部。
  - `ca6c500` Composer 模型下拉（`session-model.ts` 会话内存态 → RunConfig.modelId 惰性生效，§11.1 Rust 面既有）；权限档可点击。
  - `c39cfb8` 运行中消息排队（`useLocalRuntime(adapter, { unstable_enableMessageQueue: true })`；`ComposerPrimitive.Queue` + `QueueItemPrimitive`；Send 恒显）。
  - `e927a5b` **工作流风格改版**（对照用户参考截图 A1–A7+B）：助手内容平铺无气泡 + 内容列 max-w-4xl 居中；工具卡单行摘要化（`ToolCardShell`：图标+主参数+diff 徽标+状态，点击展开，错误态默认展开）；思考单行；运行中 spinner；Composer 三层（输入/工具栏：附件占位+盾牌权限档下拉+模型+方形发送；状态栏：思考计时+三数）；左栏相对时间 + 分组超 5 条折叠；Inspector 计划区（`plan-state.ts` 订阅 plan:update，内存态）。
  - `fbc99d5` 通知 capability（`notification:default`——插件注入的初始化脚本走 JS 面查权限，Rust 侧调用不需要但 JS 面要）+ Inspector planItems selector 稳定空数组。
- **CI 修复**（`ae5189c`→`0c5bf3b` 六笔）：Windows 编译三处（enable_process_group 导入缺 cfg(unix)、JobGuard 缺 unsafe Send/Sync、symlink 测试 unix API）+ S5/e2e 测试 cfg(unix) + Windows 分支 clippy let_and_return + dead_code + 四例测试平台化（workspace/grep 展示路径统一正斜杠；command 测试双 shell 分支）。**欠账（进程树收割 e2e Windows 跳过等四项）已立 19 号票承接**。
- **分支记录**：`fix/*`、`feat/*` 各分支均 fast-forward 并入 main，未删除（最新：`fix/packaging-arm64`）。

## 三、下一步（地图收官阶段）

1. **19 号票（平台欠账收敛）**：e2e 双平台 executor（Windows 批取消语义真实覆盖）、macOS 托盘左键对齐（`show_menu_on_left_click(false)`）、macOS 通知权限引导、退出确认 10s 超时兜底——可丢 worktree 后台子代理做。
2. **20 号票（C-8 安全复判，grilling/HITL）**：临时会话 roots=全局数据目录对 write/edit 是否过宽（config.json 明文 Key 在可写面内）——主会话与用户做，子代理不得代答；结论回写附录 C-8 + 必要改动立票。
3. **终局关**：19/20 关闭后，用户按 `docs/specs/m1-mvp/acceptance.md` 在 macOS 真实项目连续使用 1 周（每日记录 acceptance-log.md），通过即 M1 关图。
4. 小项（穿插）：rate 的 TextDelta 自算、steer/followUp（**排队 Steer 按钮随 §5.5 票**——队列 UI 已就位，QueueItemPrimitive.Steer 未接）、错误分类 §11.3、ts-rs 引入（C-2 后半）、session_prefs 域命令（#24/#25——**模型选择/权限档的持久化挂这**）、pending_intent UI 接线、设置页与预设打磨（含：通知正文带会话标题、权限档初始档与 §10.6 confirm_each 口径复核、关闭到托盘改直接退出的配置项、主题启动防亮闪、思考时长帧面计时（现 thinkingSince 只有起止点））。
5. **待核实的疑似笔误**（注释任务发现、铁律未动）：`ui/src/settings/SettingsPage.tsx` 的 SecurityPage 中 `entry.indexOf("")` 恒为 0，白名单条目切分疑似漏了分隔符实参，随设置域票核实。

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
- **zustand selector 内严禁 `?? []` / `?? {}` 兜底**——每次渲染新建引用，Object.is 恒不等，React 报「getSnapshot should be cached」并 Maximum update depth 无限循环；一律模块级常量空值（run-metrics 的 EMPTY_METRICS、Inspector 的 EMPTY_PLAN 同款）。新加 selector 时全仓 grep 复查。
- **capability 要覆盖插件注入脚本的 JS 面**：tauri-plugin-notification 的初始化脚本查 is_permission_granted 走前端权限（`notification:default`），Rust 侧调用不需要——判权限时两头都要想。
- **跨平台路径口径（CI 修复定式）**：面向用户的展示路径（workspace 搜索、grep 命中行）统一正斜杠（`replace('\\', "/")`）；command 测试用 `if cfg!(windows)` 给 powershell 等价命令（产品命令入口本就 Unix sh / Windows powershell 双 shell）；unix 专属 API（std::os::unix、进程组收割）的测试加 `#[cfg(unix)]`；Windows JobGuard 裸句柄要 `unsafe impl Send/Sync`（内核句柄本可跨线程）。
- **CI 契约测试跑位**：contract.test.ts 依赖 cargo test 产物（ipc-manifest.json + session-tree-fixture.jsonl），前端 job 不装 Rust——完整 pnpm test 在 backend job（ubuntu）cargo test 之后跑（ci.yml），前端 job 只 lint/typecheck。本地顺序同：先 cargo test 再 pnpm test。
- **macOS 交叉验证 Windows 不可行**：aws-lc-sys 构建脚本需目标平台工具链（cmake/NASM），cfg 面只能交 CI windows job 验证——本地绿≠CI 绿，改 cfg 相关代码预期要跑两三轮 CI。（macOS 内交叉 x86_64 切片原本同样受此限；18 号票决策放弃 Intel 后不再有跨架构构建，此坑仅存 Windows 向。）
- **release 面 strip 坑（18 号票实证）**：`[profile.release] strip=true` 剥坏 proc-macro dylib——`dlopen: mis-aligned LINKEDIT string pool`，dyld 拒载、rustc 报 E0463 can't find crate；修复 `[profile.release.build-override] strip=false`（Cargo.toml 已配）。**日常检查链全在 dev 面（strip 不生效）探不到**——涉及 release 面/打包的排查，本地直接 `cargo check --release`。
- **build.yml 打包首跑已验证**（run 40646253875 三平台绿）：产物收集步骤按 `target/release/bundle/` 统一路径；tag push 触发时多一个「创建 Release 草稿」job 并挂载产物，workflow_dispatch 只验构建。排查构建日志用 `gh run view <id> --log`（GitHub 直连抖动时前缀代理变量）。
- **streamdown 包名**：Vercel 流式 Markdown 组件是 `streamdown`（react-streamdown 是更名前旧名，npm 404）；样式导入 `streamdown/styles.css` + 自建 `.cw-md` 排版段（Tailwind preflight 会拍平标题/列表）。
- **消息排队（assistant-ui 实验面）**：`useLocalRuntime(adapter, { unstable_enableMessageQueue: true })` 开启后运行中发送入 composer 队列、run 结束自动续发（后端 E_SESSION_RUNNING 约束天然满足——remove 在帧发出后同步完成）；`ComposerPrimitive.Queue` + `QueueItemPrimitive`（Text/Remove/Steer——Steer 随 §5.5 票）；官方标注 experimental，升级 0.15.x 小版本留意此点。
- 新依赖（累计）：`anstream`（ANSI 清洗）、`rfd`（select_dir，同步命令）、`@types/json-schema`（ui devDep）、`tauri-plugin-single-instance` 2.5.4、`tauri-plugin-notification` 2.7.0、tauri features += `tray-icon`、**`streamdown` 2.7.1（ui）**；`sha2` 全仓统一 **0.11**。
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
