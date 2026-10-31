# CraftWave 工作交接（HandOff）

> 更新于 2026-10-31（全仓整理后重写）。M1、M2 两个里程碑已完成；历史需求、决策与施工记录在 git 历史的 `docs/specs/`（本分支前可查），当前工作区已清理。

## 一、必读顺序（新会话开场）

1. `AGENTS.md`——提交纪律、检查链、工具链、assistant-ui 使用模式（有本地补充再读 `AGENTS.local.md`）。
2. `docs/技术设计文档.md`——字段级技术设计（IPC 契约、模块设计、错误码注册表），开发直接依据。
3. `docs/功能需求文档.md`——PRD（FR-xx/NFR-xx 编号源头）。
4. `GLOSSARY.md` + `docs/adr/`——领域术语与正典决策。
5. 全仓源码注释按"新入职实习生可独立读懂"标准维护——领域术语首次出现处有一句话解释，读代码即读文档。

## 二、当前状态

- **M1/M2 全部功能完成**。已知发布面问题：长会话（2000 条消息级）发送时末尾插入有实测 ~1.1s 渲冻（决策"带已知问题发版"，虚拟滚动排在 M3）。
- 历史需求与决策考古：`git log -- docs/specs/`（整理分支 `chore/project-cleanup` 之前的提交里可读到全部票文件）。
- 测试规模约束（用户硬性要求）：**后端 ≤250 个、前端 ≤150 个**——新增功能优先在既有测试内扩展 case 表，不轻易新增测试文件。

## 三、检查链与工具链

- **Rust**（`src-tauri/` 下）：`cargo fmt --check` → `cargo build` → `cargo clippy --all-targets -- -D warnings` → `cargo test`。提交前必须全绿。
- **前端**（**仓库根**跑）：`pnpm typecheck` → `pnpm test` → `pnpm lint`（lint 只在根定义，Biome 在根 devDependencies；在 `ui/` 下跑会命中 PATH 上无关同名工具）。
- **CI 契约测试跑位**：`contract.test.ts` 依赖 `cargo test` 落盘产物（`target/ipc-manifest.json` 等）——本地先 cargo test 再 pnpm test；CI 里完整 pnpm test 在 backend job（ubuntu）之后跑，前端 job 只 lint/typecheck。
- `MACOSX_DEPLOYMENT_TARGET=13.0` 已在 `.cargo/config.toml [env]`；`rust-toolchain.toml` 钉 1.97.1；CARGO_HOME 在 `~/Developer/Rust/cargo`（crates 走 rsproxy 镜像）；GitHub 抖动时命令前缀 `https_proxy=http://127.0.0.1:7890`。
- **release 面 strip 坑**：`[profile.release] strip=true` 会剥坏 proc-macro dylib（rustc E0463），修复 `[profile.release.build-override] strip=false`（已配）。日常检查链全在 dev 面，探不到 release 问题——涉及打包/发布排查直接 `cargo check --release`。
- **macOS 上无法交叉验证 Windows**（aws-lc-sys 构建需目标平台工具链）：cfg 相关改动预期跑两三轮 CI windows job。
- 排查构建日志：`gh run view <id> --log`。

## 四、工程约定

- 测试不进源码文件：Rust 用独立 `*_test.rs`（mod.rs 里 `#[cfg(test)] #[path]` 挂载）/ `tests/`；前端用 `*.test.ts(x)` 与源码同目录。
- 测试方法名一律英文 snake_case；断言消息保持中文。
- 同型用例优先在单个 `#[test]`/`it` 内写 case 表循环（或前端 `it.each`），不铺独立用例——测试总量有硬性上限。
- 提交：Conventional Commits 中文主题、按路径精确 add（禁 `add -A`）、禁 `--no-verify`、重构与功能分开提交。
- wayfinder/本地票机制约定见 `docs/agents/issue-tracker.md`（新需求仍在 `docs/specs/<name>/` 建图立票）。

## 五、架构要点与已知的坑（长期有效）

- **多 Tab 架构**：每 Tab 一个 `SessionTab`（keep-mounted，非活跃 `display:none`）+ 每会话独立 `sessionAdapter(id)` runtime 与**独立 `AssistantRuntimeProvider`**（config 共用模块级 `cwAuiConfig`）——用到 Thread 原语/`useAuiState` 的组件必须在 Provider 内，否则启动即抛「requires an AuiProvider」。
- **会话相关 store 一律按 sessionId 分桶**（persist scopes、run-metrics bySession、ask bySession+hints）；组件内取所属会话用 `useSessionScope()`（`chat/session-scope.ts`）；`ask:closed` 载荷无 sessionId，按 askId 全桶匹配。
- **zustand selector 内严禁 `?? []` / `?? {}` 兜底**——每次渲染新建引用导致「getSnapshot should be cached」无限循环；一律模块级常量空值。新加 selector 时全仓 grep 复查。
- **assistant-ui 0.15.31 API 坑**：① `BranchPickerPrimitive.Count` 零 props（样式套外层 span）；② `ThreadMessage` 不带 parentId（活动路径父子关系按传入线性序推导——save 协议建立在此前提上）；③ `ComposerPrimitive.Input` 原生承担 Enter 发送/Shift+Enter 换行/IME 防护（勿重复手写）；④ `ThreadPrimitive.Viewport autoScroll` 内建「跟随最新 + 上滚暂停」；⑤ 消息排队用 `useLocalRuntime(adapter, { unstable_enableMessageQueue: true })`（实验面，升级留意）。
- **前端 vi.mock("@/ipc/client") 要补全依赖链导出**（session.ts → runtime.ts → startChat 等），mock 缺导出整文件报错。
- **测试禁时序赌注**：`!handle.is_finished()` 类断言赌「主线程先于子线程执行」，CI 慢机必炸——并发断言用 `try_lock`/channel 即时判定的确定性写法。
- **Rust 测试性能**：归档类行序勿依赖 HashSet 迭代序（按时间序显式写入）；大索引测试直接构造 JSON 一次落盘再单次 upsert 触发归档（逐条 upsert 是 O(n²)，27s→2s）。
- **跨平台路径口径**：面向用户的展示路径统一正斜杠（`replace('\\', "/")`）；command 测试用 `if cfg!(windows)` 给 powershell 等价命令；unix 专属 API 的测试加 `#[cfg(unix)]`；Windows JobGuard 裸句柄要 `unsafe impl Send/Sync`。
- **契约单一事实源**：`ipc_commands!` 清单宏同时展开 invoke_handler 注册表与 ipc_manifest 清单，新增命令不可能漂移；payload 类型由 ts-rs 生成（`ui/src/ipc/generated/`），Rust 改字段 → 重新 cargo test → typecheck 红即漂移。
- **rig 非流式坑**：`Model::call`（单次补全，suggest/起名用）期待 `chat.completion` JSON，mock_openai 已支持流式/非流式两形态。
- **智谱 GLM Coding Plan 端点三协议分流**：Anthropic `…/api/anthropic`；OpenAI Chat Completions `…/api/coding/paas/v4`；OpenAI Responses `…/api/v1`。协议不匹配时服务端回 5xx/403 而非明确报错。
- **streamdown 包名**：Vercel 流式 Markdown 组件是 `streamdown`（react-streamdown 是更名前旧名，npm 404）；样式导入 `streamdown/styles.css` + 自建 `.cw-md` 排版段（Tailwind preflight 会拍平标题/列表）。
- **真实帧序是 Usage 先于工具执行**——RunRecorder 落盘顺序依赖它，写帧序测试别把 ToolResult 放 Usage 前。
- **通知授权态插件面不可信**：tauri-plugin-notification 桌面端 `permission_state` 恒报 Granted——真实授权查询自建（macOS 直查 UNUserNotificationCenter）；capability 要覆盖插件注入脚本的 JS 面（`notification:default`）。
- **托盘**：tauri 2.12.5 + tray-icon 0.25.1 的 `show_menu_on_left_click(false)` 在 macOS 同样生效——左键唤起/右键菜单三平台一致（Linux AppIndicator 有边界）。
- **rfd select_dir**：同步命令在主线程执行（macOS NSOpenPanel 线程约束），不可改 async。
- **@ 补全路径口径**：项目主目录下条目返回相对路径，数据目录专属条目绝对路径；macOS `/var` ↔ `/private/var` 符号链接——相对路径判定必须对 canonicalize 后的根做 `strip_prefix`。
- **前端纯浏览器黑屏是既有行为**（无 Tauri IPC 不渲染）；UI 视觉调试用临时 `preview.html` 入口预置 store 数据绕开 IPC（用完即删，勿提交）。
- **vitest**：`toBe` 无第二参数（失败消息写注释）；测试里按 role 取按钮要带 `name`（Composer 区有多个 button）。
- **Biome**：`obj["字面量"]`→`obj.键`；`role="button"` 的 div 要求改真按钮（button 不嵌 button——操作按钮放行容器做兄弟）。

## 六、敏感信息

API Key 明文存 `~/.craftwave/config.json`（用户决策废除 keyring，文件权限 0600）；**仓库内不含任何密钥**。用户本机已配 GLM Coding Plan Key，排障时可用其 curl 但不得写入仓库/日志。
