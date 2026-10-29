# 实现：attach_run 帧重放与热重连

Type: task
Status: open

## 要做什么

C-1/D16 定案（§5.8 字段级）：WebView 重载/崩溃后不再受 M1"只读运行中快照、结束后才可见"的限制——RunHandle 持环形帧缓冲（512 帧），`attach_run(sinceSeq)` 重放缺口帧并把帧通道热切换到新 Channel，前端无缝续流。同时补齐对账命令面：`session_run_snapshot`（FR-03-08，§5.6 前端 runEpoch 自愈的服务端半）。

## 具体做什么

### 后端（src-tauri/）

- **环形帧缓冲**：`agent/run_manager.rs` RunHandle 增帧枢纽（建议 `FrameHub` 结构，Mutex 保护）：
  - `{ buf: VecDeque<SequencedFrame>（容量 512）, dropped: u64, channel: 当前发送通道 }`；
  - 现状：`host/commands/chat.rs` `ChannelSink` 独占 Channel 且 seq 从 0 递增（每 run 一纪元）——把"seq 递增 → 入环 → 下发"收进 FrameHub；
  - 溢出丢最旧并 dropped++；发送失败（webview 已重载）不影响入环；
  - 现状锚点（sink 装饰链，动工前先理顺）：start_chat 装配处（chat.rs）现链为 ChannelSink（seq 递增 + Channel 下发）→ EventfulSink（usage 帧差分 → tokens:update）→ RunRecorder/PersistingSink（canonical 落盘，`host/run_persistence.rs`）。FrameHub 收编最内层 ChannelSink 的职责（`chat.rs:804` 的 `ChannelSink { channel, seq: 0 }` 即改造点），外两层不感知重放；重放帧原样携带 seq/ts 字段（§3.4 帧基座）。
- **attach_run 命令**（§3.3 #57）：`{ sessionId, channel, sinceSeq } → { runId?, replayed: number }`。
  - 语义：重放 `(sinceSeq, 当前头]` 区间帧（原 seq 不变，前端按既有去重规则忽略已见）→ 把 FrameHub 的发送通道热切换为新 Channel（此后新帧只走新通道）；
  - 无活跃 run → `{ replayed: 0 }`（runId 省略）；
  - 触发场景：webview 重载/崩溃恢复、前端路由重建（§5.8）。
- **重放窗口耗尽兜底**（§5.8）：sinceSeq 落在窗口之前或 dropped>0 时，补发一次"runState 摘要帧"供前端整体对账——注意 §3.4 M2 增补 6 帧（toolProgress/subStarted/subFrame/subReport/subDone/compactDone）里没有该帧，**帧 type 与负载 §3.4 字段级缺失，先回写 §3.4 定名**（建议 `runSnapshot` 帧，负载对齐 §3.2 RunSnapshot）再实现。
- **session_run_snapshot 命令**（§3.3 #20）：`{ sessionId } → RunSnapshot | null`。
  - RunState（run_manager.rs）已有 status/started_at/turn/tool_count/last_seq，缺 usage——RunState 增 `usage: UsageTotal` 累计（DriveState 收口处同步写入）；
  - RunSnapshot 类型按 §3.2 落命令层；run_manager.rs 头注释"对账快照命令为后续工单"即本票。
- 命令两个都注册进 `ipc_commands!`（lib.rs 单一事实源）；错误面：sessionId 未知 → null（快照）/空重放（attach），不报 E_NOT_FOUND（幂等查询语义，回写 §3.3 注记）。
- 取消/挂起交互：attach 不影响 run 生命周期（不触碰 CancellationToken 与 AskSlot）；审批挂起中 attach → 重放帧含 ask:opened 之前的帧面，前端观察态照常（AskPanel 走事件键而非 Channel，重放不覆盖事件面——重载后 AskPanel 的恢复机制在 Answer 记录取舍）。

### 前端（ui/）

- **重连观察态**：会话进入/恢复路径（`stores/session.ts` + `SessionWorkspace.tsx`）先调 `session_run_snapshot`：
  - status=running → 进入重连观察态：`attach_run(sessionId, channel, sinceSeq=0)`（重载后前端一无所见，从 0 重放最多 512 帧）；
  - 重放帧与续流帧喂独立 FrameBuffer，渲染只读实时视图（文本/工具卡照常增量呈现）；
  - 收到 runEnded → `load_session_tree` 重载 canonical 树收口，退出观察态；
  - 不把帧注入 assistant-ui LocalRuntime（运行态 adapter 实例已随重载消亡，M1 13/15 号票树协议不动）。
- **runEpoch 自愈**（§5.6 前端半，建议新文件 `chat/run-reconnect.ts`）：
  - 帧 seq 必须 > 已见最大 seq 否则丢弃；seq 缺口（跳跃）→ 调 `session_run_snapshot` 对账；
  - runEnded 帧丢失兜底——5s 无新帧且快照 status=idle → `load_session_tree` 重载终态；
  - 现行 `chat/runtime.ts` 的 maxSeq 为单 adapter 局部变量（无跨重载记忆），本票把"已见最大 seq"提升为会话级状态供重连判定。
- 观察态下禁用输入（防 start_chat 撞 E_SESSION_RUNNING），提供"取消"按钮走 `cancel_run`；观察态视觉与正常流式区分（顶部细条"已重连到运行中的任务"）。
- 交互细节：重放帧中的 runStarted 不触发运行态节点记账（`useSessionPersistStore.noteRuntimeNode` 观察态旁路，防污染 canonical 双写协议 §6.3）；观察态期间收到的 ask:opened 事件照常弹 AskPanel（审批不因重载失联）。
- `ipc/client.ts` 增 `attachRun`/`sessionRunSnapshot` 两个出口 + COMMANDS 注册；types.ts 增 RunSnapshot/AttachRunResult（随 02 号票生成物面则 derive）。

### 测试

- Rust 单测（`run_manager_test.rs` 扩展）：
  - 环形缓冲溢出丢最旧 + dropped 计数；
  - 重放区间 (sinceSeq, head] 边界（0/中间/头/超越头）；
  - 热切换后新帧只走新 Channel；无 run attach 返回空；RunState.usage 累计正确。
- e2e（`tests/backend_e2e.rs`）：
  - run 中途以第二 Channel attach → 收到重放帧 + 后续增量走新通道、旧通道断流；
  - run 结束后 attach → 空重放；
  - 人为缩小缓冲构造 dropped>0 → 收到 runState 摘要帧；
  - snapshot 的 usage/lastSeq 与帧面一致；审批挂起中 attach → resolve_ask 仍可恢复。
- 前端 vitest：seq 去重与缺口触发对账（run-reconnect 纯逻辑）；观察态组件（重放渲染 + runEnded 收口 + 输入禁用 + 取消可用）；观察态旁路运行态节点记账；contract.test 扩域（#20/#57 命令、RunSnapshot 类型、若回写摘要帧则帧面覆盖更新）。

## 边界（不在本票）

- 不改帧协议既有语义（M1 已钉死：seq 纪元、64ms 节流、callId 同源）；唯一新增帧面 = runState 摘要帧（先回写 §3.4）。
- 后端 followUp 队列不经本票（06 号票消费本票的 attach_run 转发路径）。
- 多窗口 attach（§5.8 触发场景列了 M3 多窗口）——FrameHub 单通道切换语义对多窗口天然不够，届时另评。
- 子代理 subFrame 帧进重放缓冲的细节（10 号票；本票保证 FrameHub 对帧类型不感知即可）。

## 验收

- [ ] 运行中 reload WebView（dev 下 Cmd+R），会话回到前台后帧不丢续流，runEnded 后树完整收口。
- [ ] 重放窗口耗尽（>512 帧）场景有摘要帧兜底，前端状态与后端一致。
- [ ] session_run_snapshot 随时可查，字段与 §3.2 一致（含 usage/lastSeq）。
- [ ] 断连路径（模拟 Channel 发送失败）不 panic、不丢已提交落盘内容。
- [ ] 既有 e2e 与前端测试零回归；§3.3（#20/#57 注记）、§3.4（摘要帧）回写完成。
