# 实现：工具系统接线（command/plan/ask/suggest + 审批门 + 工具卡片 UI）

Type: task
Status: resolved

## Question

工具系统与安全审批雾区的收尾切片（map「工具系统与安全审批」剩余项）。五文件工具、围栏、权限矩阵判定已随 11 号票落地；核心链路（RunLoop + 挂起机制层 + 帧渲染）已随 13 号票落地。本票接线：

1. **四个工具**（§8.3）：
   - `command`（Network）：`{command, timeoutSec?(1–600 默认 120), cwd?}`；Unix `sh -c` / Windows `powershell -NoProfile -Command`；进程组/Job Object 收割（接入 `tools/process` 原语）；stdout+stderr 合流 + ANSI/OSC 清洗；>96KB spill 至 `~/.craftwave/logs/spill/<callId>.txt`（尾部 16KB + spill 路径，24h lazy 清理）；cwd 走 safe_join。
   - `plan`（Meta）：`{action:"set"|"update", items?}`；≤100 项、单 in_progress 校验；PlanState{items, approvedSnapshot?, approvedAt?}；`plan:update` 事件；计划门 §8.6（approvedSnapshot 存在且 items 全 completed 时 FileWrite/Network 写入 → E_PLAN_STALE 回喂）。
   - `ask`（Interactive）：`{title, questions:[{id,text,options?,allowFreeText}]}`；执行=注册 AskSlot → `ask:opened` 事件 → 等 `resolve_ask` → answers 回喂为工具结果；取消关闭（answered=false）。
   - `suggest`（非模型工具，不进注册表）：run Done 后后台小调用一次模型（C-9：计入会话累计 tokens:update、不计 turnUsage、失败静默）；`run:suggestions` 事件。
2. **审批门接线**（§5.4 步骤①–④ × §8.5 矩阵 × §8.7 围栏）：
   - ApprovalGate：已批准放行 → 围栏（command：Block→回喂 E_FENCE_BLOCKED 不中断；Confirm→挂起带 fenceReason）→ 计划门（E_PLAN_STALE）→ 权限矩阵（NeedsApproval→挂起；Filtered→E_TOOL_BLOCKED）。
   - 挂起恢复循环（host run 任务）：Suspended → 构造 AskPayload(kind=approval)（write=新文件全 add hunk、edit=oldText/newText hunk、command+fenceReason）→ `ask:opened` → 等待 `resolve_ask`/取消/autoConfirm 300s 自动 recommended → allow 则 gate 记批准 + resume_run 续跑（usage/tool_count/计时延续、不重发 runStarted）；deny/取消 → 挂起调用补拒绝结果帧 + runEnded(cancelled)。
   - `resolve_ask` 命令（§3.3 #22）与 `approve_plan` 命令（计划门批准快照，§3.3 增补行）。
3. **事件面**：EventSink 抽象（util；生产 tauri emit / 测试 Recording）；实装 5 键：ask:opened/ask:closed/plan:update/run:suggestions/tokens:update（§3.5 payload 契约）；manifest events 键填充 + toolSpecs（name+params_schema）导出供前端 toolkit 同源校验。
4. **前端**：
   - 正式 toolkit（defineToolkit）：read/write/edit/list_files/grep/command/plan 卡片（§10.3 规格：read 行区间、write/edit diff 徽标、command monospace+退出码+输出折叠、grep/list_files 计数列表、plan 计划卡+批准按钮）；ask 落地为极简状态卡（防无卡 fallback 空白，交互在 AskPanel）；suggest 不产生 tool-call part 无需卡。parameters 与 Rust ToolSpec 经 manifest 同源校验。
   - AskPanel + ask store（zustand，事件驱动）：question（选项/自由文本）与 approval（diff/command + 允许/拒绝）两形态 → resolve_ask。
   - askHint store + Composer ghost（§10.4：空输入整句/前缀补尾、Tab 填入不发送）。
   - runMetrics：sessionUsage 增 tokens:update 权威校正（事件到达 replace，帧累计兜底）。

边界：折叠分组/摘要行（桌面体验票）；runMarker/canonical 落盘、toolres 边车、index.json（持久化票）；steer/inject_run_message；§11.3 错误分类重试（provider 票）；§8.4 步骤 4/5 批次事务与写后检查（M2）；always_session 命令记忆与 commandAllowlist 接线；E_PLAN_REQUIRED 严格硬门（默认关）；右栏计划面板（桌面体验票，M1 计划呈现=消息内卡片）。

完成标准：headless 全测（Rust 单测 + e2e：审批挂起→批准/deny、fence Block 回喂、plan:update、ask resolve、suggest 后台调用，mock SSE 驱动）+ 前端 vitest（卡片/AskPanel/store/契约）；检查链全绿；与 §3/§5.4/§8 有出入最小修订文档并记录（§3.3 补 approve_plan、§3.6 补 E_COMMAND_TIMEOUT、§5.4 挂起批级预判语义、变更记录）。

## Answer

（2026-10-27 完成，工作区 main 干跑未提交（提交按用户指令另行执行）。检查链七项全绿：cargo fmt --check / build / clippy --all-targets -D warnings / test（lib 209 + 集成 24，1 例真钥匙串 #[ignore]）+ pnpm typecheck / test（75 测）/ lint。技术设计文档修订六处（变更记录 v2.4）。）

### 1. 模块落位（§2.2）

| 落位 | 内容 |
|---|---|
| `tools/command.rs` | Network 能力；sh -c / powershell 承载；进程组/Job Object 收割接入 `tools::process` 原语；stdout+stderr 合流 + anstream ANSI/OSC 清洗；>96KB spill 至 `<data>/logs/spill/<callId>.txt`（尾部 16KB + 路径，24h lazy 清理）；timeoutSec 1–600 默认 120；cwd 走 safe_join；超时 E_COMMAND_TIMEOUT + data 保留输出 |
| `tools/plan.rs` | PlanState{items, approvedSnapshot, approvedAt} + set/update 纯逻辑（≤100 项、id 唯一、单 in_progress）；`approved_and_all_completed` 守门条件 |
| `tools/ask.rs` | 问卷校验（选项 id 唯一、无选项须容自由文本）+ AskPayload(question) 构造纯逻辑 |
| `backend/ask.rs` | §3.2 AskPayload/AskAnswer/AskSlot/ApprovalDiff 协议类型（question/approval 两形态） |
| `backend/gate.rs` | ApprovalGate：已批准签名 → 围栏（Block 硬拦/Confirm 挂起带因）→ 计划门（E_PLAN_STALE）→ 矩阵（Filtered→E_TOOL_BLOCKED，NeedsApproval→挂起） |
| `backend/run_loop.rs` | GateVerdict 扩展 Suspend{fenceReason}/Blocked{code}；**批级门控预判**；DriveState（计时/usage/计数/callId 映射跨挂起延续）；Suspended(Box<SuspendedInfo>) |
| `backend/exec.rs` | ToolsetExecutor 分发 8 工具：command 取消/超时感知执行；plan 更新共享状态 + plan:update；ask 注册 AskSlot → ask:opened → 等 oneshot；AskSlotCell/ExecutorEnv 挂点（agent 构造、backend 写、host 读） |
| `backend/turn.rs` | `complete_once`（非流式单次补全，suggest 用） |
| `host/commands/chat.rs` | 挂起恢复循环（wait_approval：AskSlot + ask:opened + resolve/取消/autoConfirm300s + ask:closed 单一出口清槽）；AskPayload(approval) 构造（write 全 add / edit 替换 hunk / command+fenceReason）；deny 补 E_CANCELLED 拒绝帧 + runEnded(cancelled)；EventfulSink（usage 帧增量折算 → tokens:update 全量）；suggest 后台小调用（Done 后 spawn，C-9 静默）；resolve_ask/approve_plan 命令 |
| `util/events.rs` | EventSink trait + 5 键常量 + RecordingEventSink；生产 TauriEventSink（host/mod.rs，AppHandle.emit） |
| 前端 | `chat/toolkit.ts`（defineToolkit 8 工具 + CW_TOOL_PARAMETERS 原始表）、`chat/ToolCards.tsx`（7 卡片 + ask 状态卡）、`chat/AskPanel.tsx`（question/approval 两形态）、`chat/event-bridge.ts`（4 键事件分发）、`stores/ask.ts`（ask+hint+matchGhost）、Thread ghost + Tab 填入（aui.composer.setText）、App 装配 AuiConfig(Tools) |

### 2. 两处上游缺陷修正（本票发现并修复，均有测试钉死）

1. **挂起重放重复执行**（13 号票形态）：原「批内逐个判定逐个执行」在首个 Suspend 挂起时，其前已执行的 Allow 调用 answers 未提交（answer_all 未达），恢复重放会重复执行（写类副作用翻倍）。修正为批级预判——Suspend 挂起时批内零执行；§5.4 已按此修订。
2. **跨轮 callId 撞车**：桥接派生 id `call_part_{n}` 的 part 序号按响应各自从 0 计，两个工具轮的 part 0 会派生同一帧面 id（前端 FrameBuffer Map 合并丢卡）。修正为 `call_part_{turn}_{n}`；§3.4 已注明。

### 3. 实现细化记录（§8 未定细节的落地选择，非出入）

1. **command 非零退出码**：ok=false + error E_INTERNAL_STATE（message 注 exitCode），输出仍在 data——模型可读输出自纠；超时为独立 E_COMMAND_TIMEOUT（§3.6 补录行）。
2. **审批放行签名 = (name, argsJson)**：挂起恢复重放生成的模型 call id 与原 id 不同（run 重放），按内容签名放行；同批同签名重复调用会连带放行（相同操作，可接受）。
3. **deny/取消补帧**：run 已序列化、模型侧不再收下一轮，帧面为前端工具卡补 E_CANCELLED 结果（「用户拒绝了此操作」）——§5.3 AwaitingApproval→Cancelled 的帧面落地；无「拒绝」专用错误码，借 E_CANCELLED（票面记录）。
4. **ask:closed 单一出口**：resolve_ask 只投递 oneshot 不清槽不发事件；等待方（executor run_ask / host wait_approval）收口时统一清槽 + ask:closed（避免双发）。resolve_ask 对已答/失效槽返回 E_NOT_FOUND（§3.3 #22）。
5. **autoConfirm**：start_chat 入口读一次 AppConfig.approval.autoConfirm 快照进 run 任务（挂起 300s select 分支）；挂起必非黑名单（Block 不挂起），灾难级例外天然满足。
6. **tokens:update 口径**：帧出口事件层对 usage 帧（run 内累计值）做差分得增量 → 会话累计 → 发全量；suggest 收口补发。前端 runMetrics：帧累计兜底 + 事件 replace 校正（§10.5「以事件为准」的落地）。
7. **suggest 上下文**：最后一条 assistant 文本（缺省取最后一条 user）；mock 非流式请求返回固定建议文案（mock_openai 增 stream!=true 分支——rig Model::call 单次调用期待 chat.completion JSON，SSE-only mock 会静默失败，测试排障记录）。
8. **start_chat_core 防御性建目录**：default_root create_dir_all（command cwd/spill 基准；生产由 setup 创建，测试环境兜底）。
9. **commandAllowlist/always_session**：审批选项只给 allow_once/deny 两项（§3.2 键面含 always_session，M1 不实装会话记忆，票面边界）。
10. **同批写冲突（§5.4 ②）**：M1 串行执行 + edit 三级唯一匹配天然收敛（重叠 edit 自然 NO_MATCH/AMBIGUOUS 回喂），显式批次预演事务（§8.4 步骤 4/5）留 M2；§5.4 已注记。
11. **工具卡片数据面**：M1 帧面只有 uiSummary——read 预览展开/command 输出折叠以 args + 摘要承载，详细 data 恢复随 toolres 边车票（load_tool_outcomes #29）。
12. **manifest 增量**：events 5 键 + toolSpecs（name+paramsSchema）导出，前端 contract.test 双向锁定 toolkit 声明（§10.3「同一份来源」的机制落地）。
13. **测试方法命名**：全仓测试函数名统一英文（用户指令；断言消息保持中文），共重命名 160 处。

### 4. 测试清单

Rust（lib 209 + 集成 24 全绿）：
- 工具单测：command 8（退出码/合流/ANSI/超时收割 E_COMMAND_TIMEOUT/取消收割/spill 阈值与尾部/cwd 越界与合法/参数校验）、plan 7（set 替换/空与重复 id/上限/单 in_progress/update 合并/守门条件）、ask 4（构造/空标题/无选项不容自由文本/选项空列表重复 id）。
- gate 单测 7：四档矩阵汇总表驱动、黑名单全档硬拦、写信号挂起带因、plan 白名单仍被矩阵过滤、计划门拦写不拦读、签名放行（同签名放行异签名不连带）、未知工具放行交 executor。
- run_loop/桥接：e2e 挂起测试升级（挂起零执行断言 + 恢复不重发 runStarted + toolResult 与挂起前 toolStarted 同源）；桥接跨轮不撞车。
- e2e 新增 6：审批链 allow（ask:opened 载荷 + 挂起期无 toolResult/runEnded + 会话占用 + resolve→恢复 Done + 已答 E_NOT_FOUND）、deny（拒绝帧 + cancelled 终态 + 摘除）、围栏黑名单（full_access 也拦 + Block 无 ask + run 继续 Done）、plan（plan:update 事件 + approve_plan 命令 + 无 run E_NOT_FOUND）、ask 问卷（ask:opened(question) + resolve 回喂第 2 轮 + ask:closed）、suggest（run:suggestions 非空 + tokens:update 三段累计 360/144）。
- host 纯逻辑：审批 payload 构造 4（write 全 add/edit del+add/command+围栏因/建议上下文优先 assistant）。

前端（vitest 75 测全绿）：toolcards 10（8 卡渲染 + 相位机 + isError）、ask-panel 4（问卷作答/审批 diff+允许拒绝载荷/空态）、ask store + 事件桥 6（open/close 按 id、matchGhost 四态、四键事件分发各达其 store、卸载退订）、contract 增量（tools 8 + toolSpecs 逐字同源双向 + events 5 键 + resolve_ask/approve_plan 接线与类型锚点）；既有 55 测零回归。

### 5. 与文档的出入及处理（技术设计文档 v2.4，六处最小修订）

§3.3 增补 approve_plan（26c，32→33）；§3.6 补 E_COMMAND_TIMEOUT；§3.4 派生 id turn 前缀；§3.5 tokens:update 实装口径；§5.4 审批门落地口径（批级预判/DriveState/签名放行/deny 补帧 + ②⑤ 的 M1 收敛注记）；§10.3 ask 卡 M1 落地注记。其余实现与 §3.2/§8.3/§8.5/§8.6/§8.7/§8.8 无文字性出入。

### 6. 遗留与去向

- toolres 边车 + 工具卡详细数据（#29）→ 持久化票；折叠分组/摘要行（§10.4）→ 桌面体验票；右栏计划面板 → 桌面体验票；steer/inject_run_message、§11.3 错误分类重试 → 各自票；always_session 命令记忆 + commandAllowlist → M2；suggest 的 rate TextDelta 自算 → 小项票。
