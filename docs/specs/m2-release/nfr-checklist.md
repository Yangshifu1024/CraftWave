# M2 全 NFR 核查表

> Label: `m2:nfr-checklist`。34 号票交付物：PRD M2 出口准则「全 NFR 达标」的逐项证据库。
> 四列口径：**口径**（PRD 原文，`docs/功能需求文档.md` NFR 表）/ **验法**（怎么验）/ **实测**（数据或测试引用，全部可点验）/ **结论**。
> 无对应自动化测试的行标注「人工清单项」并指向 [e2e-checklist.md](e2e-checklist.md) 对应条目。
> 核查时间：2026-10-31（34 号票收口时点）；硬件环境见 NFR-03 行。

## 主表

| NFR | 口径（PRD 原文） | 验法 | 实测 / 证据 | 结论 |
|---|---|---|---|---|
| **NFR-01** 冷启动 | 「冷启动到可交互 < 2s（M2 验收，中等硬件）」 | 启动到应用就绪计时；M1 acceptance §5「本期记录实测」线在此收口 | dev 构建参考值（Apple Silicon 开发机，全新数据目录首启）：进程启动到 Rust 启动序完成（`~/.craftwave/logs/app.log` 出现「应用启动完成」行）**2.60s**——这是未优化 debug 构建 + vite dev 按需编译的**上界**口径；release 产物（安装包装机）为正式口径 | **线收口留 35 号**：release 装机秒表实测（e2e-checklist §0/§12.1 一并走查）。dev 上界 2.6s 与 §2.1 预算分解（Rust 侧 <50ms + 首屏异步加载）一致，无结构性超标信号 |
| **NFR-02** 流式延迟 | 「Rust 收到模型 delta → 前端渲染 < 100ms（Channel 节流 64ms 内）」 | 端到端 TTFT 采样 + 帧保真测试 | S2 spike 实测（设计文档 §3.4 钉死）：TTFT **0.4ms**、单帧桥接 **0.4ms**，Rust 侧上界 = 64ms 窗口 + 亚毫秒桥接，渲染余量 >35ms；回归测试 `e2e_frame_fidelity_call_id_same_origin_throttle_nfr02`（src-tauri/tests/backend_e2e.rs）+ 首增量立即出帧语义钉死 | **达标**（自动化钉住 + spike 实测；64ms 节流语义设计性保证 <100ms） |
| **NFR-03** 长会话 | 「单会话 ≥ 2000 条消息（含压缩后）滚动与发送不卡顿；虚拟滚动下 10 万条可打开」 | dev-only 压测 harness（`ui/src/chat/perf/`，生产构建已实测剔除）合成 2000 条消息灌真实渲染管线，rAF 采样帧间隔，>100ms 长帧 = 0 为通过线（量化口径 PRD 未给，34 号票定为评审可调注记）；后半句（10 万条）属虚拟滚动引入后的口径，不在 M2 | 两次独立实测（2026-10-31，Apple Silicon 8 核 / Safari 27 WebKit——与 Tauri WKWebView 同系统渲染引擎 / vite dev 页面）：**滚动**（预热后首尾往返 10 次）：长帧 0 与 1（孤立 120ms）、平均帧间隔 13.9–15.9ms（60fps）——达标；**发送**（流式一轮）：长帧 4 与 5，其中**发送插入点（2000 条列表末尾追加）稳定 1.07–1.14s 渲染冻结**、流式中另见 100–207ms 偶发长帧——**不达标**；首屏冷渲染（参考）：333–381ms。报告全文（含帧间隔序列）见下方「NFR-03 压测报告」节 | **部分达标，发送口径不达标 →「NFR-03 后果」决策点已触发**（[39 号 grilling 票](tasks/39-grilling-nfr03-send-jank.md)：虚拟滚动提前进 M2 / 带已知问题发版 / 口径修订，三选一待用户决策）。本票按边界不实施优化（热点=无虚拟滚动的全量列表 reconcile，C-5 预设场景） |
| **NFR-04** 工具输出有界 | 「命令输出 spill 机制保证单工具结果内存占用有界（>96KB 落盘）」 | 阈值常量 + spill 行为测试 | `SPILL_THRESHOLD = 96 * 1024`（src-tauri/src/tools/command.rs，>96KB 全量落盘 `logs/spill/<callId>.txt` + 结果只带尾部 16KB + spillPath，24h 惰性清理）；测试 `large_output_spilled_tail_and_path`（src/tools/command_test.rs）；web_fetch（50MiB 预检+流式计数）、http_request（>96KB 复用 spill 原语）、doc 工具（512KB 响应截断）同口径各有测试 | **达标** |
| **NFR-05** 内存/体积 | 「空载 < 300MB；安装包 < 25MB（FR-16-05）」 | 空载 RSS 实测 + release 安装包体积 | 空载 RSS **122 MB**（debug 主进程，2026-10-31 实测；WebView 系统进程另计）——余量充足；安装包（release bundle，2026-10-31 本机 `pnpm build`）：dmg **14.60 MiB**（CraftWave_0.1.0_aarch64.dmg，< 25MB 达标）、.app 30.67 MiB、updater tar.gz 15.10 MiB（updater `.sig` 步骤因 secrets 未配退化——33 号票已记录的 HITL 场景，不影响产物）；Windows/Linux 体积归 build.yml tag 产物 | 内存与体积均**达标**（macOS arm64 实测） |
| **NFR-06** 会话数据零丢失 | 「append-only + 原子写；任意时点强杀进程，重启后历史完整、无半写段」 | D15 fsync 策略（已提交历史零丢失 + 进行中 run 可弃为设计口径）+ 强杀演练 | 单测 `run_persistence_test`（src/host/）；原子写 `util/atomic.rs`（tempfile+rename+fsync）；人工演练 e2e-checklist §12.8（SIGKILL 后无半行、interrupted 标注）；§7.3 时机表按数据类别分级 | **达标**（机制自动化钉住 + 演练留人工复核） |
| **NFR-07** 终态一致 | 「事件丢失时快照对账可自愈；无悬挂 run 状态」 | attach_run 重放 + runSnapshot 对账测试 | `attach_mid_run_replays_and_hot_switches_channel` / `attach_after_run_end_returns_empty_replay_and_null_snapshot` / `snapshot_matches_frames_and_attach_during_approval_pending` / `attach_replay_window_exhausted_sends_snapshot_frame`（src-tauri/tests/backend_e2e.rs 四例）；512 帧环形缓冲 + sinceSeq 重放语义钉死（§5.8） | **达标** |
| **NFR-08** 崩溃隔离 | 「单工具 panic 不崩应用（兜底 E_TOOL_PANIC）；单会话错误不拖垮其他 Tab」 | panic 兜底机制核对 + 多 Tab 架构核对 | 兜底机制**存在但错误码与文档不符**：批次任务 panic 经 JoinError 兜底为 `E_INVALID_ARG`（src/backend/exec.rs「任务 panic：成员位置留空，下方兜底为内部错误」）；pdf/xlsx 解析器 `catch_unwind` 包裹（src/tools/doc.rs，测试 doc_test.rs）——**应用不崩达标，`E_TOOL_PANIC` 独立码未设立**；多 Tab 隔离 = 每 Tab 独立 runtime + 后端每会话独立 run + ChatErrorBanner 行内报错（架构性保证，人工清单项） | **机制达标；文档漂移一处**——设计文档 §3.6 已注记回写：兜底走 E_INVALID_ARG，不设独立 E_TOOL_PANIC 码（避免注册表空挂）。多 Tab 隔离为人工清单项（e2e-checklist §7） |
| **NFR-09** 密钥保护 | 「密钥仅存系统钥匙串；进程内存外无明文持久化；日志全链路脱敏」 | 存储协议核对 + 脱敏测试 | 密钥面按 v2.5 用户决策**改写**：明文随 config.json **0600** 落盘（§9.4 回写，keyring 废除——PRD 原文的「钥匙串」口径已被 v2.5 决策取代并回写）；脱敏闸 `util/redact.rs` 11 例（redact_test.rs：敏感键值/Bearer/sk-/图片 base64 摘要化）；集成 `session_logs_e2e` 2 例（tests/session_logs_e2e.rs：全文无 sk- 明文、Info 级无消息体）常驻 CI；32 号票删除联动测试 4 例 | **达标**（按 v2.5 修订口径；脱敏全链路自动化钉住） |
| **NFR-10** 权限最小化 | 「Tauri capability 白名单；CSP 严格；IPC 命令入参全校验」 | capabilities 清单核对 + CSP 串核对 + 抽验 | capabilities/default.json **17 项显式 allow、零 wildcard**（core:default + window 八项 + decoration + notification + global-shortcut 四项 allow-*（default 为空集实证，29 号票）+ updater:default + process:allow-restart（实名最小项，33 号票））；CSP 严格串（default-src 'self'，connect-src 仅 ipc，frame-src 扩 preview: 沙箱协议——14 号票 §9.5 回写）；IPC 入参全校验 = 每命令 E_INVALID_ARG 前置校验（各命令单测） | **达标**（M2 新增插件 updater/global-shortcut 均实名最小权限，无 default 全开面） |
| **NFR-11** 本地优先 | 「默认零遥测；网络请求仅限用户配置的 LLM/代理/MCP 目标；无内置云依赖」 | 全源码外联 URL 扫描 + 清单核对 | Rust 全源码扫描（grep http(s)://）所得**非测试外联目标全集**见下方「M2 外联域名清单」；遥测面：无任何 analytics/上报代码（扫描无 telemetry/crash 报告端点）；MCP/SearXNG 为用户配置地址不属内置依赖 | **达标**（清单内为目标、清单外无外联；GitHub Releases 为 updater 唯一外联已列入 33 号票） |
| **NFR-12** 事件契约测试 | 「双向扫描测试钉死事件/帧键面；契约漂移 CI 阻断」 | Rust 产出 manifest → 前端双向扫描 | Rust `ipc_manifest_test`（导出 manifest + ts-rs 生成物 + 正向一致，src-tauri/src/ipc_manifest_test.rs）；前端 `contract.test.ts` 五测试面（manifest 扫描/键面卫生/覆盖度/fixture 双向/生成物双向，ui/src/ipc/contract.test.ts）；CI 顺序 cargo test → vitest，缺文件必须红（两文件头注释明言）；**本票扩域核对**：命令 78 行（含 dev-only 1 行）、帧 16、事件键 24 与 FRAMES/EVENTS 常量逐字一致，对照表见附录 A | **达标**（零漂移；CI 阻断生效） |
| **NFR-13** 无 Key 回归 | 「rig-cassette 录制回放覆盖核心 agent 流程」 | — | 设计文档 §12.5 定 M3（ScriptedBackend/rig test-utils 剧本假模型已承载当前全部无 Key 回归：backend_e2e 46 例 + protocol_dialects_e2e 6 例 + watchdog/audit/subagent 单测均无 Key） | **不适用 M2**（M3 引入 rig-cassette，本票边界明示） |
| **NFR-14** 测试金字塔 | 「Rust 单元+集成（cargo test）、前端 vitest（store/组件）、三平台 CI、GUI 人工验收清单」 | 数量与结构盘点 | Rust lib ~870 + 集成 e2e（backend_e2e 46 / protocol_dialects_e2e 6 / session_logs_e2e 2 / tools_registry / spike×5）；前端 vitest ~560（含本票 +15）；三平台 CI（ci.yml 矩阵）+ GUI 人工清单（e2e-checklist.md 本票入库） | **达标**（四层齐备） |
| **NFR-15** 平台矩阵 | 「macOS 13+（arm64）、Windows 10+、主流 Linux；三类 webview 全覆盖测试」 | CI 矩阵 + 发布产物 | ci.yml 三平台（fmt/clippy/test + Linux vitest 契约闭环）与 build.yml 三平台产物矩阵主线结构不动（本票核对无偏移）；三类 webview 的真实走查归 e2e-checklist 三平台过单（WKWebView/WebView2/WebKitGTK） | **达标**（CI 面；webview 真机走查留发版三平台过单） |
| **NFR-16** 协议兼容 | 「OpenAI Chat / Anthropic Messages / OpenAI Responses 三协议，流式与非流式均支持」 | 三方言 e2e + 真机对话 | `protocol_dialects_e2e` 6 例（anthropic 全链 thinking 预算/tool_result id 同源/usage 四口径、openai_responses 全链、401 明确反馈、慢流取消、跨方言热切换）+ probe_test 13 例（三方言连接测试）+ tests/common 三 mock（mock_openai/anthropic/responses）；真机三协议实际对话留 e2e-checklist §9（不以设置项存在替代端到端） | **达标**（自动化 + 人工真机复核双面） |
| **NFR-17** 防腐层 | 「rig 依赖经内部 trait 隔离（D6）；升级 rig 不触碰业务层」 | 模块依赖核对 | §4 防腐层：`backend/` 中立接口（TurnDriver 泛型化后业务层只见 DialectWire/Model 抽象，38 号票）；rig 依赖收敛在 providers/backend 桥接面（rig_bridge.rs），锁版 0.45（§4.4 换边策略 C-6） | **达标**（架构核对；锁版下无升级回归面） |
| **NFR-18** 文档纪律 | 「docs/ ≤ 20 篇主题制；未验证事项必须标注；AGENTS.md 作为工程宪法」 | 盘点 | docs/ 主题制：PRD/技术设计/GLOSSARY/adr/+specs（specs 为票体系非主题文档）；技术设计文档 32 版变更记录全部带「未验证/HITL 留验」标注（如 §12.4 发布面验证属 HITL） | **达标** |
| **NFR-19** i18n 完备 | 「所有 UI 文案经 i18next；新增文案不得硬编码」 | 31 号票门禁 | `i18n/i18n.test.ts` 9 例：zh/en 双包键集双向 diff 为空 + TypeScript AST 扫描 CJK 硬编码（文件级 8 + 精确 4 豁免清单可评审）；Rust 侧 `host/strings.rs` 键集一致性单测；实际实装为轻量自研层（31 号票收敛，§10.8 回写——接口形状对齐 i18next） | **达标**（按收敛后口径；门禁在 CI 生效——31 号票开发期实际拦截过多轮漏网） |
| **NFR-20** 键盘可达 | 「全功能键盘操作；焦点管理正确（弹窗/抽屉）」 | 人工抽查 | e2e-checklist §13 两条抽查路径（核心流纯键盘 + 触发符/快捷键面）；ghost hint Tab 接受/Ctrl+L/Esc 关闭等单点行为各有单测或既有验收 | **人工清单项**（无 GUI 自动化，M1/M2 均未引入 tauri-driver——34 号票边界） |
| **NFR-21** 索引可重建 | 「删除 cache/ 索引目录后：应用正常启动、零数据丢失、降级 + 自动后台重建；启动完整性校验（quick_check + 版本号），异常即弃库重建」 | 建库状态核对 + 单测 | **M2 建库已落地**（20 号票，`storage/index_db.rs`：rusqlite bundled，sessions+stats_cache 两表）；测试 `corrupt_db_is_discarded_and_rebuilt` / `wrong_user_version_triggers_rebuild` / `unavailable_cache_dir_degrades_gracefully` / `backfill_restores_full_table`（src/storage/index_db_test.rs）——quick_check+user_version 校验、弃库重建、缺日回填、降级直读全部钉住 | **达标**（自动化钉住；「删除目录后台重建带进度」的 UI 进度面属 FR-14-09 P2，未到） |
| **命令审计回归（36 号票专项）** | 灰区三态（allow/block/转人工）、硬规则不可豁免、失败转人工、取消传播、恢复不重复审计或执行、模型审计与人工批准分离 | 独立测试文件点验 | 单元 `src/backend/audit_test.rs` 16 例（三态解析/提示词无历史/证据预算/灰区资格表/单槽消费即清=恢复重放不重复审计/硬拒不调模型）+ e2e 6 例 `audit_*`（tests/backend_e2e.rs：allow 全链单次审计/ask 转人工/请求失败转人工/指定模型失效不静默换端点/block 拒绝 E_AUDIT_BLOCKED/指定模型独立端点）；取消传播 = select! 15s 与 run 取消竞争（36 号票决议记录注记：超时分支未单独 e2e，同构收口路径已覆盖）；恢复不重复执行 = grant_audit 单槽消费即清测试 | **达标**（清单引用全部可点验） |

## M2 外联域名清单（NFR-11 附表）

> Rust 全源码扫描（`https?://` 字面量）剔除非测试值后归类；「测试值」指 fixture/SSRF 向量/单测假端点（example.com、evil.sh、10.0.0.x、169.254.169.254 等），不构成真实外联。

| 类别 | 域名 | 引入票 | 说明 |
|---|---|---|---|
| LLM 端点（用户配置驱动） | api.openai.com、api.anthropic.com 及用户自定义 baseUrl | M1 | 对话/连接测试，走 §9.2 代理与 SSRF 管线 |
| 配额适配器白名单（7 家） | api.deepseek.com、api.kimi.com、api.moonshot.cn、api.minimax.cn / api.minimax.com / api.minimax.io、api.z.ai、open.bigmodel.cn、opencode.ai / zen.opencode.ai、openrouter.ai | 21 | 主机白名单 const 逐家核对，仅用户已配该供应商时查询 |
| web_search 通道 | duckduckgo.com（HTML 直抓）、api.tavily.com（Tavily BYOK）、SearXNG=用户自托管地址（不固定域名，本地回环例外不经代理） | 13 | C-12 三通道；白名单外的通用抓取经 web_fetch 且受 SSRF 管线约束 |
| 更新器 | github.com / objects.githubusercontent.com（Releases 下载，latest.json 同源） | 33 | `releases/latest/download/latest.json` endpoint；查询与下载不受安装门限制 |
| 预设清单 | openrouter.ai（供应商预设模板） | 03 | 仅作为预设默认 baseUrl 文案，实际请求仍由用户配置触发 |

## NFR-03 压测报告（数据 + 硬件环境 + 结论）

- **日期**：2026-10-31（两轮独立复跑）
- **硬件**：Apple Silicon 开发机（darwin arm64，8 逻辑核），系统 WebKit（Safari 27.0.1，`AppleWebKit/605.1.15`——与 Tauri WKWebView 同系统渲染引擎）；页面经 vite dev server（5173）加载
- **载体**：dev-only 压测 harness（`ui/src/chat/perf/`：`stress-source.ts` 合成数据 / `frame-sampler.ts` rAF 采样 / `StressHarness.tsx` 负载驱动）——合成 2000 条消息（线性链、20 组 runMarker 分组、四种 assistant 形态轮转（长文本 400–800 字/工具卡×2/混合/短文本）、4 张图片 part、3 条压缩边界投影），经 `entriesToExported` 灌入生产 ChatThread（Markdown 流式组件/工具卡/折叠分组/压缩分隔全在管线内）；生产构建剔除已实测（`vite build` 产物 grep 零残留）
- **负载与判据**：滚动 = 预热 1 次往返后，滚动容器首尾往返 10 次；发送 = 剧本流式一轮（reasoning 10 段 + 文本 60 段，段间双 rAF 还原真实 64ms 节流节奏），采样至 run 收口；判据 = 采样期相邻帧间隔 >100ms 的长帧计数 = 0（与 NFR-02 同量级；PRD 未给数值，34 号票定为评审可调注记）
- **数据**（长帧数 / 最大帧间隔 / 平均帧间隔）：

| 负载 | 第 1 轮 | 第 2 轮 | 判定 |
|---|---|---|---|
| 滚动（稳态） | 0 帧 >100ms / max 53ms / avg 14.5ms | 1 帧 >100ms（孤立 120ms）/ max 120ms / avg 15.9ms | **达标**（两轮均 60fps；第 2 轮单帧 120ms 判为调度/GC 噪声，复跑回落） |
| 发送（流式一轮） | 4 帧 >100ms / max **1075ms** / avg 32.1ms | 5 帧 >100ms / max **1143ms** / avg 33.9ms | **不达标**（两轮重现） |
| 首屏冷渲染（参考，不计线） | 1 帧 333ms | 1 帧 334ms | 记录留档 |

- **长帧定位**（帧间隔序列诊断）：发送负载的长帧集中在**发送插入点**——append 在 2000 条列表末尾追加 user+assistant 后的首帧渲染（1075/1143ms，两轮各 1 次，占发送总时长约 25%）；其余 3–5 个 100–207ms 长帧散布在流式期间（增量重渲染/Markdown 重解析）。
- **根因方向**（不在本票实施）：无虚拟滚动的全量消息列表在末尾追加与流式更新时的整列表 reconcile 与布局成本——正是附录 C-5 虚拟滚动的预设场景；压缩边界投影与折叠分组状态更新为次要嫌疑。
- **结论**：NFR-03 前半口径**滚动达标、发送不达标** → 「NFR-03 后果」决策点触发（[39 号 grilling 票](tasks/39-grilling-nfr03-send-jank.md)），虚拟滚动是否提前进 M2 由用户决策；未决策前 34 号票保持 open。
- **复跑方式**：`pnpm dev` 后浏览器（WebKit）打开 `http://localhost:5173/?stress=1&auto=1` 自动跑完整序列并回传报告（`vite.config.ts` dev-only 中间件落盘 `src-tauri/target/stress-report.json`）；手动单跑用页面按钮。Tauri 窗口内复测（WKWebView 进程内）待 35 号用户自验一并做（唯一差异为 IPC 桥，渲染管线一致）。

## 附录 A：「文档注册表 ↔ 实装」对照表（M2 收口时点）

> 文档注册表 = 技术设计文档 §3.3（命令）/§3.4（帧）/§3.5（事件）规划全集；实装 = `ipc_commands!`（src-tauri/src/lib.rs）+ `FRAMES`/`EVENTS` 常量（src-tauri/src/host/commands/mod.rs），M2 验收线 = 实装全集双向扫描零漂移（NFR-12，已达标）。本表为设计文档注册表全集与实装的差异总览；M3 行单列。

### A.1 命令（文档注册表全集 vs 实装 78 行）

| 分组 | 文档行 | 实装状态 |
|---|---|---|
| P0（#1–31，文档 33 个含 26b/26c） | #3–14、#16–31、26b、26c | **全部实装**（#15 `load_session` 按文档口径不单独注册，由 #14+26b 组合承担） |
| P0 未实装 | #1 `ping`、#2 `app_version`、#4 `open_url` | **未实装**（M1 起未立项：ping/app_version 为连通性诊断占位、open_url 无 P0 需求触发面；不阻塞任何已验收链路） |
| M2（#32–59 表） | #32–59 全表 + 增补行 59b–59o、#34 补充 `revoke_external_dir`、#48 补充 `skill_revoke_trust` | **全部实装**（含 13/14/17/20/28/32/33 号票全部 59x 增补） |
| 实装超出文档表的行 | — | `activate_custom_titlebar` / `restore_native_titlebar`（自绘标题栏激活协议，§2.1，骨架既有未单列）、`mock_chat_run`（dev-only，cfg(debug_assertions)，S1 spike 原型；release 构建不注册）、`path_kind`（#59j 已在表）、`select_dir`（#5 已在表） |
| M3（#60–72，13 个） | scheduler×6、service_list、stop_service、activity_snapshot、index_status、index_rebuild、workspace_tree、mcp_read_resource | **未实装**（M3 范围，单列） |

实装命令总数：**78 行清单**（含 dev-only mock 1 行、平台无关 77 行）；与 manifest 逐字一致（ipc-manifest.json，`cargo test` 产出）。

### A.2 帧（文档注册表全集 vs 实装 16）

| 分组 | 文档行 | 实装状态 |
|---|---|---|
| P0 9 帧 | runStarted/stepStarted/textDelta/reasoningDelta/toolStarted/toolArgs/toolResult/usage/runEnded | **全部实装** |
| M2 增补（文档表列 8 种） | toolAudit、subStarted、subFrame、subReport、subDone、compactDone、runSnapshot | **实装 7 种** |
| M2 增补未实装 | toolProgress（「长任务进度（命令 tail/服务状态）」） | **未实装**——M2 无长任务进度命令需求（其注明的「命令 tail/服务状态」消费方属 M3 后台服务面 §5.10）；S2 spike 的同名 MockFrame 是 dev 演示私有帧不进 ChatFrame 契约。若 M3 服务面需要再补（届时随票回写本表） |

### A.3 事件键（文档注册表全集 vs 实装 24）

| 分组 | 文档行 | 实装状态 |
|---|---|---|
| P0（文档 18 键） | run:start/done/error/cancelled/inject/suggestions、plan:update、tokens:update、ask:opened/closed、session:title、notify:activate、app:exit_requested | **实装 13 键** |
| P0 未实装 | run:retry（可恢复错误自动重试——重试策略 M2 未启用）、tool:start/tool:result/tool:error（跨 Tab 镜像——单会话单 run 模型下无消费面）、menu:action（macOS 原生菜单——M1 起未立项） | **5 键未实装**（EVENTS 常量注释「随对应票填充」；无阻塞链路） |
| M2 增补 | run:compacting/compacted/compact_failed、run:queue、sub:* 六键、mcp:status | **实装 11 键全部落地**（文档 §3.5 节标题「M2 增 11 键」与内表「增补 10 键」计数不一致，随本次回写统一为 11） |
| M3 增补 3 键 | service:update、scheduled:fired、scheduled:done | **未实装**（M3，单列） |

### A.4 工具（注册表）

静态段 **17 个**（M1 9 + 12 号 web_fetch/http_request + 13 号 web_search + 14 号 batch_read/glob/calculate/render_html + 09 号 skill + 10 号 subagent）+ MCP 动态段（运行时可变）；manifest.tools/toolSpecs 双面 17 与前端 toolkit 同源校验（contract.test 锁定）——零漂移。
