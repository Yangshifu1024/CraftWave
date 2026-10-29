# 实现：attach_run 帧重放与热重连

Type: task
Status: resolved

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
- **待审批恢复（2026-10-29 用户确认）**：attach 不改变 run 生命周期或审批决定。重连时查询后端仍有效的待审批请求，恢复 AskPanel 并展示原始操作及参数；已经失效的审批关闭。重载本身不代表批准或拒绝。审批走事件键，Channel 重放不能替代待审批查询；查询与实时事件按请求标识对账，处理重连期间请求已解决的竞态。待审批查询及有效性校验的字段级契约先回写 §3.3/§5.6，再实现。

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
- 交互细节：重放帧中的 runStarted 不触发运行态节点记账（`useSessionPersistStore.noteRuntimeNode` 观察态旁路，防污染 canonical 双写协议 §6.3）；观察态期间收到的 ask:opened 事件照常弹 AskPanel；重连前已挂起的审批按后端有效请求恢复，已失效请求关闭（审批不因重载失联）。
- `ipc/client.ts` 增 `attachRun`/`sessionRunSnapshot` 两个出口 + COMMANDS 注册；types.ts 增 RunSnapshot/AttachRunResult（随 02 号票生成物面则 derive）。

### 测试

- 待审批恢复：重载前已有请求恢复原始操作与参数；已失效请求关闭；重连查询与实时批准/取消事件交错时不重复或恢复过期审批，后端拒绝过期请求操作。

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

- [x] 审批挂起时重载，原始操作与参数恢复且可正常批准/拒绝；重载不自动决定审批。重连期间已解决或取消的请求不残留可操作弹窗。（e2e `snapshot_matches_frames_and_attach_during_approval_pending`：挂起中 `session_pending_ask` 返回原始 approval 载荷（含 command）、attach 不改 run 生命周期、resolve 照常恢复；AskPanel 对 resolve 的 E_NOT_FOUND 按 askId 关闭弹窗；解决后查询为 null、过期 askId 拒绝 E_NOT_FOUND）

- [x] 运行中 reload WebView（dev 下 Cmd+R），会话回到前台后帧不丢续流，runEnded 后树完整收口。（e2e `attach_mid_run_replays_and_hot_switches_channel`：中途 attach 全量重放 + 续流只走新端旧端断流 + 帧序连续；前端 `reconnectIfNeeded` 观察态 runEnded → exit + onEnded 重载 canonical 树；run-reconnect.test 全链覆盖）

- [x] 重放窗口耗尽（>512 帧）场景有摘要帧兜底，前端状态与后端一致。（单测 `attach_snapshot_frame_when_window_exhausted` + e2e `attach_replay_window_exhausted_sends_snapshot_frame`：runSnapshot 摘要帧补发、占新 seq 号不进环形缓冲、lastSeq=摘要帧之前最大序号）

- [x] session_run_snapshot 随时可查，字段与 §3.2 一致（含 usage/lastSeq）。（e2e `snapshot_matches_frames_and_attach_during_approval_pending`：lastSeq=帧面最大 seq、usage=最后 usage 帧累计、runId 对上注册表；结束/未知会话 → null）

- [x] 断连路径（模拟 Channel 发送失败）不 panic、不丢已提交落盘内容。（ChannelSink.send_seq 失败静默——帧已入环形缓冲可重放；canonical 落盘在 PersistingSink 层先于下发、不受通道影响；既有 e2e 断连语义零回归）

- [x] 既有 e2e 与前端测试零回归；§3.3（#20/#57 注记）、§3.4（摘要帧）回写完成。（cargo test 全绿：lib 286 + e2e 21；vitest 176 全绿；§3.3 #20/#57 补实装与幂等注记、#59c 增补，§3.4 runSnapshot 定名，§5.6 待审批恢复字段级，§5.8 FrameHub 口径，变更记录 v2.13）

## 决议记录（2026-10-30 实现定案）

- **FrameHub 落位与职责**（§5.8 实装形态）：枢纽落 `agent/run_manager.rs`（票面建议位置），持有 `{ buf: VecDeque<SequencedFrame>（512）, dropped, next_seq, out: Option<Box<dyn SequencedSink>> }`；`SequencedSink` 为新抽的下发端 trait（backend/frames.rs）——seq/ts 由枢纽统一填写，生产适配器退化为纯 Channel 转发，测试注入收集器，agent 模块不依赖 tauri 类型。sink 装饰链变为 `EventfulSink（usage 差分 + RunState 同步）→ PersistingSink（canonical 落盘）→ HubSink（枢纽）`；压缩编排帧也经枢纽出槽（compactDone 随重放恢复）。重放与热切换在同一把枢纽锁内完成：切换后新帧必在全部重放帧之后经新通道发出（帧序严格不乱）。
- **sinceSeq 语义钉死**：sinceSeq = 前端已见最大帧序号，`0 = 一无所见`（重载后唯一取值）——此时重放区间为 `[0, head]`（含 seq 0）；非 0 取值重放 `(sinceSeq, head]`。窗口耗尽判定 = 重放起点之前仍有环外不可见帧（`oldest > 起点`），比票面"dropped>0 即发"更精确（dropped>0 但前端已覆盖缺口时不重复对账）。
- **RunState 同步口径**：`RunState` 增 `usage`（EventfulSink 在 usage 帧处写入运行累计载荷）+ turn（stepStarted）+ tool_count（工具结果帧计数，含硬拦/取消/拒绝的补发结果帧——与 DriveState 口径在审批拒绝场景差 1，RunSnapshot 为运行时观察值以帧面为准）；`last_seq` 读取时以枢纽实际序号覆盖（状态层不重复记账）。
- **runSnapshot 摘要帧**（§3.4 回写定名）：负载对齐 §3.2 RunSnapshot 运行中形态，占用一个新 seq 号、不进环形缓冲（幂等对账辅助帧）；`RunSnapshot`/`RunSnapshotStatus` 类型落 backend/frames.rs（与 UsageTotal 同层的帧面镜像），命令返回 `Option<RunSnapshot>`（manifest 非名义豁免，生成物显式补根导出）。
- **待审批恢复命令面**：新增 `session_pending_ask`（§3.3 #59c 回写），`SessionPendingAsk = { askId, runId, kind, payload }`——`AskSlot` 增 `payload` 字段留存开启时原始载荷（审批与问卷两路径都留存）；有效性以后端 ask 槽为权威（收口即查不到）。前端恢复 = 查询命中按 askId 登记 AskPanel；`resolve` 返回 E_NOT_FOUND（重连期间已解决的竞态）时 AskPanel 按 askId 关闭弹窗，不残留可操作面板；重载本身不代表批准或拒绝（attach 不改 run 生命周期，e2e 钉住）。
- **观察态承载**：新文件 `chat/run-reconnect.ts`（store + 流程）+ `chat/ReconnectOverlay.tsx`（覆盖层渲染）。观察态 = absolute 覆盖层（顶部细条「已重连到运行中的任务」+ 取消按钮 + 只读实时视图），天然隔离输入（防 start_chat 撞 E_SESSION_RUNNING）；帧喂独立 FrameBuffer 不注入 assistant-ui LocalRuntime、不触碰 noteRuntimeNode（§6.3 双写旁路）；"已见最大 seq"提升为会话级状态（store 记账）；runEnded 收口经 reloadTick 信号驱动 SessionTab 重载 canonical 树；5 秒无帧且快照 idle 兜底收口（进入观察态起 5 秒静默期防长工具误触）。恢复待审批查询在快照判定之外独立执行（与观察态无耦合——不进观察态的重连也恢复审批）。
- **回写偏差**：票面"重放区间 `(sinceSeq, 当前头]`"在 sinceSeq=0 语义下按字面会漏掉 seq 0 帧，实现按"0 = 全量重放"钉死并回写 §5.8；摘要帧触发条件按缺口精确判定（票面"或 dropped>0"的宽口径收窄，见上）——两处均为字段级实现澄清，不改变行为承诺。

检查链（2026-10-30）：cargo fmt --check / cargo build / cargo clippy --all-targets / cargo test（lib 286 + e2e 21 + spike + registry 全绿）✓；pnpm typecheck / pnpm test（176 全绿）/ pnpm lint ✓。
