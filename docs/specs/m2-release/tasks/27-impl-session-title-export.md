# 实现：会话标题自动生成与会话导出

Type: task
Status: resolved

## 要做什么

FR-01-09 + FR-01-12（PRD 行 286/289，出处 §3.5/§6.1/§10.5/C-9）：

- 新会话发出第一条消息后自动起名——后台零工具小模型调用生成标题，左栏与卡头即时更新；失败静默、超时兜底截断首条消息（PRD 原文口径）。
- 会话可一键导出 Markdown（人读归档）或 JSONL（机器可读取，含工具调用记录；图片仅外部引用，不是跨机器完整备份）；HTML 分享页 P2 不做。

归属注记：设计文档 §10.7 把「会话导出」标为【M3】、附录可追溯矩阵同（"FR-01-12 导出【M3】"）——本图 Destination 已将其拉进 M2（27 号票）。**动工前先把 §10.7 与可追溯矩阵的 M3 标注改注 M2（随 27 号票）**。

## 具体做什么

### 后端（src-tauri/）

- **触发点与判定**：`host/commands/chat.rs` start_chat 流程内，user 消息落盘后判定：
  - 条件 = `SessionMeta.title` == create_session 默认标题常量（`host/commands/session.rs`）且历史仅此一条 user 消息；
  - 命中即 spawn 后台标题任务，否则跳过；
  - 三个不触发面由此一并覆盖：用户 `rename_session`（#17）改过名、非首条消息、重启后继续旧会话——"重启不重生成"由判定条件天然保证，无需额外标记位。
- **并发防护**：同会话短时双发（首条后立刻编辑重发再发）可能 spawn 两个标题任务：
  - 以会话锁（`MutexMap`，§7.4）内的"title 已非默认"复判收口——任务开始前再查一次 index；
  - 仍可能双生成时以先写者为准（后写覆盖同值无害，记 Answer）。
- **生成调用**：复用 `backend/turn.rs` `complete_once`（suggest 后台小调用同路径，04 号票先例）：
  - 零工具、max_tokens 压到 32、超时 10s；
  - 模型 = 会话激活模型（专用小模型字段 §3.2 未定义，若 04 号票落了 compaction 摘要模型配置则共用，取舍记 Answer）；
  - 输入 = 首条 user 消息文本（超长截 2000 字符），prompt 要求输出 ≤20 字标题、无引号无前缀（prompt 全文回写 §6.3 注记）。
- **超时/失败兜底**（PRD 原文口径）：超时或调用失败 → 标题 = 首条消息首行截断（≤30 字符，换行/前后空白归一；C-9"失败静默"→ 不报错、不提示、不重试）；成功 → 生成文本 trim 后落。
- **落盘双写**：
  - index.json `SessionMeta.title` 更新（`storage/index_store.rs` 既有 upsert 通道）+ `updatedAt` 顺带刷新；
  - 会话段尾追加 meta 行 `{"kind":"meta","title":…,"revision":…}`（§6.1 meta 行 `title?` 字段已预留；**追加前先回写 §7.3 持久化时机表增此行**，fsync 与会话条目同档）；
  - 锁序 session → index 不变（§7.4）。
- **事件**：emit `session:title` `{ sessionId, title }`——§3.5 P0 18 键之一，键位 M0 已定死；`host/commands/mod.rs` EVENTS 注释点名"session:title 随对应票填充"，本票加键，时序在落盘之后发。
- **C-9 计量（Q39/Q40 收口）**：complete_once 后台调用的实际 usage 计入会话及按日/实际模型统计，不计 turnUsage、不增主 run 次数；响应返回 usage 时按当时本地日期落账。失败静默不提示、不重试，但已返回的 usage 仍须计入，不能承诺失败免费；未返回的用量不伪造。重启恢复遵守 Q37 四口径记录。

### 前端（ui/）

- 事件桥（`chat/event-bridge.ts`）接 `session:title` → sessions store 更新 tabs 内对应 SessionMeta.title（`stores/sessions.ts` tabs 为 SessionMeta 快照）；会话不在当前 tabs（后台项目会话）→ 只更新缓存列表，不强行开 Tab。
- 展示点零改动验证：左栏会话行（`chat/SessionWorkspace.tsx` Sidebar 会话树）、卡头 `activeTab.title`（L140 附近）——随 store 更新即生效，不需要新组件。
- **导出（纯前端，无新命令）**：
  - 数据源 = 既有 `load_session_tree`（活动路径 messages，§6.3）+ `load_tool_outcomes`（工具 uiSummary，历史工具卡恢复同源）；工具行明细缺文件时按既有 summary 降级口径处理（#29 语义）。
  - 新模块 `ui/src/chat/export.ts`：`toMarkdown(session, messages, outcomes)` / `toJsonl(...)`。
  - Markdown 形态（§10.7 无格式定义，本票定义并回写）：`# <标题>` + 导出时间行 → 逐条 `## user / assistant`（createdAt + 文本）→ 工具调用以列表行 `- ⚙ name — uiSummary（ok/error）` 附在产生它的 assistant 段内；reasoning 不导出（归档/分享面，记边界）。
  - JSONL = 活动路径 WireMessage 排除 reasoning 后逐行导出（含 toolCall/toolResult parts）；图片 part 只导 blob 路径引用不导内容，不宣称跨机器完整回放。
  - 只导**活动路径**（当前分支），全树导出不做（PRD"分支当前路径"口径）。
  - 下载：`Blob` + `URL.createObjectURL` + `a[download]`，文件名 `<标题>-<YYYYMMDD>.md|jsonl`（标题非法文件名字符替换为 `-`）；
  - 入口 = 左栏会话行 context-menu 增「导出 Markdown / 导出 JSONL」两项（Sidebar 既有 context-menu「删除会话」同位置，`SessionWorkspace.tsx` L343 附近）。

### 测试

- 后台调用失败但已返回 usage 时照常计会话/日/模型用量，不增本轮与主任务次数；无 usage 不伪造，标题仍走静默兜底。
- Rust：默认标题判定（首条/非首条/已改名三态表驱动）；并发双触发只生成一次（锁内复判）；超时兜底首行截断；meta 行 + index 双写一致性；session:title 事件在落盘后发出（事件顺序断言）。
- 前端 vitest：`export.ts` 序列化单测（活动路径、工具记录行、空会话、含图片 part 降级、非法文件名字符清洗）；事件桥 session:title 分发 → sessions store 更新（含会话不在 tabs 的分支）；contract.test 扩域（EVENTS 增 session:title，无新命令）。

## 边界（不在本票）

- HTML 分享页导出（PRD P2）；全树/多分支导出；图片内容导出；批量导出、导出到指定目录（系统保存对话框 rfd 另立）；导出含 reasoning。
- 标题生成的专用小模型选择 UI（随 18 号票设置面或后续打磨）。

## 验收

- [x] 新会话首条消息后数秒内自动得名，左栏/卡头同步；重启后标题保留且不重新生成。（e2e `auto_title_full_chain_first_message` 钉触发与双写；`auto_title_not_regenerated_on_second_message` 钉"标题已非默认 → 判定不触发"——重启续聊/连发同判定面；前端 `applyTitle` 事件就地更新 tabs 与列表，展示点零改动随 store 生效）
- [x] 标题模型调用超时 → 标题为首条消息首行截断，界面无错误提示。（单元 `title_task_timeout_falls_back`（悬空 transport + 短超时确定性触发）与 `title_task_failure_falls_back_silently`（剧本错误）；C-9 失败静默——无事件无提示无重试）
- [x] 改名过的会话再发消息不会被自动标题覆盖；同会话快速连发不会产生两个标题任务竞写。（单元 `title_task_skipped_when_renamed`（复判一：改名后零请求零写盘）+ `title_trigger_three_states` 表驱动；竞写收口 = 任务入口与落盘前两次复判 + 事件同步锚的 `title_task_second_writer_yields`（先写者胜、后写者放弃）；编辑重发使树内 user 计数 > 1 天然不再触发）
- [x] 导出 Markdown 含标题、活动路径全部消息与工具调用摘要行；JSONL 排除 reasoning，逐行结构与活动路径消息一致；图片仅引用，不保证脱离原存储后完整回放。（`export.test.ts` 11 例：Markdown 标题/时间/段落/工具行（边车优先、帧面降级、ok/error 标注）、runMarker 不进归档、空会话；JSONL 逐行仅剔 reasoning、toolCall/toolResult 保留、图片仅 blobHash 引用）
- [x] 标题小调用计入会话累计（tokens:update 照发）、不计 turnUsage（C-9，测试钉住）。（单元 `title_task_success_writes_both_and_records_usage`：tokens:update 载荷 + 明细 kind=aux + runs 计数不增；e2e 计量 480/192 全链收敛）
- [x] §10.7/可追溯矩阵 M3→M2 归属回写、§7.3 持久化时机表增行完成。（v2.33：§10.7 导出改注【M2】+ 形态定稿、附录 B FR-01-12 改注【M2（27 号票）】、§7.3 增"自动标题落盘"行、§6.3 增自动起名注记含 prompt 全文）

## 决议记录（2026-10-30 实施）

1. **标题模型取舍（票面 Answer）**：标题生成跟随压缩摘要模型配置（`compaction.modelId`，04 号票已落）——配置存在且解析成功即复用；未配置**或摘要模型解析失败**（如指向已删除模型）时回退会话激活模型，起名不因摘要配置漂移而静默跳过。未新增专用小模型字段（随 18 号票设置面或后续打磨）。
2. **prompt 全文与归一口径（已回写 §6.3 注记）**：`根据用户的这条消息，给这个 AI 助手会话起一个简短标题。要求：不超过 20 个字；直接输出标题本身，不要引号、不要任何前缀或解释。消息内容：` + 首条消息文本（超长截 2000 字符）。生成结果与兜底共用同一归一函数（trim → 取首个非空行 → ≤30 字符按字符截断）——模型无视 ≤20 字要求或输出多行解释时兜住长度，兜底输入（首条消息全文）由同一函数天然完成"首行截断 + 换行/空白归一"。
3. **并发防护实现口径（票面 Answer）**：复判 = 任务入口一次 + 落盘前一次（`SessionIndex.get` 原子读），复判谓词 = title 仍为 `DEFAULT_SESSION_TITLE`；两次复判之间的极窄窗口下后写者可能覆盖先写者（票面"后写覆盖同值无害"口径——两个合法标题互覆盖，无错误态）。编辑重发场景由判定条件天然消解（树内 user 计数变 2，第二个 start_chat 不再 spawn）。
4. **tokens:update 累计域**：标题任务与 run/suggest 共用同一份会话累计快照（start_chat 内创建的副本表）——若与 run 双表各自累计，前端 replace 语义下后写事件会互相覆盖导致快照漂移（实施中发现并修正，e2e 断言 480/192 全量收敛钉住）。
5. **事件与落盘顺序**：`session:title` 严格在 meta 行落盘之后发出；index mutate 失败仅记日志仍发事件（史册面已有标题；index 由下次 canonical 落盘 upsert 以 prior 补齐，尽力而为面）。测试以"事件到达 ⇒ 断言 index/meta 已更新"锚定顺序。
6. **文件名清洗细化**：非法字符（`/ \ : * ? " < > |` 与 Unicode 控制字符 `\p{Cc}`）替换为 `-` 后连续 `-` 折叠为单个、再去首尾 `-.`（Windows 结尾点非法）——避免 `///` 之类标题产生一串减号，清空后兜底「会话」。
7. **JSONL 形态补充（票面未明说处）**：仅剔除 reasoning part，其余结构原样保留（含 runMarker 条目与空 parts 行）——"逐行结构与活动路径消息一致"优先；图片 part 本就不含字节（§6.1），保留 blobHash/mimeType 即"仅外部引用"。
8. **无 usage 不伪造的实测面**：complete_once 失败（Err）与超时（Elapsed）两条路径都不产生 usage——零计量事件、零落账、零重试（单元与 e2e 各自钉住）；成功但返回空文本时已返回的 usage 照常记账、标题走兜底。

## 遗留（不阻塞验收）

- 左栏排序在事件后不即时重排（updatedAt 已被后端刷新，下次 `refresh` 恢复后端权威序）——标题就地更新语义下无感知差异，未做额外处理。
- `EventfulSink` 副本表与 `AppState.session_usage` 原表并存的既有双源形态（20 号票遗留）不在本票收编范围；标题任务已并入 run 副本表消除本票新增的漂移面。
