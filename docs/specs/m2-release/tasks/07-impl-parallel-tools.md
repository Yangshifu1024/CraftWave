# 实现：并行工具执行与批次调度

Type: task
Status: open

## 要做什么

FR-03-11 + FR-04-04（§5.4/§8.3 调度规则字段级）：同批工具从"M1 串行执行"升级为受控并发——策略门 → 同批写冲突检测 → 权限排除 → 执行（文件写串行、无冲突并发默认 4，可配）→ 取消收齐，结果按源序回喂。AI 一次交代的多件事真正同时干，且写冲突、审批、取消语义一个不坏。

## 具体做什么

### 后端（src-tauri/）

- **调度管线**（FR-04-04 五步，落 `backend/run_loop.rs` 执行段 + `backend/exec.rs`）：
  1. 策略门（Q35）：批内混有 ask（Interactive）和其他工具时只执行 ask，其余调用明确返回“未执行，等待回答后重新决定”，无副作用；模型看到回答后重新发出所需调用，不能继续执行同批预先生成的参数。suggest 属 run 尾后台调用，不进本批。先筛选本批真正执行的调用，再进行既有门控预判；Suspend 时挂起零执行，不为被跳过的工具请求审批。未执行结果的错误码与帧/恢复契约先补齐 §5.4，再实现。
     - **多个 ask（Q42）**：同批只执行源序第一个 ask，其余 ask 也明确返回未执行；模型收到首个回答后再决定下一问。互不依赖的问题放在单个 ask 的 questions 中；停止操作仍取消整个 run，不新增跳过协议。
  2. 同批写路径冲突检测（§5.4 ②，M1 挂账的 §8.4 步骤 4 组内预演在本步落地）：
     - write/edit 先 safe_join 求 canonical 路径，同文件多个 edit 统一基于修改前内容预演，不重叠则允许合并原子提交；区间重叠，或同文件混入 write，则拒绝后续冲突调用（E_INVALID_ARG + 冲突说明回喂，Q32）；
     - 同批 edit 统一基于修改前内容预演目标态（§8.4 步骤 4：全部 edit 先算替换区间，区间重叠后者拒绝）；
     - 预演通过的整组提交走 §8.4 步骤 5 批次事务（备份原文件到 tmp → 倒序写入 → fsync → 任一失败回滚整组）。
  3. 权限排除：既有门控判定（§8.5 矩阵 + fence）已在批级预判完成，Filtered→E_TOOL_BLOCKED 帧内标记不变。
  4. 执行：`backend/exec.rs` 执行器从批内 for 循环串行改为 tokio JoinSet 并发——
     - FileWrite 能力（write/edit）走全局互斥队列（§5.4 ⑤"文件写全局队列"，跨会话串行，防两会话同写一文件）；
     - ReadOnly/Meta 无冲突并发，信号量默认 4。
  5. 取消收齐（S5 不变量"runEnded(cancelled) 必为收齐后最后一帧"的并发版）：
     - cancel 置位 → 停止派发未开始任务 → join 全部在飞任务（命令类走既有进程组收割 §8.8/S5）；
     - 已完成结果保留、未执行调用补 `toolResult.ok=false + E_CANCELLED`（既有 `ToolExecution::Cancelled{executed}` 语义平移到并发形态）→ 才发 runEnded。
- **结果按源序回喂**（FR-03-11）：`answer_all` 回喂顺序 = 模型调用源序（完成顺序不改序——JoinSet 收集后按批内下标重排）；toolResult 帧面亦按源序发出（前端镜像工具卡按 callId 配对，不依赖帧序，保序降低语义噪声）。
- **并发度配置**：默认 4、可配——载体 §3.2 未含（AppConfig/RunConfig 均无），**先回写 §3.2 定字段**（建议 AppConfig 增 tools 组：`{ maxParallel: number }`）。FR-03-11 的〔RIG tool_concurrency〕标注在回写时澄清：本仓为 S3 sans-IO 手动工具循环（`rig::run::AgentRun` 步进、工具由 CraftWave 自管执行），rig 侧 tool_concurrency 不适用，自研信号量即等价配置面。
- **帧面**：无新增帧/事件（toolStarted/toolArgs/toolResult 并发交错属帧序既有语义；seq 严格递增由帧出口保证——与 05 号票 FrameHub 的接缝：入环顺序 = 发送顺序）。
- 现状锚点：
  - 执行串行点 = `backend/run_loop.rs` 批内 for 循环逐调用 `executor.execute`（约 :331）；`backend/exec.rs` 头注释明载"串行执行（单 run 内天然串行）"；
  - 取消语义载体 = `ToolExecution::Cancelled { executed, message }`（run_loop.rs；executed=false 为批内尚未执行，补标记不留悬空调用）——并发版即"未派发"与"在飞被收割"两态的映射；
  - 全局写队列的锁先例 = `storage/locks.rs`（进程内 MutexMap 与锁序纪律），FileWrite 全局互斥可复用其风格新立 tools 域锁或 tokio Semaphore(1)；
  - 双通道结果（§8.2 compact_text 截断/toolres 边车）与并发无关，不改。

### 前端（ui/）

- 无结构性新组件：镜像工具卡（`chat/toolkit.ts`/`ToolCards.tsx`）按 callId 独立状态机，天然支持同批多卡并行推进（运行中/完成互不阻塞）。
- 本票仅回归验证并发交错帧序下卡片状态与 FrameBuffer 合并不串卡（14 号票跨轮 callId 撞车修正 `call_part_{turn}_{n}` 的回归面）。
- `runMetrics`/usage 累计不受并发影响（usage 帧仍 run 级累计）。

### 测试

- Q35 回归：ask“选 A/B”与 write(A) 同批，回答 B 后 write(A) 从未执行且明确返回未执行；下一轮由模型按回答重发；挂起/恢复不意外执行已跳过调用，所有调用结果配对完整。
- Q42 回归：多个 ask 仅源序首个展示、其余不占审批槽且返回未执行；单个问卷多问题正常；首个问卷停止后取消整个 run，不弹出后续问卷。
- Rust 单测（`backend/exec_test.rs`/run_loop 测试扩展）：
  - 并发上限信号量（8 只读调用峰值并发 ≤4）；
  - FileWrite 全局串行（同批两 write 时序先后、跨会话互斥）；
  - 同文件非重叠 edit 可合并原子提交；同批冲突拒绝（write+edit、edit+write、write+write、edit 区间重叠均拒后续冲突调用）；
  - 批次事务回滚（组内第二个 edit 落盘失败 → 第一个回滚）；
  - 源序回喂（慢的先完成仍按源序 answer_all）；
  - 取消收齐（在飞任务 join 后才 runEnded）。
- e2e（`tests/backend_e2e.rs`，ScriptedBackend 驱动）：
  - 同批混合读写批次（读并发 + 写串行 + 源序结果）；
  - 取消收齐不回归（S5 既有断言保持）；
  - 审批挂起批级预判与并发的组合（挂起恢复重放零重复执行——14 号票语义回归）。
- 前端 vitest：并发交错帧的 FrameBuffer/工具卡回归用例（同批多卡各自推进）。
- §12.5 回归门：既有全部 e2e（围栏/审批/ask/suggest/取消）零回归。

## 边界（不在本票）

- 写后检查（FR-04-06，§8.3 M2 表的 `{enabled, command, timeoutSec: 30, tailChars: 3000}` 配置）归 22 号票；本票只做 §8.4 步骤 4/5 批次事务。
- AST fence（22 号票）与子代理 worktree 隔离/结果回收（10 号票）；原跨子代理共享目录文件认领方案已被替代，父级同批冲突检测继续复用本票。
- MCP 工具的并发语义（08 号票落地时按本票调度器接入）。
- 单工具内部的并发（如 batch_read 并行读多文件属工具实现细节，不强制）。

## 验收

- [ ] ask 混批只执行 ask，其他调用明确返回未执行，无副作用、不申请其审批；回答后由模型重新决定。
- [ ] 同批多个 ask 仅执行源序首个，其余明确未执行，独立问题可放单问卷，停止仍取消整个 run。
- [ ] 同文件非重叠 edit 基于同一原内容合并原子提交，重叠或混入 write 时拒绝后续冲突调用。
- [ ] 同批混合读写批次：读并行、写串行、冲突写被拒并回喂模型，批次语义正确。
- [ ] 并发度配置生效（默认 4，改配置后峰值并发随之变化）。
- [ ] 取消收齐：并发批中 cancel，runEnded(cancelled) 仍为收齐后最后一帧，既有 S5 e2e 全绿。
- [ ] 审批挂起恢复零重复执行（14 号票批级预判语义在并发下不回归）。
- [ ] 结果按源序回喂有测试钉住。
- [ ] §3.2（并发度字段）、§5.4（ask 互斥语义、执行⑤修订）、§8.4（步骤 4/5 落地注记）回写完成。
