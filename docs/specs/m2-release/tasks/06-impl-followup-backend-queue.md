# 实现：followUp 后端队列与 Alt+Enter

Type: task
Status: open

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
  - 投递时机 = run 终态处理完（runEnded 帧发出 + RunManager.remove 之后，§5.2"followUp 队列头 → 自动作为新 run 重放"）；
  - 队列头出队 → 以其文本构造 user 消息自动 start_chat 新 run（user 条目由 Rust 侧落盘，§6.3 双写协议 start_chat 兜底补落路径）；
  - 新 run 复用同一 Channel（§5.2"同通道"），seq 随新 run 新纪元——webview 重载场景经 attach_run（05 号票）续流；
  - 投递前复查无活跃 run（runEnded 与 remove 之间有并发 start_chat 撞 E_SESSION_RUNNING 时，队列头回队重试，投递循环不丢消息）。
- **取消返还**（§5.5 末条 + FR-01-07）：
  - `cancel_run` 生效的 run 收尾时清空本会话 followUp 队列（含持久化文件），内容返还前端草稿区展示；
  - 返还通道：前端收到既有 `run:cancelled` 事件后清空本地队列态并回填 drafts（`stores/sessions.ts` 现成机制）；后端清空前的文本交还字段级未定则回写 §5.5。
- **队列持久化**（FR-01-11 队列半 + 验收"跨重启不丢"）：
  - 存储路径 §7.7 未列，**先回写 §7.7**（建议 `sessions/<sid>.queue.json`，入队/出队/清空时 `util/atomic` 原子写）；
  - 启动/会话加载时恢复进内存队列，重启后待投递消息在下次 run 收尾时正常投递。
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

- Rust 单测：
  - steer 入队/turn 边界消费/acceptedAtTurn 回执；
  - 空闲入 followUp；队列头自动投递（mock 后端驱动第二 run）；
  - 取消清队列；queue.json 原子写与重启恢复。
- e2e（`tests/backend_e2e.rs`）：
  - 运行中 inject → 下一请求 history 含插话 user 条目 + run:inject；
  - run 收尾自动投递队列头（两连发场景：run1 结束 → run2 自动开始 → 队列空）；
  - cancel 后队列清空且持久化文件删除。
- 前端 vitest：composer-keys 三态路由；队列条增删与徽标；队列 store 持久化往返；contract.test 扩域（inject_run_message 命令 + run:inject/run:start 事件键）。

## 边界（不在本票）

- steer 消息带图（17 号票 image part 落地后由 Entry.attachments 通路承接）。
- 压缩触发点与队列的交互（04 号票 ask 关闭后压缩已含等待场景；投递前压缩时序如遇冲突在 04 号票 Answer 记录）。
- 定时任务/后台服务产生的 run 复用队列（M3 §5.10）；多会话并行下的全局投递节流（单实例单窗口，M1 口径不变）。
- §5.7 看门狗（独立票）；steer 消息的编辑/撤回（文档无定义，不做）。

## 验收

- [ ] 运行中 Alt+Enter 排队成功显示在队列条，run 结束后自动投递开新 run（帧面连续，无需用户操作）。
- [ ] 运行中 Enter 插话在 turn 边界被消费，模型回应考虑插话内容，run 不终止。
- [ ] 中止 run 后队列返还前端草稿区，可再次编辑发送。
- [ ] 排队后重启应用，队列仍在并在下次 run 收尾时投递。
- [ ] steer 与 followUp 在 UI 上语义可区分（徽标/位置）。
- [ ] §3.2/§3.3 #21/§5.5/§7.3/§7.7 回写完成（返回形态、steer 树承载与 id 规则、队列存储路径）。
