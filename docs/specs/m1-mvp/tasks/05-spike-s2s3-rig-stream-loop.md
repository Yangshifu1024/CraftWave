# S2+S3 rig 流事件保真与手动工具循环（含 C-6 收口）

Type: prototype
Status: resolved
Blocked by: 04

## Question

以「S6 最小骨架与三平台构建验证（含 C-7）」产出的骨架为底，做一次粗糙但端到端的 rig 流式验证（技术设计文档附录 A **S2**+**S3**，**C-6** 收口）：

1. **S2 流事件保真**：rig 流事件（文本 delta / 思考 delta / 工具参数增量，含半截 JSON）经 Rust 侧转换为 `ipc::Channel` 帧（64ms 节流合并）到前端 assistant-ui 渲染——验证无丢失、无乱序、打字机流畅、端到端延迟满足 NFR-02（<100ms）。
2. **S3 手动工具循环**：rig 0.46 能否按「单 turn 返回 + 外部执行工具 + 结果回喂」驱动（审批挂起需要外部循环）；不可行则验证 AgentHook::Dispatch 拦截路线并说明 backend 需如何重设计。
3. **C-6 双版本**：0.45 稳定版与 0.46-rc.1 各跑一轮上述两项，结合「调研：rig 0.45 稳定版 vs 0.46-rc.1 锁版底料（C-6 前置）」的结论给出锁版建议。

完成标准：三个问题各有明确结论；帧协议字段如需调整，给出修订清单。结论回写技术设计文档 §4/§5.2，勾销附录 A-S2/S3 与附录 C-6。

## Answer

（2026-10-27 完成。分支 `spike/s2s3-rig-stream-loop`：`bddcf11` spike 主体 + `eaa2bd8` 工具链抬升，已并入 main。本机无 API key，以本地 OpenAI 兼容 mock SSE 服务（axum）驱动 rig 真 Chat wire 解码，全程离线确定性。）

### 1. S2 流事件保真：可行（测试实证 + 实测数据）

链路 mock SSE → rig 流事件 → `ipc::Channel` 帧桥接（64ms 节流）→ 前端 FrameBuffer。脚本化序列覆盖思考 delta / 文本 delta / 工具参数三段增量（第二段结尾半截 JSON）。

- 无丢失无乱序：帧拼接=发送序列、seq 严格递增、toolResult 晚于同 callId 全部 toolArgs、思考先于文本（`tests/spike_s2_stream.rs` 3 例）。
- 64ms 节流：帧间隔 ≥ 窗口、合并无损（6 例单测 + 集成实测帧间隔 `[95.3, 193.1, 64.5]ms`，首帧立即、其后 ≥64ms）。
- 实测延迟（进程内单调钟）：TTFT 0.38ms（0.45.0）/ 0.32ms（0.46-rc.1）；单帧平均 0.36ms / 最大 0.45ms；前端帧→快照 <0.005ms。**NFR-02 <100ms 达标，余量 >35ms**——前提是节流语义为「首增量立即出帧 + 最小帧间隔 64ms」（朴素 tick 会让首帧平白多等一个窗口相位，已钉死进 §3.4）。

### 2. S3 手动工具循环：可行（无需 AgentHook::Dispatch 重设计）

rig 0.45 的 **`rig::run::AgentRun`（sans-IO 状态机）** 正是 §4.3/§5.2 需要的形态（`tests/spike_s3_tool_loop.rs` 2 例）：

- `next_step()` → `CallModel` → 单 turn 流式调用（`prepare_request` → `Model::stream` → `StreamedTurnAssembler::ingest` 边收边出帧）→ `CallTools{calls}` → 外部执行 → `answer_all` 回喂 → 下一 `CallModel` → `Done`。断言：恰好 2 次模型调用、mock 第 2 轮请求含 `role=tool` 消息、流式半截片段拼合后参数原样传给执行器、两轮 usage 记账无丢失。
- **审批挂起 = CallTools 步骤上把整只 run `serde_json` 序列化挂起、反序列化恢复继续**——HITL 审批挂起（含跨重启恢复）形态直接成立。
- 注意点：装配器对非法工具调用有 `InvalidToolCall` 分支需宿主决议（spike 未触发，生产 §5.4 处理）。

### 3. C-6 双版本：锁 0.45.0（维持调研结论，行为面已实证）

同一套宿主代码在 rig 0.45.0 与 0.46.0-rc.1 下**零改动**：编译、clippy 零警告、11 测全绿，延迟同量级。锁 0.45.0；换边条件 = 0.46.0 stable 发布且 #2907/#2921/#2910 修复落地，届时仅改 Cargo.toml 一行。注意 cargo pre-release 匹配：`0.46.0-rc.1` 需求串不会自动升到 0.46.0 stable。

### 4. 帧协议修订清单（已回写 §3.4）

1. 帧基座新增可选字段 `ts?: number`（Rust 出口 epoch ms），供端到端延迟埋点与 §5.6 对账。
2. 64ms 节流语义钉死为「首增量立即出帧 + 相邻 flush 间隔 ≥64ms」。
3. `toolStarted.callId` 与 `toolArgs.callId` 同源 = 模型 call id（spike 桥接曾用 `call_part_{n}` 派生 id，与装配器产出的模型 call id 不同源，生产须统一，否则镜像工具卡无法与 toolResult 配对）。
4. 无需新增帧型——现有 9 帧 P0 集覆盖 S2/S3 全部需求。

### 5. 附带发现：工具链 1.97.0 → 1.97.1

rig 0.45.0 的 MSRV 是 **1.97.1**（cargo 直接拒绝 1.97.0），`rust-toolchain.toml` 已抬升并单独提交；§4.4/§12.4 已同步。

**文档回写**：§3.4（ts 字段/节流语义/callId 同源）、§4.3（backend 载体改 AgentRun）、§4.4（锁 0.45.0 + MSRV）、§5.2（RunLoop=AgentRun 步进循环）、§12.4（1.97.1）、附录 A-S2/S3 勾销、附录 C-6 补实证。

**验证**（合并 main 后终验）：cargo build / clippy -D warnings / cargo test（16 测）/ fmt --check 全绿；ui typecheck / test（27 测）/ lint 全绿。
