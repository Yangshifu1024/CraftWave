# 调研：rig 0.45 稳定版 vs 0.46-rc.1 锁版底料（C-6 前置）

Type: research
Status: resolved

## Question

rig 锁 0.45 稳定版还是 0.46.0-rc.1（技术设计文档附录 C-6 的前置调研，为「S2+S3 rig 流事件保真与手动工具循环」的双版本验证聚焦）。

产出四部分：

1. **API 差异清单**：两版在 AgentRunner/run_channel、手动工具循环（单 turn 返回 + 外部执行工具回喂）、流式事件（MultiTurnStreamItem：Text/Reasoning/工具参数增量）、AgentHook::Dispatch（HITL 审批）、rig-memory 会话格式上的差异；以官方仓库源码/docs.rs 为准。
2. **破坏性变更记录**：MIGRATING/changelog/issue 中 0.45→0.46 的 breaking changes 与已知问题。
3. **稳定性信号**：0.46-rc 系列的发布节奏、是否有项目生产使用、未决 rc 问题。
4. **锁版倾向建议**：给出一侧倾向与触发换边的条件，供 Spike 双版本验证时聚焦观察。

信源：rig 官方仓库（GitHub: 0xPlaygrounds/rig）、docs.rs、crates.io、discussions/issues。

## Answer

（调研日期 2026-10-27，全部结论以 crates.io / GitHub releases / docs.rs 一手页面核对）

### 0. 版本现状核实

调研文档的说法**属实**，与官方仓库实际状态一致：

- crates.io：`0.45.0`（2026-10-19 发布）为 `max_stable_version`；`0.46.0-rc.1`（2026-10-23 发布）为唯一预发布版本且是 `max_version`，无 alpha/beta，无 yanked。
- GitHub releases：tag `v0.45.0`（stable）与 `v0.46.0-rc.1`（prerelease）为最新两个 release；repo 约 9k stars、77 个 open issues、默认分支 main。
- 截至 2026-10-27：**rc.1 发布仅 4 天**，且 rc.2 的 release PR（#2883）已于 2026-10-23 开出——说明 rc.1 已带已知待修问题。

### 1. API 差异清单（docs.rs 两版逐页比对）

结论先行：**CraftWave 关心的全部 API 面在 0.45.0 与 0.46.0-rc.1 签名级一致**。这套 API 是 0.45 重构后的新体系，0.44→0.45 才是大断层。

- **AgentHook**（两版 trait 定义逐方法一致，13 个 provided 方法，无 required，not dyn-compatible）：
  `name / on_run_start(RunStart)->RunStartAction / on_run_settled(RunSettled) / on_model_select->ModelSelectionAction(sync) / on_completion_call(CompletionCallEvent)->CompletionCallAction / on_model_turn_finished(ModelTurnFinished)->ModelTurnAction / on_invalid_tool_call(&InvalidToolCallContext)->Option<InvalidToolCallAction> / on_text_delta(TextDelta)->ObservationAction / on_reasoning_delta(ReasoningDelta)->ObservationAction / on_tool_call_delta(ToolCallDelta)->ObservationAction / on_dispatch(DispatchEvent)->DispatchAction / on_outcome(OutcomeEvent)->OutcomeAction / observes(StepEventKind)->bool`。
  HITL 审批要点（两版相同）：`on_dispatch` 门控依赖 `observes` 对 `StepEventKind::ToolDispatch`/`CompletionDispatch` 返回 true，否则"不能 deny/patch/replace"；`on_outcome` 每个被派发 id 至多一次、被 deny 的派发无 outcome，**"Ok 的 completion outcome 不代表 turn 被接受"**（0.46 文档把该语义写得更明白，签名不变）；`on_tool_call_delta` 中 stop 会在工具执行前取消 run，工具参数增量在 turn 被接受前是 provisional。
- **AgentRunner**（两版一致）：`run() -> impl Future<...Result<PromptResponse, PromptError>>`、`stream() -> Pin<Box<dyn Stream<Item = Result<MultiTurnStreamItem, PromptError>> + Send>>`（lazy）、`run_channel() -> (impl Future<...>, RunEvents)`——事件馈送为有界队列（`RUN_EVENTS_CAPACITY`）**背压式**（满时 run 等待消费方；drop 馈送则 run 无人观测跑完）。配置面一致：`add_hook / max_turns(总模型调用预算) / memory(impl AsyncStore) / conversation / without_memory / tool_context / history / tool_concurrency(默认1) / unhandled_invalid_tool_call` 等。构造经 `Agent::prompt` / `Agent::resume`。
- **MultiTurnStreamItem**（两版同为 7 个变体，`#[non_exhaustive]`）：
  `StreamAssistantItem(Item<StreamEvent>)`（provider 流事件：文本/思考片段、**工具参数增量片段**）、`ToolCall{tool_call}`（**已提交历史**的调用投影，commit 顺序，后续必有配对 `ToolResult`）、`ToolExecutionCommitted{tool_call}`（rig 真执行完并提交，非实时开始通知，整批 settle 后才浮出，含 `DispatchAction::Patch` 改写）、`ToolResult{tool_result}`（整批原子提交、按调用顺序浮出）、`CompletionCall(CompletionCall)`（usage 为该次请求最终值）、`ModelTurnRetried{turn}`（hook 拒绝重试，该 turn 的文本/思考增量是 provisional，消费方应丢弃/重置）、`FinalResponse(PromptResponse)`（终态，错误走 stream `Err`）。0.46 文档措辞更细（如 "unmodeled passthrough payloads"、"原子性挂在 tool_concurrency 上"），无变体增删。
- **手动工具循环 = AgentRun（sans-IO 状态机，两版一致）**：驱动协议 `next_step() -> Result<AgentRunStep, PromptError>`（`CallModel`/`CallTools`/`Done`；等待应答期间幂等，重发 CallModel 不耗预算）→ `model_response(ModelTurn)` / 流式路径 `record_streamed_completion_call(...)` + `streamed_turn(StreamedTurn)` → `CallTools` 时宿主外部执行工具后 `answer(ToolAnswer)` / `answer_all(iter)`（all-or-none，整批按调用序提交为一条 user 消息）。畸形调用：`malformed_context / resolve_invalid_tool_call(InvalidToolCallAction::{Fail,Retry,Repair,Stop,Skip})` 及流式对应变体。`AgentRun` 为 `Clone + Serialize + Deserialize`，持久化信封 `RUN_FORMAT = 5`（两版相同），**任意非终态可序列化挂起、重启后从 `next_step()` 恢复**（pending invalid-call 决议除外）——正是 HITL 挂起需要的形态。差异：0.46-rc.1 新增 `run::project` 函数。
- **rig-memory 会话格式（两版一致）**：`rig_memory::FORMAT = "rig-session/4"`（doc："The saved format a log's header names: the one format this rig reads"）。导出面一致：`Session / Writer / SessionMeta / DirStore / MemoryStore / Store / AsyncStore（每个 Store 自动是 AsyncStore）/ load / save / load_async / save_async / CompactionPolicy / Summarizer / recent`。0.45.0 与 0.46.0-rc.1 的会话文件互通。
- **其余可见差异（均不在 CraftWave 关心面）**：0.46-rc.1 新增插件系统 CLI（`rig plugin ...`）、`rig-harness`/`rig-tools`/`rig-basics` 插件生态、gemini3/vertexai-anthropic/minimax 等 provider 特性、catalog 构建器、cassette JSONL 回放与 `Handlers::resume_ids_after`。注：`StreamedTurnAssembler`（手动流式装配，`rig::run::streamed`）两版模块清单一致，未逐签名比对。

### 2. 破坏性变更记录

- **0.44→0.45 才是大断层**（MIGRATING.md#044--045，CraftWave 直接从 0.45+ 入场即绕过）：MSRV 提到 Rust 1.97.1；新内核用 Bevy `=0.20.0`；`rig::core` 路径移除（用根 `rig::`）；histories/tools/requests 改 `Arc` 共享；`PortableTool`/`ErasedTool` 移除、统一为上下文 `Tool`；effect bus 移除改内联 `Handlers`（并发限流是 handler 自己的责任）；`PreparedRequest.policy`（TurnPolicy 统持工具选择/可执行集/输出工具）；`PendingToolCall` 拆分、`AgentRun::tool_results` 移除改 `answer`/`answer_all`；`RUN_FORMAT` 升 5 **无升级垫片**（旧持久化 run 不可加载）；`ConversationMemory` 及策略包装移除；hooks 改为"每次派发尝试都触发"（计费/持久化逻辑要挂在 accepted turn 上）；`CHECKPOINT_FORMAT = 7`。
- **0.45→0.46.0-rc.1：无独立迁移指南章节**。rc.1 release body 引用的迁移锚点仍是 `MIGRATING.md#044--045`，其 "Other" 段那一大串 breaking 项是随链接带入的 0.44→0.45 累计列表（与 0.45.0 release 重合，连 PR 号 #2756/#2757/#2762/#2763 都相同）。rc.1 **自身**的 breaking 项只有 observe 模块一条：usage 改为每个成功尝试只观测一次、`ObservationSink::emit` 移除（`ObservationSink` 属 `rig::observe`，不在 `rig::agent`/`rig::run` 面）。
- **rc.1 已知未决问题**（GitHub issues，多与 rig-memory 持久化相关，恰是 CraftWave 会话存储路径）：#2907 `Session::save` 在 blob 写失败后丢 pending 消息（10-26）；#2921 并发 `DirFiles` 写损坏文件并报 NotFound（10-27）；#2910/#2911 Bedrock 丢 `tool_use` 类型调用（0.44.0+，提交者要求 0.46.0 前修复）；#2883 rc.2 release PR 已开。

### 3. 稳定性信号

- **节奏极快且每版带 breaking**：0.40(07-11)→0.41(07-28)→0.42(08-17)→0.43(09-30)→0.44(10-07)→0.45(10-19)→rc.1(10-23)。稳定版 1~6 周一发；无维护旧分支的迹象（修复面向 main/最新线）。
- **rc.1 无外部生产使用反馈**：web 检索无任何针对 0.46.0-rc.1 的第三方稳定性讨论——发布 4 天，谈不上有生产验证。
- **正向信号**：craft 侧仓库活跃（9k stars）；rc.1 相对 0.45.0 在 CraftWave 关心面上零签名变化；`#2910` 明确以 "before 0.46.0" 为修复期限，暗示 0.46.0 stable 不远。
- **风险信号**：rc.1 一出 4 天内已排 rc.2；新报 bug 集中在 Session/DirStore 持久化——但 #2907/#2921 在 rc.1/main 复现、其代码 0.45.0 同样包含，锁 0.45 并不豁免这些 bug。

### 4. 锁版倾向建议

**倾向：S2+S3 Spike 以 `rig = "0.45.0"` 为主底料；用 `rig = "=0.46.0-rc.1"` 做平行编译/行为验证分支。** 同一套宿主代码预期两个版本都能编译（关心面签名一致），双版本验证成本主要在行为观察而非 API 移植。

理由：
1. 工单关心的五个能力（AgentRunner/run_channel、AgentRun 手动工具循环、MultiTurnStreamItem、AgentHook::Dispatch、rig-session/4）两版逐签名一致，锁 0.45 不损失任何目标能力；
2. 0.45.0 是 `max_stable_version`，Cargo 默认语义直接可用；rc 需要显式精确 pin，且 `0.46.0-rc.1` 作为依赖需求串对后续 `0.46.0` stable 不可自动升级（pre-release 匹配规则），换版要手动；
3. rc.1 太新（4 天）且 rc.2 已在排队，rc 期 API/行为仍可能微调（项目惯例是每版都敢 breaking）。

**触发换边到 0.46（或直接等 0.46.0 stable）的条件**：
- 0.46.0 stable 发布，且 #2907（Session::save 丢消息）、#2921（DirFiles 并发写损坏）、#2910（Bedrock tool_use）修复已落地——届时直接上 0.46.0 stable，跳过 rc；
- Spike 需要 rc.1 独有增量：`run::project`、cassette 回放/`resume_ids_after`、插件生态、provider 新特性（当前 S2/S3 范围内均用不到）；
- 0.46 stable 落地后 0.45 线无补丁（该仓库只维护最新线），长期停驻 0.45 会积累不可回移植的 bug 修复。

**Spike 双版本验证时的聚焦观察清单**（无论锁哪版都要看）：
- `DirStore` 并发写（#2921 场景）与 `Session::save` 失败路径（#2907 场景）——CraftWave 会话持久化直接踩这两块；
- `run_channel` 背压行为：`RUN_EVENTS_CAPACITY` 满时 run 阻塞，Tauri 事件桥消费不及时会拖住 agent 循环；
- `on_dispatch` 必须实现 `observes` 放行 `ToolDispatch`，否则 HITL 审批 hook 静默不触发；
- `MultiTurnStreamItem` 的 provisional 语义：`ModelTurnRetried` 后必须重置该 turn 已渲染的文本/思考增量；`ToolExecutionCommitted` 与 `ToolCall`/`ToolResult` 的时序差（整批 settle 后才浮出）；
- hooks "每次派发尝试触发" 语义对审批/计费去重的影响（0.45 起的行为变化）。

### 信源

- crates.io API：https://crates.io/api/v1/crates/rig
- GitHub repo 元数据：https://api.github.com/repos/0xPlaygrounds/rig
- Release v0.45.0：https://github.com/0xPlaygrounds/rig/releases/tag/v0.45.0
- Release v0.46.0-rc.1：https://github.com/0xPlaygrounds/rig/releases/tag/v0.46.0-rc.1
- 迁移指南（main）：https://raw.githubusercontent.com/0xPlaygrounds/rig/main/MIGRATING.md
- docs.rs 0.45.0：https://docs.rs/rig/0.45.0/rig/agent/trait.AgentHook.html 、…/agent/enum.MultiTurnStreamItem.html 、…/agent/struct.AgentRunner.html 、…/agent/struct.AgentRun.html 、…/run/index.html 、…/run/constant.RUN_FORMAT.html
- docs.rs 0.46.0-rc.1：https://docs.rs/rig/0.46.0-rc.1/rig/agent/trait.AgentHook.html 、…/agent/enum.MultiTurnStreamItem.html 、…/agent/struct.AgentRunner.html 、…/agent/struct.AgentRun.html 、…/run/index.html 、…/run/constant.RUN_FORMAT.html
- rig-memory：https://docs.rs/rig-memory （latest=0.45.0）、https://docs.rs/rig-memory/0.45.0/rig_memory/constant.FORMAT.html 、https://docs.rs/rig-memory/0.46.0-rc.1/rig_memory/constant.FORMAT.html
- rc 相关 issues：https://github.com/0xPlaygrounds/rig/issues/2883 、/issues/2907 、/issues/2910 、/issues/2911 、/issues/2921
