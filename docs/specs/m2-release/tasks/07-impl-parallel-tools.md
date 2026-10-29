# 实现：并行工具执行与批次调度

Type: task
Status: open

## 要做什么

FR-03-11 + FR-04-04（§5.4/§8.3 调度规则字段级）：同批工具从"M1 串行执行"升级为受控并发——策略门 → 同批写冲突检测 → 权限排除 → 执行（文件写串行、无冲突并发默认 4，可配）→ 取消收齐，结果按源序回喂。AI 一次交代的多件事真正同时干，且写冲突、审批、取消语义一个不坏。

## 具体做什么

### 后端（src-tauri/）

- **调度管线**（FR-04-04 五步，落 `backend/run_loop.rs` 执行段 + `backend/exec.rs`）：
  1. 策略门：批内含 ask（Interactive）时不并发——suggest 本就非模型工具（run 尾收尾阶段调用，不进批），本步实际语义为"ask 与同批其余调用互斥"；互斥的精确语义（ask 先行、其余串行其后，或整批退化串行）§5.4 字段级未钉死，**先回写 §5.4 再实现**。门控批级预判机制（Suspend 整批挂起、挂起零执行）不动——并发只作用于全 Allow 的批。
  2. 同批写路径冲突检测（§5.4 ②，M1 挂账的 §8.4 步骤 4 组内预演在本步落地）：
     - write/edit 先 safe_join 求 canonical 路径，同批同文件多写 → 后者拒绝（E_INVALID_ARG + 冲突说明回喂）；
     - 同批 edit 组内预演目标态（§8.4 步骤 4：全部 edit 先算替换区间，区间重叠后者拒绝）；
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

- Rust 单测（`backend/exec_test.rs`/run_loop 测试扩展）：
  - 并发上限信号量（8 只读调用峰值并发 ≤4）；
  - FileWrite 全局串行（同批两 write 时序先后、跨会话互斥）；
  - 同批写冲突拒绝（同文件 write+edit、edit 区间重叠）；
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
- AST fence（22 号票）与子代理文件认领 E_FILE_CLAIMED（10 号票，其父级冲突检测"复用 §5.4"届时接本票冲突检测器）。
- MCP 工具的并发语义（08 号票落地时按本票调度器接入）。
- 单工具内部的并发（如 batch_read 并行读多文件属工具实现细节，不强制）。

## 验收

- [ ] 同批混合读写批次：读并行、写串行、冲突写被拒并回喂模型，批次语义正确。
- [ ] 并发度配置生效（默认 4，改配置后峰值并发随之变化）。
- [ ] 取消收齐：并发批中 cancel，runEnded(cancelled) 仍为收齐后最后一帧，既有 S5 e2e 全绿。
- [ ] 审批挂起恢复零重复执行（14 号票批级预判语义在并发下不回归）。
- [ ] 结果按源序回喂有测试钉住。
- [ ] §3.2（并发度字段）、§5.4（ask 互斥语义、执行⑤修订）、§8.4（步骤 4/5 落地注记）回写完成。
