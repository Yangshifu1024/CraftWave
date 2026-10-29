# 实现：followUp 后端队列与 Alt+Enter

Type: task
Status: resolved

Blocked by: 05

## 要做什么

C-3 定案（§5.5/§5.2 字段级）：消息排队从"M1 前端驱动（runEnded 后自行重新 start_chat）"升级为后端会话级队列——运行中 Enter=steer 插话（注入当前 run 不终止）、Alt+Enter=followUp 排队（当前 run 完成后自动投递），界面明确两种语义；中止时返还队列（FR-01-07 后半）；队列持久化跨重启不丢（FR-01-11 的队列半）。

## 具体做什么

### 后端（src-tauri/）

- **inject_run_message 命令**（§3.3 #21，当前未注册）：`{ sessionId, text: string } → { queued: boolean }`，注册进 `ipc_commands!`。M2 语义分叉：
  - 会话有活跃 run → 入 `steer_tx`（mpsc/watch 队列挂 RunHandle），返回 queued=true；
  - 无活跃 run → 入会话级 followUp 队列并立即返回；
  - 注意 §3.3 #21 现行说明只定义了"queued=true=steer 接纳；false=无活跃 run（M1 前端转 start_chat）"两态——M2 下空闲入 followUp 的返回语义需回写 §3.3 #21（建议返回扩展 `{ queued, mode?: "steer" | "followUp" }`），先回写再实现。
- **steer 消费**（§5.5 第一条 + §5.2 时序）：
  - 消费点 = turn 边界（工具批结果回喂后、下一次模型调用前，`backend/run_loop.rs` 步进循环处）；
  - 追加 user 条目进下一请求的 history（`backend/wire.rs` 构建处）；
  - 回执事件 `run:inject { sessionId, runId, acceptedAtTurn }`（§3.5 P0 键，EVENTS 尚未含，本票实装）；
  - steer 的 user 条目落 canonical（§7.3 时机表 user 行）——steer 消息在前端树/帧面的承载 §3.4 无 user 消息帧，**字段级缺失，先回写 §5.5**（建议：前端以 run:inject 回执本地插入 user 节点 + save_session_tree 同步；Rust RunRecorder 落同 id 条目，两侧 id 规则回写时定）。
- **followUp 队列**：
  - 会话级 `VecDeque<FollowUpEntry>`（建议新 `host/followup.rs` 或挂 RunManager 旁；Entry 含 text/createdAt/附着 runConfig 摘要）；
  - 投递时机 = run 正常完成且终态处理完（runEnded 帧发出 + RunManager.remove 之后，§5.2"followUp 队列头 → 自动作为新 run 重放"）；
  - **失败暂停（2026-10-29 用户决策）**：run 失败时暂停队列，持久保留全部待发消息，等待用户明确继续；不得因任何 run 终态都自动投递。队列暂停态跨重启保留；用户取消仍按下述返还草稿规则处理。前端提供暂停状态与继续入口，状态和操作的字段级契约先回写 §5.5/§3.3，再实现。
  - 队列头出队 → 以其文本构造 user 消息自动 start_chat 新 run（user 条目由 Rust 侧落盘，§6.3 双写协议 start_chat 兜底补落路径）；
  - 新 run 复用同一 Channel（§5.2"同通道"），seq 随新 run 新纪元——webview 重载场景经 attach_run（05 号票）续流；
  - 投递前复查无活跃 run（runEnded 与 remove 之间有并发 start_chat 撞 E_SESSION_RUNNING 时，队列头回队重试，投递循环不丢消息）。
- **取消返还**（§5.5 末条 + FR-01-07）：
  - `cancel_run` 生效的 run 收尾时清空本会话 followUp 队列（含持久化文件），内容返还前端草稿区展示；
  - 返还通道：前端收到既有 `run:cancelled` 事件后清空本地队列态并回填 drafts（`stores/sessions.ts` 现成机制）；后端清空前的文本交还字段级未定则回写 §5.5。
- **备份恢复（Q41）**：手动恢复升级备份后，全部待发队列保持暂停，等待用户明确继续；不能因恢复了旧 queue.json 或 ui-state 自动投递。与 33 号票的恢复标记接线，普通 WebView 重连不受影响。
- **队列持久化**（FR-01-11 队列半 + 验收"跨重启不丢"）：
  - 存储路径 §7.7 未列，**先回写 §7.7**（建议 `sessions/<sid>.queue.json`，入队/出队/清空时 `util/atomic` 原子写）；
  - 启动/会话加载时恢复进内存队列，重启后恢复队列及暂停态；未暂停队列在下次 run 正常完成后投递，暂停队列等待用户明确继续。
- 带图消息不入 steer、进 followUp（§5.5）——image part 随 17 号票，本票在 Entry 结构预留 attachments 字段即可。
- 现状锚点：
  - `host/commands/chat.rs` RunConfig 已有 `steer: bool` 字段，注释明言"steer 注入随 §5.5 后续票（本票前端恒不传 true）"——命令面占位已留、逻辑为零；
  - `agent/run_manager.rs` 头注释"用户插话队列（steer）为后续工单"；
  - §5.2 时序图已画"steer 队列非空且本 turn 已收尾 → 追加 + run:inject"与"followUp 队列头 → 自动作为新 run 重放（同通道）"两步——本票即这两步的实装。
- 事件面顺带项：`run:start { sessionId, runId }`（§3.5 P0 键，EVENTS 注释点名"随对应票填充"）随本票实装——自动投递的新 run 与手动 start_chat 都发，前端用它刷新队列态与运行态记账；`run:inject` 键同批注册进 manifest events。

### 前端（ui/）

- 快捷键：`chat/composer-keys.ts` 扩展三态路由——
  - 空闲 Enter = 普通发送（现状）；
  - 运行中 Enter = steer（调 inject_run_message，queued=true 即本地插入 steer 消息节点）；
  - Alt+Enter = followUp（任何时刻可排队）。
- RunConfig.steer 字段随之复审：后端接管后该字段去留回写 §3.2（后端队列下前端不再需要以 steer=true 调 start_chat）。
- 队列展示（Composer 上方队列条）：N 条排队、每条可删/可上移编辑；steer 消息在时间线带"插话"徽标、followUp 在队列条带"排队"徽标——两种语义视觉区分（FR-01-07"界面明确两种语义"）。
- 队列态 store：`stores/sessions.ts` 增 per-session 队列数组（与 drafts 同桶、经 get/set_ui_state 防抖持久化——FR-01-11 的 UI 现场恢复半）；后端队列文件为权威，前端队列条以事件（run:inject/run:cancelled/runEnded 自动投递/run:start 新 run）驱动增删。
- 自动投递的帧面：新 run 的 runStarted（新 runId）到达即新 adapter 实例接手（`chat/runtime.ts` maxSeq 为 per-adapter 局部变量，天然隔离）；观察态路径由 05 号票重连逻辑覆盖。

### 测试

- 回归：前一任务失败时，下一条有副作用的 followUp 不启动；全部待发消息与暂停态跨重启保留，用户明确继续后恢复投递。

- Rust 单测：
  - steer 入队/turn 边界消费/acceptedAtTurn 回执；
  - 空闲入 followUp；队列头自动投递（mock 后端驱动第二 run）；
  - 取消清队列；queue.json 原子写与重启恢复。
- e2e（`tests/backend_e2e.rs`）：
  - 运行中 inject → 下一请求 history 含插话 user 条目 + run:inject；
  - run 正常完成后自动投递队列头（两连发场景：run1 正常完成 → run2 自动开始 → 队列空）；
  - cancel 后队列清空且持久化文件删除。
- 前端 vitest：composer-keys 三态路由；队列条增删与徽标；队列 store 持久化往返；contract.test 扩域（inject_run_message 命令 + run:inject/run:start 事件键）。

## 边界（不在本票）

- steer 消息带图（17 号票 image part 落地后由 Entry.attachments 通路承接）。
- 压缩触发点与队列的交互（04 号票 ask 关闭后压缩已含等待场景；投递前压缩时序如遇冲突在 04 号票 Answer 记录）。
- 定时任务/后台服务产生的 run 复用队列（M3 §5.10）；多会话并行下的全局投递节流（单实例单窗口，M1 口径不变）。
- §5.7 看门狗（37 号票；E_RUN_STALLED 作为失败进入队列暂停，不自动投递下一条）；steer 消息的编辑/撤回（文档无定义，不做）。

## 验收

- [x] 正常完成自动投递；失败暂停并保留全部待发消息，重启后仍暂停，只有用户明确继续才恢复投递；取消仍返还草稿。（e2e：`followup_auto_delivery_after_done_two_runs_back_to_back` / `failure_pauses_queue_until_explicit_resume` / `cancel_clears_queue_and_deletes_persisted_file`；单测：followup_test 暂停态跨重启恢复）

- [x] 运行中 Alt+Enter 排队成功显示在队列条，run 正常完成后自动投递开新 run（帧面连续，无需用户操作）。（composer-keys 三态路由 + FollowUpQueueBar 队列条；自动投递新 run 帧面经 run:start 事件 → attach 观察态续流（05 号票机制复用），runEnded 收口 reloadTick 树重载，用户零操作）
- [x] 运行中 Enter 插话在 turn 边界被消费，模型回应考虑插话内容，run 不终止。（e2e：`steer_injected_midrun_consumed_at_turn_boundary`——run:inject 回执 + steer 条目落 canonical + 第 2 轮请求 messages 实含插话 user 条目 + runEnded(done)）
- [x] 中止 run 后队列返还前端草稿区，可再次编辑发送。（run:queue.returned 事件 → returnQueueToDraft 清队列 + 追加回填 drafts）
- [x] 排队后重启应用，队列与暂停态仍在；未暂停队列在下次 run 正常完成后投递，暂停队列等待用户明确继续。（queue.json 原子写 + 惰性恢复，followup_test `queue_file_atomic_write_and_reload_across_restart`）
- [x] steer 与 followUp 在 UI 上语义可区分（徽标/位置）。（steer=时间线尾部「插话」徽标节点；followUp=Composer 上方队列条「排队」徽标）
- [x] §3.2/§3.3 #21/§5.5/§7.3/§7.7 回写完成（返回形态、steer 树承载与 id 规则、队列存储路径）。（v2.14：#21 返回扩展 mode?/entryId? + 增补 #59d/#59e；§5.5 steer 树承载 `{runId}:steer:{n}`；§7.3 增 steer 消费行；§7.7 增 `sessions/<sid>.queue.json`）

## 决议记录（2026-10-30 实施定稿）

- **#21 返回形态再扩展**：票面建议 `{ queued, mode? }`，实施加 `entryId?`（followUp 模式返回队列条目 id）——前端队列条的删除/编辑取出需要真实条目 id，本地占位 id 与后端 `fu-{uuid}` 不同步会让删除命令必然 E_NOT_FOUND。
- **新增命令 #59d `resume_followup_queue` / #59e `remove_followup`**：失败暂停的「明确继续」与队列条删除都需要后端命令（票面只点名了继续入口与可删，命令面字段级由本票补齐并回写 §3.3）；「上移编辑」降级为「编辑取出」（删除 + 文本回填输入框 + 改完重新排队）——文档无上移交互的字段级定义，删除+重排可达同等效果。
- **新增事件键 `run:queue`**（§3.5 M2 增补第 11 键，总 32 触顶）：队列条以事件驱动增删/暂停态；`returned` 数组承担取消/失败的文本交还（含 steer 未消费残余，前端本地数组没有这部分）。重启/重载的队列展示恢复走 ui-state（queues/queuePaused 桶）。
- **自动投递新 run 的帧出口**：后端侧无前端通道——`DetachedSink` 起手（帧只入环形缓冲不下发），前端收到 `run:start` 事件且本会话非运行态时调 reconnectIfNeeded 走 attach 观察态（05 号票机制复用，observeRun 加防重入）；runEnded 收口 reloadTick 树重载。「帧面连续」以此达成（ReconnectOverlay 实时只读视图 + 收口自动重载，用户零操作）。
- **steer 树承载**：采纳票面建议形态——Rust 消费点直接 append_canonical（id `{runId}:steer:{n}`，父=消费时刻链尾）+ 前端以 run:inject 回执本地插入时间线节点（不入 assistant-ui 树——LocalRuntime 运行中 import 会打断流式快照，风险不可控）；树重载收养后按 entryId 在 adoptTree 里剔除本地节点（放弃订阅 thread.messages 方案——assistant-ui 该 selector 引用不稳定，会触发 useSyncExternalStore 无限重渲染）；save_session_tree「仅追加缺失节点」语义保证前端树暂缺该节点不丢失不漂移（§6.3 既有语义实证）。
- **steer 消费点取 turn>1**：drive_run 的 CallModel 拍次在 turn>1（上一 turn 已收尾）时消费——首拍不消费（插话不并入模型见到的第一批输入，§5.2「本 turn 已收尾」口径）。已知限制（记录不阻塞）：rig AgentRun 内部 history 不可回溯，注入只生效于下一拍请求；若模型在消费后又发起多轮工具调用，后续拍次的请求不再携带插话文本（会话层面不丢——steer 条目已落 canonical，下个 run 的 wire 重建含它）；M1 实证场景（消费后模型直接答复）不受影响。
- **AppState 全字段共享句柄化**：run 收尾的自动投递需要以完整状态面重走 start_chat_core——session_usage/projects/ui_state/exit_confirm 包 Arc + `share_handles()`（Arc 克隆零深拷贝），替代「投递依赖细粒度克隆件」方案（start_chat_core 依赖 config/events/store/session_usage/projects/followups 六面，克隆件装配重复且易漂移）。
- **取消清空 = 删除 queue.json 文件**：票面测试节「cancel 后队列清空且持久化文件删除」按字面落地（clear 时 remove_file）——空队列无信息量，重启后惰性加载默认空队列语义一致；Q41 的恢复暂停不受影响（33 号票恢复流程改写的是暂停态文件）。
- **Q41 接线点**：暂停态随 queue.json 持久化且跨重启保留（followup_test 实证）；恢复旧升级备份后的强制暂停由 33 号票恢复流程改写 queue.json 达成（queue.json 即权威），本票保证「改文件即生效」（惰性加载按文件恢复）。
- **前端队列迁移**：M1 的 `unstable_enableMessageQueue` 撤除（assistant-ui 前端队列与后端队列双轨会重复投递）；队列条改自绘（FollowUpQueueBar：排队徽标 + × 删除 + 铅笔编辑取出 + 暂停横幅 + 继续按钮）。
- **RunConfig.steer 复审结论**：字段保留为恒 false 的契约兼容占位（§3.2 回写），前端已无任何 steer=true 调用面。
