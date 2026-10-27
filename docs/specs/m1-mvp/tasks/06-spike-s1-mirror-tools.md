# S1 assistant-ui 镜像工具桥接

Type: prototype
Status: resolved
Blocked by: 04

## Question

验证 assistant-ui 镜像工具桥接方案可行性（技术设计文档 §10.3，附录 A **S1**）：

- 前端 `defineToolkit` 声明**镜像工具**（后端工具的渲染副本，不在前端执行）；
- Rust 侧 Channel 帧（ToolStart / ToolProgress / ToolResult）驱动镜像工具的 execute resolve；
- 工具卡片状态机（参数准备中 → 运行中 → 完成/错误）正常流转，不闪烁不重建；
- 半截 JSON 参数增量以「参数准备中」态展示。

**判定标准**：桥接可行、状态机正常。**否则**退 D11 路线（自定义 data part 驱动工具卡片），说明差异与代价。

完成标准：二选一有明确结论与依据。结论回写技术设计文档 §10.3，勾销附录 A-S1。

## Answer

（2026-10-27 完成。分支 `spike/s1-mirror-tools`：`45fe417` 前端原型 + `cc9fb2e` Rust mock 帧源，已并入 main。）

### 结论：桥接可行，不退 D11；唯一修正是「execute 桥接」不存在，改为 adapter 直写 result

判定依据（全部来自真实代码与真实渲染管线测试，`ui/src/chat/mirror/mirror.test.tsx` 10 例）：

- **镜像工具声明成立**：`defineToolkit` 真实存在于 0.15.31（从 `@assistant-ui/core/react` re-export），条目形状 `{ type, description, parameters, render }`，`type:"frontend"` 可通过类型检查。卡片组件直接作条目 `render` 字段——`makeAssistantToolUI`/`useAssistantToolUI` 已 @deprecated，不采用。
- **状态机原生成立**：adapter 以全量累计快照 yield tool-call part，`result` 从无到有驱动 part 状态 running→complete（`isError`→错误）。卡片实测流转：**参数准备中（半截 JSON 可见）→ 运行中 → 完成/错误**，逐态断言通过。
- **不闪烁不重建成立**：每个 `toolCallId` 的卡片 mount 恰好 1 次、全程零 unmount（挂载埋点 + 断言）。
- **半截 JSON 展示成立**：增量累积进 `argsText` 原文展示，`parsePartialJsonObject`（assistant-stream/utils 官方导出）产出含元数据的 partial args，`useToolArgsStatus().argsStatus` 区分 streaming/complete。
- **修正**：设计原「`execute()` 注册 pending Promise、ToolResult 帧到达后 resolve」在 local runtime 下**永远不会被调用**（ToolInvocationTracker 仅接 external-store runtime，与 §10.2 ChatModelAdapter 路线互斥）——结果注入改为 **adapter 把 FrameBuffer 累积的 toolResult 帧直接写进 part 的 `result`/`isError`**。D11 决策（复用原生状态机）不变、依据更扎实；data part 路线需手写状态机/自管 key/闪烁防护，成本更高无收益，不退。

### 设计假设 vs 0.15.31 实际差异清单（已回写 §10.3）

1. execute 桥接：local runtime 无 execute 管线 → adapter 直写 result。
2. `makeAssistantToolUI`/`useAssistantToolUI` 已废弃 → 卡片放 toolkit 条目 `render`。
3. `Tools` 是 @assistant-ui/tap resource 工厂，不能当 JSX 元素（TS2786）→ 经 `AssistantRuntimeProvider config={AuiConfig({ tools: Tools({ toolkit }) })}` 注入。
4. 「参数准备中」判定 = `useToolArgsStatus()`（status 为字面量联合）；args 须由 `parsePartialJsonObject` 产出（含 partial-parse 元数据）。
5. partial JSON 解析器在传递依赖 `assistant-stream` 里，pnpm 隔离布局下 ui 须**显式声明依赖**（已加 `"assistant-stream": "^0.3.54"`）。
6. `ToolCallMessagePart` 尚有 `isPreliminary`/`artifact`/`interrupt`/`approval`/`addResult` 能力——M1 可忽略，M2 子代理/审批会用到。

**文档回写**：§10.3（声明/挂载/结果注入/参数流式四段重写）、§13-D11（修正与补强）、附录 A-S1 勾销。

关键文件：`ui/src/chat/mirror/`（frames / mockSource / toolkit / MirrorCards / mirrorRuntime / MirrorDemo `?mirror=1` 原型页 / mirror.test.tsx）；`src-tauri/src/mock_chat.rs` + `mock_chat_test.rs`（dev-only mock 帧源，release 不编译）。

**验证**（合并 main 后终验）：cargo build / clippy -D warnings / cargo test / fmt --check 全绿；ui typecheck / test（27 测）/ lint 全绿。GUI 视觉走查（`pnpm dev` 后 `?mirror=1` 看卡片动画）留用户首次 dev 顺带确认，测试已覆盖数据与状态机语义。
