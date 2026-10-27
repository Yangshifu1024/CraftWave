# 15. 实现：run 生命周期落盘与历史恢复

Type: task
Status: resolved

## Question

把 §7.3 持久化时机表全量落进 storage，并接通前端历史加载与分支同步——本票完成后，CraftWave 重启不丢已提交历史（NFR-06/D15），工具卡可从 toolres 边车恢复（FR-04-07）。

范围（Rust）：

1. **run 生命周期落盘（§7.3 时机表逐行）**：start_chat user 条目；工具结果（所属 turn 条目逐 turn 落 + toolres 边车逐结果落）；turn 文本完成落条目；run 终态落 runMarker + meta(revision) + index 更新。
2. **index.json 读写器（§7.1）**：`sessions/index.json` 原子写（§7.4），SessionMeta upsert；`list_sessions` 命令（§3.3 #14）。
3. **toolres 边车（§7.5）**：`sessions/<sid>.toolres/<callId>.json`（注意实际数据根布局 histories 同级），5MB 上限拒入；`load_tool_outcomes` 命令（§3.3 #29）。
4. **崩溃恢复（§7.3 末行）**：load 时发现无 runMarker 收口的 run → 追加 interrupted 修复行。

范围（前端）：

5. **历史加载**：启动/会话进入时 `load_session_tree` → `entriesToExported` → `thread.import()`。
6. **run 终态树同步**：runEnded → save_session_tree；运行态节点（assistant-ui 生成的 run 产物消息）不进载荷（剔除 + 引用改写为 canonical 派生 id）。
7. **分支/编辑/重生成 UI 接地（§6.3 钉死的 API）**：编辑重发、重生成、分支切换的操作入口与 save 同步。
8. **工具卡历史恢复**：渲染历史 toolCall part 时按 callId 批量拉边车。

关键设计决策（本票钉死，Answer 回写文档）：

- **canonical assistant 条目 id 规则 = `{runId}:{turn}`**（帧面 runStarted.runId + StepStarted.turn 两侧同源可派生）；user 条目 id 沿用前端传入（天然同源）。前端树运行态节点与 canonical 节点粒度不同（assistant-ui 单 run 单消息 vs turn 级条目），故运行态节点不进 save 载荷，重启/加载后树即 canonical 形态。
- **runMarker 落独立 message 条目**（parts=[RunMarker]，id=`{runId}:marker`），§6.2 重建时跳过。
- start_chat messages 载荷为活动路径线性序（13 号既有形态），canonical 追加只取本条新 user。

不在本票：create/delete/rename_session 与项目注册（多会话票）、attach_run（M2）、压缩（M2）、跨进程写锁（1.x）。

## Answer

2026-10-28 完成（分支 `feat/run-persistence`）。验证：cargo fmt/build/clippy(-D warnings)/test 全绿（lib 210 + 集成 24+），前端 typecheck/test/lint 全绿（89 例，新增 session store 8 例 + RunRecorder 4 例 + storage 5 例 + e2e 1 例）。

### 核心决策：canonical 条目 id 两侧同源 + 双写协议

设计文档 §6.3 原只说"双写、canonical 先发生"，未钉 id 同源机制——本票补齐（v2.6 已回写）：

- **canonical assistant 条目 id 规则**：文本 turn `{runId}:{turn}`、工具 turn `{runId}:{turn}.tools`、runMarker `{runId}:marker`。Rust 落盘派生，前端可从帧面（runStarted/stepStarted）同规则派生。
- **粒度分歧的解法**：assistant-ui 单 run 单消息（adapter 接口硬约束）vs §6.1 turn 级条目——**运行态节点不进 save 载荷**：发送流程 = append → save（活动路径线性链，parentId 按线性序推导；运行态节点剔除、链指针与 activeLeafId 改写为其 `{runId}:marker`）→ start_chat。运行中树保留运行态节点渲染（无重载闪烁），重启 load→import 后树即 canonical turn 级形态。
- **user 条目拓扑**由发送前 save 保证（编辑分支的 parent 正确）；start_chat 侧同 id 幂等兜底（headless/异常路径，parent=史册链尾）。
- **E_SESSION_REV 自愈**：canonical 落盘推进 revision 而前端基线过期时，load 校正后重试一次（§6.4 重载重试的产品化）。
- **往返一致性**：toWire 补 status/runId 透传；argsJson 两侧统一"parse 保序 + compact 输出"（Rust `serde_json::to_string` / 前端 `JSON.stringify(args)`），漂移消除。

### Rust 侧落点

- `storage/index_store.rs`（新）：SessionMeta + sessions/index.json 原子写，upsert/updatedAt 倒序；损坏降级空表重建（可删可重建铁律）。
- `storage/toolres.rs`（新）：边车读写（5MB 上限占位 + callId 文件名防御）；`load_tool_outcomes` 缺文件调用不在返回表（前端 summary 降级）。
- `storage/session_tree.rs` 增：`append_canonical`（canonical 追加原语：首条自动写 session 头、meta 推 revision/activeLeaf/title 沿用）+ `repair_interrupted_runs`（读时修复：**只认 canonical 形态条目**（id 前缀 `{runId}:`），save 直接落的任意 id 条目不误报——此口径修复过程中由 10 号票既有测试逼出）。
- `host/run_persistence.rs`（新）：`RunRecorder`/`PersistingSink`——帧观察器聚合落盘（usage 帧收口文本条、批内最后 toolResult 收口工具条、runEnded 收口 marker + index upsert 含 totalTokens 四项和）。混合 turn 两条条目（文本先工具后，与真实帧序一致：Usage 先于 CallTools 执行）。落盘失败 eprintln 不中断 run（M1 取舍）。
- `backend/exec.rs`：ExecutorEnv 增 data_root；四类工具完成点 persist_outcome 写边车（Blocked/取消不写，帧面 summary 降级）。
- `host/commands/chat.rs`：sink 链 = EventfulSink(PersistingSink(ChannelSink))；user 兜底；ToolsContext/边车根统一 `app.store.root()`（修掉 default_root 直连——测试注入临时目录不再污染真实 `~/.craftwave`）。
- `lib.rs`：SessionStore Arc 化共享（命令层与 RunRecorder 同一锁表/索引）；注册 `list_sessions`/`load_tool_outcomes`。

### 前端侧落点

- `stores/session.ts`（新）：revision 基线 + 运行态节点记账（adapter 从 `unstable_assistantMessageId` × runStarted 帧配对）+ `toSavePayload`/`syncTreeBeforeSend`/`loadHistory`。
- `chat/runtime.ts`：adapter run() 开头 save（deps.syncTree 可注入，测试 no-op）；toWire 补 status/runId。
- `App.tsx`：启动收养（loadHistory → `loadedTreeToExported` → `thread.import`，NotFound=新会话跳过）。
- `Thread.tsx`：分支操作条（官方原语：BranchPicker Previous/Count/Next + ActionBar Reload/Edit/Copy；user=编辑重发，assistant=重生成+复制）。
- `ToolCards.tsx`：终态卡片按 callId 拉边车，折叠展示 detail/data 全量 JSON（truncated 提示 spill）；各卡片专属排版留桌面体验票。
- `ipc/client.ts`/`types.ts`：listSessions/loadToolOutcomes 接线 + SessionMeta/ToolOutcomeJson 镜像。

### 已知取舍（shortcut 清单）

- 落盘失败不中断 run（帧面可用性优先）；缺行由下次 load 的崩溃恢复补 interrupted 标记。升级条件：桌面体验票后按用户反馈决定是否改为终止。
- index.json ≤2000 条归档未实现（M1 会话量级远不触及；多会话票再补）。
- 崩溃恢复只补 marker 行，不回填该 run 尾条的 incomplete status（重建与渲染不受影响；status 精修随桌面体验票）。
- runEnded 后不强制收养（避免每次 run 重挂载列表闪烁）；canonical 树形态在下次启动/加载时生效。
