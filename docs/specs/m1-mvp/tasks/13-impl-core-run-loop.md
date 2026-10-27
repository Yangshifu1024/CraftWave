# 实现：单会话核心链路（RunLoop + 帧 + 渲染 + 取消）

Type: task
Status: resolved

## Question

把 spike 形态产品化为 M1 核心链路：

1. backend（src-tauri/src/backend/，§4.3/§5.2）：AgentRun 步进 RunLoop、CallTools 序列化审批挂起（机制层）、Channel 帧生产（64ms 节流、ts?/callId 同源）、取消（select! 流停 + 进程组收割 + runEnded(cancelled) 收齐语义，§5.3）；start_chat/cancel_run 正式注册进 ipc_commands!，顺带收敛两处合同面缺口（§3.3 注册表补 load_session_tree 行、§3.6 补 E_STORAGE_IO）。
2. 前端：帧消费 → assistant-ui 渲染（S1 adapter 直写 result 模式）、打字机、思考流、运行过程实时展示、Composer 取消接 bindCancelToRun（S5）；runMetrics 三数（rate/turnUsage/sessionUsage）按 §10.5 与 C-9 口径（后台小调用不计轮次）。
3. 测试：mock OpenAI 兼容 SSE 离线端到端（文本/思考/工具参数流、取消全链、节流与 NFR-02 <100ms 断言），Rust + vitest 双侧；既有 spike/storage/tools/providers 测试零回归。

边界：审批 UI（AskPanel/工具卡片）接线留工具系统后续票（backend 挂起机制在本票）；真实 provider 联通冒烟留有 key 环境（mock 驱动为准）；多会话 Tab 不在；会话树持久化的 run 落盘点若 §7 未钉死则纸面记录不实现。

完成标准：mock 驱动端到端全绿、检查链全过；实现与 §4/§5/§10 有出入时最小修订对应行并记录，无出入则声明。

## Answer

（2026-10-27 完成。分支 `feat/core-run-loop`；重构（spike→backend/agent/tools 产品化）与功能（命令注册 + 前端接线）两个提交。mock OpenAI 兼容 SSE 全程离线驱动 rig 真 Chat wire，确定性可重复。）

### 1. 迁移清单（spike → 正式模块：迁了什么、留了什么）

spike/ 模块整体**原样保留**（S2/S3/S5 原型与其 12 个测试零回归，本票未删一行）；产品化落在三个正式模块：

| 迁移物 | spike 出处 | 正式落位 | 产品化增量 |
|---|---|---|---|
| ChatFrame + DeltaThrottler + FrameSink | spike/frames.rs | backend/frames.rs | 帧集补全 P0 9 帧（stepStarted/usage）；SequencedFrame 增 `ts` 出口字段（§3.4 帧基座）；UsageTotal 镜像与累计 |
| RigEventBridge | spike/rig_bridge.rs | backend/rig_bridge.rs | **callId 同源映射**（见「出入」节）：End 事件登记「模型 call id → 桥接 id」，`frame_call_id_for_model` 反查；Start 幂等（wire 层 close 阶段补发 Start 不重发 toolStarted） |
| TurnDriver（单 turn 流式装配） | spike/turn.rs | backend/turn.rs | bridge 由调用方持有（RunLoop 反查）；流收口返回 rig Usage（usage 帧源）；`usage_total` 折算 |
| drive_cancellable_run + ToolOutcome/Executor | spike/cancel.rs | backend/run_loop.rs | runStarted/stepStarted/usage/runEnded 帧生产内聚（§5.2 时序）；64ms 节流器收进 RunLoop（帧生产是 backend 职责，不再是测试侧组装）；门控挂起点 ToolGate + RunExit::Suspended（审批挂起机制层）；ToolExecution 双通道形状（ok/uiSummary/error/model_text 对齐 §8.2）；suspend_run/resume_run（AgentRun serde 信封，rig 侧含 format 版本校验） |
| unix_reap / win_job 收割原语 | spike/cancel.rs | tools/process.rs | 原样迁移 + 单测（忽略 SIGTERM 的进程走宽限→SIGKILL 兜底）；command 工具成票时接入 |
| RunManager | （无 spike 对应，§5.1 新建） | agent/run_manager.rs | 同会话互斥（E_SESSION_RUNNING）、cancel 置位即返回、终态摘除（run_id 校验防竞态）；steer 队列/ask 槽/对账快照随对应票 |

留弃判断口径「正式命令面依赖什么」：RunLoop 驱动、帧生产、取消收齐、挂起机制都被 start_chat 依赖 → 迁；spike 的测试装配（tests/common 的 TimedSink/drive_throttled 等）只服务 spike 测试 → 留在原地；mock_chat.rs（S1 dev-only 帧源）与 mirror demo 保留（镜像工具卡的回归锚点）。

**agent/ 模块归属说明**：§2.2 里 RunManager 属 agent/，本票按文档建了 agent/（未挤进 backend），依赖方向 host → agent → backend/tools 合规。

### 2. 注册面与合同面收敛（§3.3/§3.6）

- start_chat / cancel_run 按第 3 章签名正式注册进 ipc_commands!（args_type/ret_type = StartChatArgs/StartChatResult、CancelRunArgs/bool），与 manifest、前端 client.ts 三方契约测试双向锁定（ui contract.test.ts 锚点断言 + SPIKE_ONLY_INVOKES 清空）。
- §3.3 表补 `load_session_tree` 行（编号 26b，P0 全集 31→32）——10 号票已实装、文档漏行，本次补齐；§3.6 补 `E_STORAGE_IO`（10 号票实装的承载码）与 `E_INTERNAL`（util/errors 与前端归一化既有键面）两行；host/error.rs 与 util/errors.rs 的「待补录」注释同步清账。

### 3. 测试清单

Rust（cargo test 全绿：lib 178 + 集成 18）：

- tests/backend_e2e.rs（6 测，mock SSE → RunLoop 生产路径）：
  1. 帧保真与 callId 同源：runStarted/stepStarted×2/delta/toolStarted/toolArgs/toolResult/usage×2/runEnded 全序列断言；三帧 callId 集合同源；seq 严格递增；usage 两轮累计（120/48 → 240/96）；**NFR-02 TTFT <100ms 实测断言**；64ms 节流相邻帧间隔 ≥64ms（55 容忍）断言；
  2. 流中取消：runEnded(cancelled) 最后一帧、全链 <1s、终态后 300ms 无新帧；
  3. 工具批取消收齐：read 结果保留（ok=true 无标记）+ command 进程组整树收割（孙进程消亡实证）补 E_CANCELLED、收齐后才发终态帧；
  4. 审批挂起机制：gate=Suspend → RunExit::Suspended（run 已序列化、无 toolResult/runEnded）→ resume_run 反序列化 → 放行跑到 Done；
  5/6. start_chat_core 命令面：config+keyring 注入解析模型出帧全绿 + RunManager 收尾摘除；同会话第二个 start 被 E_SESSION_RUNNING 拒绝 + cancel_run 置位后取消终态。
- 模块单测：backend/frames_test（ts/usage/error 面协议形态）、rig_bridge_test（callId 映射/幂等）、exec_test（五工具分发、E_INVALID_ARG 回喂、plan 档 E_TOOL_BLOCKED 不落盘）、wire_test（wire 重建 §6.2：toolCall+toolResult 配对拆分、is_error 映射、非法形态拒绝）、agent/run_manager_test、tools/process_test。

前端（vitest 55 测全绿）：run-metrics.test（§10.5/C-9 口径：usage 双口径累计、runStarted 清零、runEnded 冻结、contextProgress）；runtime.test（toWire 双 part 拆分、adapter 帧流 → 64ms 快照 yield → 终态 status 映射 done→complete/stop、cancelled→incomplete/cancelled）；frames.test 增量（usage/run 锚点、toParts 直写 result、半截参数「参数准备中」）；contract.test 更新（start_chat/cancel_run 注册与接线双向锁定）；既有 spike（mirror 5 测）/storage/tools/providers 测试零回归。

### 4. 与文档的出入及处理（均最小修订技术设计文档，见变更记录 v2.3）

1. **§3.4 callId 同源语义修订**（实现发现的设计不可行点）：S2 票钉死「帧面 callId = 模型 call id」，但 rig 流事件（Start/Arguments）只携带 part 序号，模型 call id 要到该 part 的 **End 事件**才定稿——流式工具参数帧发出时模型 id 不存在，字面遵循不可行。落地为「帧面统一桥接派生 id + End 事件后登记模型 call 映射 + toolResult 反查」：帧面三帧自洽且与模型调用一一对应（镜像卡配对不受影响），e2e 断言三帧同源。§3.4 排序规则行已按此修订。
2. **§3.2 RunConfig.tier 字面修订**：文档 snake_case（confirm_each）与 Rust 已落地的 PermissionTier serde camelCase（confirmEach，11 号票）冲突——IPC 反序列化以实现为准，§3.2 行改为 camelCase 并注明 D14 口径。
3. **§4.3 M1 落地注记**：§4.2 的 `BackendEvent` 中间 trait 与 `ScriptedBackend` 本票未实现——单后端（openai_chat）实现下纯转发层无增量价值，防腐目标（rig 升级只改 backend/providers）由模块边界（rig 类型收敛在 backend/turn.rs + rig_bridge.rs）与 mock SSE 端到端测试承载；多后端场景出现时再抽。§4.3 的 RigAgentBackend 载体（AgentRun 步进 + StreamedTurnAssembler + CallTools 序列化挂起）按文档落地，无出入。
4. **§10.5 rate 细化**：「rate=usage 帧 rate 或 TextDelta 窗口自算」取前者——Rust 侧每轮收口必发 rate（输出 tok/s = 本轮 output tokens / 流时长），前端无 rate 帧时保持上次值；TextDelta 自算窗口留后续（token 级自算需 tokenizer，M1 无）。

### 5. 范围内取舍与纸面记录（不修文档，随对应票落地）

- **canonical 落盘纸面记录**（票面边界条款）：§7.3 的 run 落盘点时机已钉死，但其承载面之一 index.json（run 终态「index 更新」行）与 toolres 边车尚未实装（map「持久化与会话树」的明确剩余项）；本票不做半套落盘，§6.3 双写协议不变——start_chat 的 canonical 追加、runMarker 追加、run 落盘随「持久化」剩余票落地。
- **§5.2 时序的 steer/followUp/suggest**：命令 21（inject_run_message）与后台小调用属对应票；RunConfig.steer 字段已就位（恒 false）。
- **错误分类（§11.3）**：RunLoop 失败以 runEnded(error, E_INTERNAL) 兜底收场（终态不变量优先），七类 provider 错误分类与退避重试随 provider 适配层票。
- **审批门策略**：机制层（ToolGate/Suspended/序列化恢复）已测；AllowAllGate 为本票 host 装配占位，§8.5 门控矩阵接线（confirm_each 审批、ask 协议、resolve_ask）随工具系统票。
- **前端范围**：思考流以 `<details>` 折叠承载（§10.4 折叠分组/摘要行随桌面体验票）；tool-call part 返回 null 走 S1 的 toolkit fallback（正式工具卡随工具系统票）；会话 id 用常量 LOCAL_SESSION_ID（sessions store 随会话域票）。

### 6. 验证

提交前检查链全绿：cargo fmt --check / cargo build / cargo clippy --all-targets -D warnings / cargo test（lib 178 + 集成 18，含 spike/storage/tools/providers 零回归）；pnpm typecheck / pnpm test（55 测，含 contract 双向扫描消费 cargo test 产出的 ipc-manifest.json）/ pnpm lint（biome）。真实 provider 联通冒烟按票面边界留有 key 环境。
