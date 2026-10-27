# 调研：TS 契约生成方案（C-2 前置）

Type: research
Status: resolved

## Question

Rust↔TS IPC 契约类型如何防漂移（技术设计文档附录 C-2 的前置调研，服务于 M1 的 72+ 命令 / 37 帧 / 31 事件契约面）。

对比三条路线在 2026 年的现状：

1. **ts-rs**——derive 导出 TS 类型；
2. **specta**（含 tauri-specta 生态）——类型导出 + Tauri 命令绑定生成；
3. **手写 TS 类型 + manifest/契约测试校验**（技术设计文档 §12.3 现方案：M1 手写 + manifest 校验，漂移痛点明显再引 ts-rs）。

评估维度：维护活跃度与版本适配（Tauri 2、Rust 1.97/edition 2024）、生成物质量（可读性、union/enum/tag 表达、与 tsconfig 严格模式兼容）、CI 集成成本、对事件/帧枚举（双向扫描式契约测试，NFR-12）的配合度、社区生产使用反馈。

产出：对比表 + 推荐方案 + 理由。结论将作为 C-2 的决策依据，回写技术设计文档 §12.3 并勾销附录 C-2。

## Answer

（调研日期：2026-10-27。版本/日期数据取自 crates.io API、docs.rs 与 GitHub 官方仓库，均为当日实时抓取。）

### 一、三条路线现状对比

| 维度 | ts-rs | specta + tauri-specta | 手写 TS + manifest 契约校验（现方案 §12.3） |
|---|---|---|---|
| 最新版本 | **12.0.1 稳定版**（2026-01-31） | specta **2.0.0-rc.27**、tauri-specta **2.0.0-rc.26**（均 2026-10-21）；稳定版停在 v1.0.2（仅 Tauri 1） | 不适用（自有代码） |
| 维护活跃度 | 健康：2023 年起每年 2–4 个版本，patch 数日内跟进（11.0.0→11.0.1 仅隔 2 天）；下载 2030 万总量 / 900 万近 90 天 | 活跃但爆发式：rc.22（2025-01-12）→ rc.23（2026-03-02）之间有 **14 个月空窗**，2026 年内连发 rc.23–rc.27；2.x 从 2023 年至今 **3 年多未出稳定版**（27+ 个 RC） | 自担：无上游风险，也无上游修复 |
| Rust 1.97 / edition 2024 兼容 | docs.rs 明示 MSRV 1.88.0，覆盖 1.97；v11/v12 发布于 edition 2024 稳定（2025-02）之后，无已知宏卫生问题 | 2026-10 仍在发版，可随新工具链编译；MSRV 未在文档标明（无官方承诺） | 天然兼容 |
| Tauri 2 集成 | 不感知 Tauri：只生成类型，命令名/invoke 封装/事件监听仍手写 | 深度集成：`collect_commands!`/`collect_events!` + `builder.invoke_handler()` 替换 `generate_handler!`，生成 invoke 封装、类型化事件访问器、类型化 `Channel<T>` | 不感知 Tauri |
| 生成物质量 | serde-compat 默认开启，支持 rename/rename-all/tag/content/untagged/flatten/skip 等；生成 `.ts` 声明 + 自动 import，严格模式友好；v12 修了 HashMap 表示与 bigint | TS exporter 自评"Stable/Production-ready"，但 crate 本体 RC，官方要求 **`=` 精确锁版本**（"use = before your Specta version"），RC 间有破坏性变更 | 手写可控，但纯手工镜像无 Rust 侧真值锚点，字段级漂移静默 |
| CI 集成成本 | 低：`#[ts(export)]` 生成导出测试，挂进现有 `cargo test`（§12.3 CI 顺序不变） | 中：需 Rust 侧改 invoke_handler 架构 + 全契约类型改 derive `specta::Type` + 生成物快照进 CI；debug 构建导出 bindings.ts | 低：已设计好（manifest artifact → vitest 消费） |
| 生产反馈 | 下载量大（ts-rs 全家桶近期月下载约 900 万），Aleph-Alpha 自用+维护；单主体维护者（NyxCode）是 bus factor 弱点 | 社区模板/2025、2026 Rust GUI 调研广泛提及；但 **tauri-plugin-midi 因运行时依赖负担移除了 tauri-specta、只留构建期生成**（issue #5）；官方 tauri v2 文档未提及/背书它 | CodeWave 已验证的既有模式（NFR-12 继承来源） |

### 二、对 NFR-12（事件/帧枚举双向扫描契约测试）的配合度

- **手写+manifest**：就是 NFR-12 的本体实现。manifest 记录 `commands/frames/events` **键面**，vitest 双向扫描 client.ts invoke 名与 events.ts 监听键。弱点：**payload 字段级漂移完全无防护**（manifest 只记类型名，不比对字段）。
- **ts-rs**：与 NFR-12 是**互补而非替代**。帧/事件 payload、命令 args/ret 类型从 Rust 真值生成后，字段漂移在 TS 编译期报错；但命令名字符串、事件键仍是手写——**键面双向扫描测试必须保留**（可从 manifest 中删去 args_type/ret_type 名单，负担变轻）。工具清单镜像（defineToolkit ↔ manifest.tools，D11）两者都管不了，manifest 测试在任何路线下都要活着。
- **tauri-specta**：理论上压缩最多——生成 invoke 封装 + 类型化事件访问器后，键面漂移升级为编译错误，NFR-12 运行时扫描可缩减为"禁止裸 invoke/listen 绕过 bindings"的 lint 级检查 + 工具清单 manifest。但注意：(1) 官方自己都在 issue #197 补"生成物快照测试"，说明生成物 ≠ 免校验；(2) Channel 类型化限制为**顶层命令参数/返回值**（"A channel cannot be nested inside a struct, enum, or event payload"）——CraftWave 的 `Channel<ChatFrame>` 顶层用法合规，但 D16 `attach_run(sinceSeq)` 若未来把 channel 包进复合负载就会撞墙。**结论：自动生成路线下契约测试不会被消灭，只是职责从"键+类型"收窄为"键面 + 工具清单 + 生成物快照"。**

### 三、推荐方案（C-2 决策建议）

**分层组合：键面维持手写 + §12.3 manifest/NFR-12 双向扫描不动；payload 类型面引入 ts-rs 12.x 自动生成（生成物进 git + cargo test 导出测试）。tauri-specta 暂不采纳，列为 M2 复评项。**

理由：

1. **ts-rs 是当前唯一"稳定版 + 活跃维护 + 零架构侵入"的选项**（12.0.1，MSRV 1.88 覆盖 1.97；引入只需 derive + 一条导出测试，不动 invoke_handler、不动 D14/D16）。72+ 命令 / 37 帧 / 31 事件的 payload 字段面足够大，"等漂移痛了再引"的成本不如现在一次 derive。
2. **tauri-specta 收益最大但风险与其不匹配**：3 年 RC、要求 `=` 锁死两个 crate、14 个月空窗前科、Tauri 官方零背书、Rust 侧迁移 + 全量换 derive；MVP 阶段押注它会绑架 IPC 层架构。其 RC 间破坏性变更的升级成本正好砸在 M1 的关键路径上。若 M2 后它出 2.0 稳定版可复评。
3. **纯手写路线守不住字段级漂移**：manifest 只锚键面，字段错位（改名/漏字段/可选性变化）静默通过 CI，直到运行时才炸——这正是 C-2 担心的事。ts-rs 把这段面挪进编译期，把 NFR-12 测试瘦身为纯键面门禁，二者拼起来才是完整覆盖。

对 §12.3 的修订建议（回写用一句话版）：M1 即引入 ts-rs 生成 payload 类型；manifest 删除 args_type/ret_type 名单、保留 commands/frames/events 键面 + tools 清单；CI 顺序不变（cargo test 同时产出 manifest 与 TS 绑定）。

### 四、信源（均为 2026-10-27 抓取）

- ts-rs crates.io（版本/下载量）：https://crates.io/crates/ts-rs
- ts-rs docs.rs（MSRV 1.88、serde-compat 属性清单、导出方式）：https://docs.rs/ts-rs/latest/ts_rs/
- ts-rs GitHub Releases（v11 2025-06、v12 2026-01-31 变更）：https://github.com/Aleph-Alpha/ts-rs/releases
- specta crates.io（2.0.0-rc.27）：https://crates.io/crates/specta
- specta 版本历史（rc.22→rc.23 十四个月空窗）：https://crates.io/api/v1/crates/specta/versions
- tauri-specta crates.io（2.0.0-rc.26，稳定版停在 1.0.2）：https://crates.io/crates/tauri-specta
- tauri-specta rc.26 docs.rs（builder API、Channel 顶层限制、beta 告警与 `=` 锁版本要求）：https://docs.rs/tauri-specta/2.0.0-rc.26/tauri_specta/
- tauri-specta GitHub（Tauri v2 兼容表、事件类型化）：https://github.com/specta-rs/tauri-specta
- tauri-specta issue #220 / PR #258（Channel 支持文档化）：https://github.com/specta-rs/tauri-specta/issues/220
- tauri-specta issue #197（生成物单元测试，2026-01 仍开放）：https://github.com/specta-rs/tauri-specta/issues/197
- specta README（TS exporter "Stable"、其余 exporter pre-1.0、生态 rspc/TauRPC）：https://github.com/specta-rs/specta/blob/main/README.md
- tauri-plugin-midi issue #5（生产项目移除 tauri-specta 运行时依赖）：https://github.com/tauri-apps/tauri-plugin-midi/issues/5
- Tauri v2 官方文档 Calling Rust from the Frontend（未提及 tauri-specta）：https://v2.tauri.app/develop/calling-rust
- 类型共享方案深度对比（二手，含 ts-rs 与 Tauri 实践）：https://cetra3.github.io/blog/sharing-types-with-the-frontend
- 2025/2026 Rust GUI 生态调研（社区对 tauri-specta 的使用反馈）：https://www.boringcactus.com/2025/04/13/2025-survey-of-rust-gui-libraries.html 、 https://blog.wybxc.cc/blog/rust-gui-survey-2026
