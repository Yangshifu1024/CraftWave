# 实现：上下文压缩 compaction

Type: task
Status: open

## 要做什么

FR-03-05（§6.2 压缩调度字段级）：长会话不再"聊着聊着超长报错"——上下文占用达阈值自动压缩 + 手动按钮；压缩前完整转录先落盘（永不丢原始历史）；wire 从压缩边界重建、display 全量保留；压缩生命周期入帧/事件面；与既有分段落盘/崩溃恢复互切（压缩边界落盘后重启可恢复）。

## 具体做什么

### 后端（src-tauri/）

- **存储条目**（§6.1 字段级）：
  - `storage/entries.rs` 增 `StoredCompactionEntry`（serde tag=kind）：`{ kind: "compaction", id, cutEntryId, summary, tokensBefore, createdAt }`；
  - 现状注释明言"compaction 属 M2，暂不定义变体（未知 kind 读入即报损坏）"——定义变体后旧版本读到新条目报损坏为已知取舍，记入 Answer；
  - id 规则 §6.1 未定——回写 §6.1 时钉（建议沿用 §6.3 canonical id 的 runId 前缀惯例，如 `{runId}:compact:{n}`）。
- **阈值与触发点**（§6.2 字段级）：
  - 阈值 = 上下文占用 ≥ compact_threshold（默认 0.6，窗口占比；contextWindow 取 `ProviderModel.contextWindow`，§3.2 default 128000）；
  - **每请求预算检查（2026-10-29 用户确认 Q18）**：每次请求模型前检查本次实际发送内容（含系统指令、工具定义、摘要、历史、本次输入、工具结果及图片），并预留回复空间；覆盖首次请求、大附件/工具结果增长、切换小窗口模型。最近 usage 仅作校准，不能以无 usage 为由跳过。超预算先压缩，仍放不下则暂停保留输入，不静默裁历史。估算器先复用已有实现/依赖，不足时调研再选型；字段级预算与预留规则先回写 §6.2。
  - 触发点 = run 开始前（start_chat 流程 §5.2 时序内、首个模型调用前）与 ask 关闭后（`host/commands/chat.rs` wait_approval 收口处，与 §7.2 封口安全点"run 终态后、ask 关闭后"重合）；
  - 配置字段（AppConfig 增 compaction 组：threshold/timeout）§3.2 未含，先回写再实现。
- **压缩流程**（§6.2 五步）：
  - ① 完整转录先落盘（不动既有分段，append-only 保证原始历史永不丢）；
  - ② 摘要模型调用：复用 `backend/turn.rs` `complete_once`（suggest 后台小调用同路径，chat.rs spawn_suggest 先例）；active model 或专用小模型（未配则 active model）；超时 compact_timeout 默认 180s；
  - ③ 追加 compaction 条目入当前段 + fsync——§7.3 持久化时机表无此行，回写增行；
  - ④ 发 `compactDone` 帧与 `run:compacted` 事件；
  - 摘要 prompt 内容文档未定——回写 §6.2 附要点（输入=cutEntryId 之前区间的完整转录，输出=结构化摘要文本），先回写再实现。
- **失败兜底（2026-10-29 用户决策）**：摘要调用失败/超时 → 事件 `run:compact_failed`（载荷 `{ sessionId }`）；原上下文仍能放入模型窗口则继续，放不下则暂停并保留本次输入，提示重试压缩或开启新会话。不得静默删除未经摘要的早期历史来继续执行；原始记录始终保留。
- **wire 重建**（§6.2 压缩边界）：`backend/wire.rs` 增 compaction 识别——
  - 路径上最近 compaction 条目的 summary 作为 system 前缀注入（位于 custom prompt 之后、对话历史之前；层叠次序与 03 号票 customPrompt 在回写 §6.2 时一并钉死）；
  - `cutEntryId` 之后的条目正常展开、之前的全部跳过；
  - token 预算超限（模型窗口 ×0.9）优先压缩；不得静默裁掉未经摘要的历史，失败按上述兜底规则处理；
  - 注意：start_chat 的 messages 由前端活动路径传入，前端树侧承载形态见前端节。
- **手动命令**：`compact_session`（§6.2 明文"手动压缩命令（M2 命令，参数 {sessionId}）"，但 §3.3 命令表 #32–59 未编号）——回写 §3.3 增补行后再注册进 `ipc_commands!`；运行中会话拒绝（E_SESSION_RUNNING）或定义为对活跃 run 外的快照压缩，取其一并回写。
- **C-9 口径**：摘要调用计入会话累计（`tokens:update` 照发）、不计 turnUsage；压缩不重置会话累计。
- 与既有机制的接缝：
  - 落盘走 `storage/segment.rs` 既有追加通道（compaction 条目计入 §7.2 封口计数 ≥200 条/512KB；触发点与安全点重合，无段内拆分组问题）；
  - 压缩后 save_session_tree 的 revision 协议不受影响（compaction 条目追加亦 revision++，前端以最新 revision 为 base，§6.4 既有协议）。

### 前端（ui/）

- `chat/frames.ts` 增 `compactDone` 帧类型（§3.4 M2 增补帧：`{ seq, type: "compactDone", cutEntryId, tokensBefore, tokensAfter }`），FrameBuffer 透传（通知面，不进打字机内容面）。
- 事件桥（`chat/event-bridge.ts`）接三键：
  - `run:compacting` → Composer 上方轻量指示条"正在压缩上下文…"；
  - `run:compacted`（tokensBefore/tokensAfter）→ 指示条收口 + 用量进度条刷新；
  - `run:compact_failed` → 行内轻提示，不打断输入（Toast 反馈层属 29 号票）。
- 手动按钮落点：用量 Popover 的上下文进度条旁（§10.5 已定义 0.6 阈值标线，进度条=本次实际上下文占用估算/contextWindow，会话累计消耗独立显示）——"压缩"按钮 → `compact_session`；运行中禁用（后端 E_SESSION_RUNNING 兜底）。
- **压缩边界在前端树的承载**：§6.3 未定义 compaction 条目在 assistant-ui 树的节点形态——先回写 §6.3 再实现：
  - 建议形态参照 runMarker（system 空消息 + custom 载荷，`tree-persistence.ts` 的 `readRunMarker` 同款机制）；
  - `entriesToExported`/`toWire` 增 compaction 分支，保证 save_session_tree 载荷与 canonical 条目 roundtrip 一致（否则 E_SESSION_REV）；
  - 时间线上压缩边界渲染为一条分隔提示（"已压缩早期上下文"），非消息体。
- `ipc/client.ts` 增 `compactSession` 出口 + COMMANDS 注册；事件 payload 类型随 02 号票生成物面。

### 测试

- 每请求预算回归：首次大输入、工具结果暴涨、切换小窗口模型和图片均在发送前检查，回复空间计入预算；压缩后仍超预算不发请求、输入保留；上下文占用进度独立于会话累计消耗。

- Rust 单测：
  - 阈值判定表驱动（0.6 上下、无 usage 首轮、contextWindow 取值）；
  - compaction 条目 serde roundtrip（entries 测试扩展）；
  - wire 重建边界（cutEntryId 前跳过/后展开/system 前缀位置；实际请求预算/回复预留、压缩失败超限暂停并保留输入、不静默裁历史的不变量——§12.5 点名的压缩往返属性测试）。
- e2e（`tests/backend_e2e.rs`，ScriptedBackend 驱动）：
  - 跨阈值自动压缩 → compactDone 帧 + run:compacted 事件 + 后续 turn wire 只含 summary+新条目；
  - 摘要失败注入：原上下文可容纳 → run:compact_failed 后继续；不可容纳 → 暂停、输入保留、重试压缩/新会话入口可用，未经摘要历史未被静默裁剪；
  - 手动 compact_session 命令路径。
- 前端 vitest：frames.ts compactDone 解析、事件桥三键分发、tree-persistence compaction roundtrip、contract.test 扩域（compact_session 命令 + compactDone 帧 + 3 事件键）。

## 边界（不在本票）

- 子代理上下文继承（10 号票）；检索/FTS5（M3）；branchSummary 条目（§6.1 表内 M2 可选项，无 FR 承载，随分支需求另立）。
- 压缩用"专用小模型"的选择 UI（配置字段留 AppConfig，设置页表单随 18 号票或后续打磨）。
- run:compacting 等键的系统通知联动（FR-13-06 归桌面票）。

## 验收

- [ ] 长会话跨阈值自动压缩，模型不报超长；压缩后对话上下文连贯（summary 生效）。
- [ ] 手动压缩按钮可用；压缩中/完成/失败三态 UI 均有呈现。
- [ ] 压缩边界落盘后重启应用，load_session_tree 恢复且 wire 重建仍从边界生效（重启后继续对话不重复注入旧历史）。
- [ ] 压缩失败：原上下文可容纳时继续，不可容纳时暂停且输入保留；提示重试压缩或开启新会话，未经摘要的历史不被静默裁剪。
- [ ] tokens 口径遵守 C-9（会话累计不因压缩重置，tokens:update 照发）。
- [ ] §3.2/§3.3/§6.1/§6.2/§6.3/§7.3 回写完成（compact_session 编号、配置字段、id 规则、树承载形态、落盘时机行）。
