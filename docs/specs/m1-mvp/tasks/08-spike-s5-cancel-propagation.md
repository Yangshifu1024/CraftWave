# S5 取消传播全链路

Type: prototype
Status: resolved
Blocked by: 05

## Question

验证取消传播全链（技术设计文档附录 A **S5**，基于「S2+S3 rig 流事件保真与手动工具循环」确定的 backend 形态）：

- 前端 `abortSignal` → `invoke("cancel_run")` → Rust 侧 `CancellationToken` → 模型流停止 + 进行中命令进程组收割（macOS 进程组 kill；Windows Job Object 方案至少纸面确认）→ `runEnded(cancelled)` 到前端，**全链 <1s**；
- 取消时收齐全部工具结果并补 `cancelled` 标记的语义验证（已写文件不回滚但明确标注）。

完成标准：全链时延实测 <1s；标记语义有结论。结论回写技术设计文档 §5.3，勾销附录 A-S5。

## Answer

（2026-10-27 完成。分支 `spike/s5-cancel-propagation`。本机无 API key，沿用 05 号票路线：本地 OpenAI 兼容 mock SSE（axum）驱动 rig 真 wire；另起真实进程树（`sh -c 'sleep 900 & …'`）验证进程组收割。全程离线、确定性、可重复。代码：`src-tauri/src/spike/cancel.rs`（RunLoop 取消版）+ `tests/spike_s5_cancel.rs`（集成 2 例）+ `src/spike/cancel_test.rs`（帧协议单测 4 例）+ `ui/src/chat/cancel-bridge.ts`（前端挂接原型）+ `ui/src/chat/cancel-bridge.test.ts`（5 例）。）

### 1. 全链时延：可行，余量两个数量级（macOS 实测）

口径 = `token.cancel()`（模拟 `cancel_run` 置位，§3.2 命令 19「置位即返回」）→ sink 收到 `runEnded(cancelled)` 帧的时刻，进程内单调钟，debug 构建（更保守），连续多轮数值稳定：

| 场景 | cancel → runEnded(cancelled) |
|---|---|
| 模型流中取消（长流 ~4.5s 处 ~100ms 时取消） | **0.03–0.06ms**（select 胜出即 drop 流） |
| 命令工具执行中取消（含进程组整树收割） | **5.96–7.83ms**（SIGTERM 后 sleep/sh 即刻退出，5ms 轮询首查即见退出；200ms 宽限未用满） |

两者均远低于 <1s 预算。收割路径的时延构成 = SIGTERM 广播 + 子进程退出 + try_wait 轮询粒度（5ms），宽限期是兜底上界而非常态开销。前端 `invoke` → Rust 置位的 IPC 开销在真 app 里属毫秒级（S2 埋点已证 Channel 单程亚毫秒），不影响数量级结论；runEnded 后 300ms 内无任何新帧（流确实停了）。

### 2. 形态钉死（在 S2+S3 形态上的增量）

- **模型流停止**：`TurnDriver` 流循环改为 `tokio::select!(biased; token.cancelled(), stream.next())`——token 触发即 drop HTTP 流（`StreamExit::Cancelled`），不调 `stream.finish()`（usage 记账放弃，取消 run 不计费语义可接受）。`cancelled()` cancel-safe，可安全用于循环内 select。
- **命令工具收割**（Unix）：spawn 时 `process_group(0)`（tokio `Command::process_group`，子进程为组长）→ 取消分支 `kill(-pgid, SIGTERM)` → 200ms 宽限轮询 `try_wait` → `kill(-pgid, SIGKILL)` → `wait()` 收尸。**孙进程实测随组消亡**（`sh -c 'sleep 900 & echo CHILD_PID=$!; wait'` 造真实进程树，收割后 `kill(pid,0)` 轮询确认孙 sleep 死亡）。
- **RunLoop 取消版**（`drive_cancellable_run`）：CallModel 取消 → 无工具进行中，直接发 `runEnded(cancelled)`；CallTools 中任一工具报 `Cancelled` → 批内剩余工具不再执行、一律补标记 → **收齐后才发终态帧**。

### 3. 标记语义（钉死）

取消是 run 终态，不是错误扩散；工具结果逐个交代：

1. **已完成的工具**：结果原样保留（`toolResult.ok=true`、无 error 面）——已写文件不回滚，runEnded 的 `cancelled` 是唯一的全局取消标注；
2. **被收割的命令**：`toolResult.ok=false` + `error.code="E_CANCELLED"`（§3.4 现有 `error?` 字段承载，**帧协议零修订**），uiSummary 注明「进程组已收割」；
3. **批内未执行的工具**：同样补 `E_CANCELLED`（message 注明未执行），不留悬空调用；
4. **顺序不变量**：`runEnded(cancelled)` 必在全部 toolResult 之后、必为最后一帧（§5.3 不变量「取消中工具收齐」的帧面表达；前端镜像工具卡按 callId 配对消费，vitest 5 例验证）。

### 4. Windows Job Object：方案落地要点（代码级可编译性已验）

- **方案**：`CreateJobObjectW` 建 Job → `SetInformationJobObject(JobObjectExtendedLimitInformation)` 设 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`（RunHandle drop 时句柄关闭即整树终止，孤儿进程兜底）→ 子进程 `AssignProcessToJobObject`（此后派生的后代自动入 Job，除非显式 `CREATE_BREAKAWAY_FROM_JOB`）→ 取消/超时 `TerminateJobObject`（kernel 层同步整树终止，对应 Unix `kill(-pgid, SIGKILL)`）→ `child.wait()` 收尸。依据：Microsoft Learn《Job Objects》/ `jobapi2`（CreateJobObjectW、SetInformationJobObject、TerminateJobObject）API 文档；`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 见 `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`。
- **落地要点**：① windows-sys 的 `CreateJobObjectW/SetInformationJobObject/TerminateJobObject` 需要 `Win32_Security` feature（本票已踩：仅开 JobObjects feature 找不到符号）；② tokio `Child::raw_handle()` 返回 `RawHandle`（已是 `HANDLE` 指针，勿再解引用）；③ `spawn→assign` 窗口期存在后代逃逸竞态（shell 启动即 fork），生产应以 `CREATE_SUSPENDED`（`creation_flags(0x4)`）起进程、assign 后 resume 主线程——std 不暴露子进程主线程句柄，届时需 windows-sys 全 FFI 的 `CreateProcessW`（约 40 行）或现成 crate（如 win32job）；spike 骨架用 std spawn + 立即 assign 表达语义。
- **验证等级**：本机以最小 crate 复刻同源代码 `cargo check --target x86_64-pc-windows-msvc` **通过**（类型/FFI/借用级实证；顺带修正了 3 处真实编译错误）。全树交叉 check 在 macOS 不可行（依赖树 aws-lc-sys 需 Windows SDK 编 C）；**运行时行为**（收割正确性）由仓库三平台 CI 在 Windows runner 上覆盖（04 号票骨架已建）。

### 5. 前端挂接（S5 增量，完整语义进 §10.2 伪代码）

`bindCancelToRun(signal, sessionId, invoke)`：abort → `void invoke("cancel_run", { sessionId }).catch(() => {})`——fire-and-forget（不 await，§10.2 口径），失败不 UI 报错（终态以 runEnded 帧为权威）；signal 已 aborted 时立即触发；adapter finally 里解绑。已 abort 的窗口（异步 run 启动间隙）覆盖。

### 6. 遗留项（均不阻塞实现票）

1. **Windows 运行时行为**未实测（本机无 Windows）——CI 绿 + 方案为官方 API 标准用法，风险低；实现票落地时在 Windows runner 上跑一次收割冒烟（可选）。
2. **取消审批挂起中的 run**（AwaitingApproval → Cancelled 转移）：审批挂起 = run 序列化挂起（S3 已钉形态），此时无进程可收割、无流可停，cancel 语义 = 丢弃挂起 run + runEnded(cancelled)，属 RunLoop 状态面收尾，实现票覆盖；本票未展开。
3. **取消时的 usage 半程记账**：流取消后 `stream.finish()` 未调，该轮 usage 丢失——M1 用量面板为展示性质，可接受；如需精确再议。
4. **`.cargo/config.toml [env]` 的 MACOSX_DEPLOYMENT_TARGET 与 release 构建**：本机 release profile 构建存在无关的 proc-macro crate 环境问题（debug 全绿），不影响本票结论（debug 数据更保守）。

**文档回写**：§5.3（取消传播句扩写为三段式：流停止 / 进程组收割 / 收齐补标记）、附录 A-S5 勾销。

**验证**：cargo build / clippy -D warnings / cargo test（22 测：15 单测 + 3 S2 + 2 S3 + 2 S5）/ fmt --check 全绿；ui typecheck / test（32 测）/ biome check 全绿。
