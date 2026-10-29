# 调研：tauri-specta M2 复评（C-2 收口）

Type: research
Status: resolved

## Question

C-2 定案（2026-10-27）：键面手写 + manifest 双向扫描保留，payload 类型面引入 ts-rs 12，tauri-specta 列 M2 复评。M2 契约面将大幅扩张（命令 31→72+、帧 9→15、事件 18→31 键），本票在 02 号票动工前复评 2026 年现状：

1. tauri-specta 当前版本与 tauri 2.12 的兼容性与成熟度（命令/事件覆盖面、错误处理、生成物形态、与 ts-rs 的关系）；
2. 它能否替代或补充现有 `export_ipc_manifest` + 前端 contract.test 双向扫描体系，整合成本多大；
3. ts-rs 12 与 specta 并存/互斥的工程现实。

结论须回答：**维持"ts-rs 生成 payload + 手写 manifest"不变，还是引入/替换为 tauri-specta**，给出建议与证据（版本号、特性、已知 issue）。结论回写附录 C-2 并勾销。

## Answer

**结论：维持"ts-rs 12 生成 payload 类型 + 手写键面 manifest 双向扫描"不变，M2 不引入 tauri-specta。**（调研日 2026-10-29）

### 三个调研点的回答

**1. 版本与兼容性：活跃维护，但 2.0 仍是 RC，依赖链锁死在 specta RC 上。**

- crates.io 最新 `2.0.0-rc.26`（2026-10-21 发布），仓库主分支已到 rc.27，最近一次推送 2026-10-28（前一天），仅 2 个真实 open issue（#170 `tauri::ipc::Request/Response` 不支持、#88 starter 模板），813 stars——维护活跃、积压极小，不是弃坑项目。
- 但 2.0 线自 rc.20（2024-09-18）走到 rc.26 历时约 25 个月仍未 stable；stable 线停在 `1.0.2`（2023-05，仅支持 Tauri v1）。
- 对 Tauri 2.12 兼容：workspace 约束 `tauri = "2"`（semver 兼容 2.12.5），且强制开启 tauri 内置 `specta` feature。rc.24（2026-03-30）宣称完成 "Specta 2.0.0 vision"，rc.25（2026-05-08）加入语义类型（`Date`/`Uint8Array`/`URL` 等）支持。
- 关键风险：tauri-specta 对 specta 是**精确锁版**（`specta = "=2.0.0-rc.27"`）——引入即让整条 IPC 类型链 ride 在一个未 stable 的 RC 依赖系上，生产项目的升级/安全修复路径不受自己控制。

**2. 生成物形态与现有体系的关系：是"替代"而非"互补"，且 M2 此刻迁移破坏面最大。**

- 生成物是一份完整 `bindings.ts`：类型化 `commands.*` 包装（内嵌 invoke 命令名与全参数/返回类型）、类型化 `events.*`（listen/once/emit/emitTo）、内联 payload 与错误类型（`ErrorHandlingMode` 产出 `{ status: "ok" | "error" }` 判别联合）、支持 `Channel<T>` 流式参数。即它同时生成**键面与类型面**，天然使 drift 结构性不可能——这正是它相对手写体系的核心优势。
- 但采纳即意味着：① 全部 72+ 命令签名、15 帧、31 事件 payload 改挂 `#[derive(specta::Type)]`；② 前端调用层从裸 `invoke("name", args)` 重写为 `commands.name(args)`（`ui/src/ipc` 调用面整体重写）；③ 错误信封改为 tauri-specta 的 status ok/error 约定；④ `ipc_commands!` 宏 + `export_ipc_manifest` + `contract.test.ts` 双向扫描整套废弃或仅存部分（流式帧协议运行时——start_chat → Channel 帧 → 64ms snapshot yield——是自定义协议，tauri-specta 只类型化 `Channel<T>` 参数面，运行时仍需自维护）。
- M2 契约面刚扩张 2 倍多（31→72+ 命令），此刻迁移恰好在最大破坏面上动刀；而现有体系已在 CI 验证且键面漂移已被双向扫描结构性拦截。

**3. ts-rs 12 与 specta 并存：技术上可共存，工程上是双真源。**

- 两者是独立项目、独立类型系统：ts-rs 走 `#[derive(TS)]` 编译期导出 `.ts`；specta 走 `Type` trait + 运行时 `DataType` 图 + `specta-typescript` 导出器（TS 导出已 stable）。specta 不依赖 ts-rs。
- 同一 Rust 类型可同时 `#[derive(TS, Type)]`（trait 名不同，无宏冲突），但这意味着每个 payload 类型挂两套 derive、TS 输出有两个潜在真源——与 C-2 "ts-rs 单一生成面" 的定案直接冲突：要么双 derive 双维护，要么弃 ts-rs 改 specta::Type 推翻 02 号票方案。

**4. 社区实践（2025-2026）：两强格局，tauri-specta 是"端到端绑定"默认解，ts-rs 是"纯类型共享"默认解；个别项目双栈并用（命令/事件走 specta、共享模型走 ts-rs），但需付双 derive 成本。社区对 tauri-specta 的共性保留意见正是"v2 长期停留在 RC"。**

### 建议

- M2：维持 C-2 定案，02 号票按 ts-rs 12 落地 payload 生成；手写键面 + manifest 双向扫描继续承担命令/事件/帧的键面回归。
- 复评触发条件（写入附录 C-2 的复评锚点）：tauri-specta 2.0 发布 stable（脱离 specta RC 锁版）之后，或 M3+ 命令面再次翻倍、手写 manifest 维护成本显著上升时再评"整体替换"路径。

### 证据链接

- tauri-specta crates.io（rc.26，2026-10-21；stable 停 1.0.2）：https://crates.io/crates/tauri-specta
- tauri-specta 仓库（pushed 2026-10-28、open issues、主分支 rc.27、`specta = "=2.0.0-rc.27"` 锁版、`tauri = "2"`）：https://github.com/specta-rs/tauri-specta
- 生成物形态（examples/app/src/bindings.ts）：https://github.com/specta-rs/tauri-specta/blob/main/examples/app/src/bindings.ts
- release notes（rc.24 "Phasing Forward" 2026-03-30、rc.25 语义类型 2026-05-08）：https://github.com/specta-rs/tauri-specta/releases
- specta crates.io（2.0.0-rc.27，2026-10-21；stable 停 1.0.5/2023-07）与 README（TS 导出 stable，多语言导出器）：https://crates.io/crates/specta 、https://github.com/specta-rs/specta
- ts-rs crates.io（12.0.1 stable，2026-01-31，累计 2066 万下载）：https://crates.io/crates/ts-rs
- 社区选型讨论：https://github.com/orgs/tauri-apps/discussions/11093 、https://www.reddit.com/r/rust/comments/1rc6rlf/typescript_rust_feels_like_a_cheat_code_stack/

### 对 02 号票（02-impl-ts-contract-generation.md）的直接影响

- 无阻塞、无方案变更：ts-rs 12.0.1 stable 可直接落地；payload 类型**不需要**加 `specta::Type` 双 derive，避免双真源。
- 手写 manifest 的命令/事件/帧键面继续作为契约锚点；ts-rs 生成物只覆盖 payload 类型面，两者边界与 C-2 一致。
- 一条实现提示：`tauri` 本身的 `specta` feature 不必开启（那只是 tauri 内建类型实现 specta::Type，与 ts-rs 无关）；ipc manifest 与 ts-rs 导出测试可同在 `cargo test` 内落盘，延续 §12.3 的 CI 顺序约定。
- 附录 C-2 的"复评完成"勾销与复评触发条件回写：由主线在 map.md/spec.md 侧同步（本票按约束只改本文件）。
