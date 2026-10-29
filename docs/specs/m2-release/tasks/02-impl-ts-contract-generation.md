# 实现：ts-rs 12 契约生成落地与错误码补行

Type: task
Status: resolved

Blocked by: 01

## 要做什么

把 C-2 定案的 payload 类型生成真正落地（M1 12 号票明确挂账、01 号票复评维持定案）：Rust 契约类型经 ts-rs 12 自动生成 TS 声明并进 git，前端手写镜像逐步改为消费生成物——M2 命令面 31→72+、帧 9→15、事件 18→31 键的扩张不再每票双侧手抄。本票同时清偿 §3.6 错误码表的 M1 遗留补行（E_IO）。纯契约面工程，不改任何运行行为。

## 具体做什么

### 后端（src-tauri/）

- 引入 ts-rs 12.0.1（01 号票定案版本，stable、不加 specta 双 derive）为 dev-dependency + derive feature。
- 对 §3.2/§3.4 契约结构体/枚举 derive `TS` 并 `#[ts(export)]`，按现有镜像面逐域覆盖：
  - `providers/types.rs`：AppConfig/ProviderConfig/ProviderModel（含 Proxy/Approval/Ui/Log/Notification 子配置——§3.2 供应商与应用配置域）；
  - `storage/entries.rs`：WirePart/MessageEntry/MetaEntry/SessionEntry/SessionTreeData/PlanItem（serde `tag = "kind"` + `rename_all = "camelCase"` 的判别联合形态须由 ts-rs 的 serde 兼容属性保真）；
  - `backend/frames.rs`：ChatFrame 9 变体/UsageTotal/SequencedFrame（§3.4 帧基座）；
  - `backend/ask.rs`：AskPayload/AskAnswer/AskQuestion/AskChoice/AskOption/ApprovalDiff 及子类型（§3.2 询问/审批域）；
  - `host/commands/` 各域 Args/Result 负载（SaveSessionTreeArgs/StartChatArgs/ReadWorkspaceResult 等——manifest 里 args_type/ret_type 点名的全部类型）。
- 生成物落 `ui/src/ipc/generated/`（ts-rs `export_to` 统一指到该目录），目录进 git；`cargo build` 不依赖生成步骤，导出由测试驱动。
- 导出测试挂进 cargo test：断言生成物齐全，且与 `ipc_commands!` 宏（src-tauri/src/lib.rs 单一事实源）点名的 args_type/ret_type 一一对应。
- manifest 精简落点（C-2 原文预留）：`IpcCommandSpec` 结构不动；contract.test 的"类型锚点"从手写镜像改查生成物；命令名字符串/事件键/工具清单的双向扫描依旧保留（C-2 明文）。
- §3.6 补行：
  - 增 `E_IO` 一行（域=通用/存储，语义=OS 层文件与目录操作失败——open_dir（§3.3 #3，03 号票实装）、read_session_log 等命令的 io::Error 归一面）；
  - `E_STORAGE_IO` 已有行且 `util/errors.rs:113` 已实装，核对语义表述无出入即可，不重复加行；
  - 回写附录变更记录。
- 现状锚点（接管对象盘点）：
  - `ui/src/ipc/types.ts` 现约 49 个导出类型/425 行（WirePart/StoredEntry 族、SessionTreeData、AppConfig 族、RunConfig/UsageTotal、StartChatArgs、ChatFrame、AskPayload 族、事件 payload 族）；
  - `ui/src/ipc/contract.test.ts` 460 行四测试面（manifest 扫描/键面卫生/覆盖度/跨语言 fixture）；
  - Rust 侧宏清单 `ipc_commands!` 现 28 个命令行 + dev-only mock_chat_run 的 cfg 面——cfg 命令在 manifest 的呈现维持现状，不做特判。
- 已知取舍：
  - 生成物为编译期产物，Rust 类型改动须重跑 cargo test 才更新生成物——CI 顺序（§12.3：cargo test → vitest）天然保证先更新后校验；
  - ts-rs 对 serde `tag/rename_all/flatten` 的兼容以 12.x 文档为准；个别复杂枚举（WirePart 六变体、AskPayload 四形态）若生成形态与手写镜像有出入，以现有 fixture roundtrip（session-tree-fixture.jsonl 逐条目比对）为红线调生成配置，绝不手改生成物；
  - `chat/frames.ts` 现有 ChatFrame 本地分发逻辑（FrameBuffer 按判别值分发）只换类型来源、不动分发代码。

### 前端（ui/）

- `ui/src/ipc/types.ts`：手写类型逐域替换为 `export * from "./generated/…"` 再导出（保留文件头注释说明单一来源已切换）；本票一次性接管现有全部镜像面，后续功能票新增命令时只 derive 不手抄。
- `ui/src/chat/frames.ts` 的 ChatFrame 改从生成物 re-export（现 `export type { ChatFrame }` 指向手写定义）。
- 键面不动：`client.ts` 的 COMMANDS/EVENT_KEYS 注册表、`ipc_commands!` 宏、manifest 双向扫描机制原样保留。
- `contract.test.ts` 增量：
  - manifest 每条 args_type/ret_type 在 `ui/src/ipc/generated/` 有同名导出（防生成物缺失）；
  - 跨语言 fixture（session-tree-fixture.jsonl）期望值改引生成物类型（编译期即校验判别形态）。

### 测试

- Rust：`ipc_manifest_test.rs` 扩展——manifest 类型名 ↔ 生成物文件名双向一致；ts-rs 导出冒烟（生成目录非空、关键类型存在）。
- 前端：`contract.test.ts` 既有四测试面全绿即回归门；新增生成物存在性断言。
- 检查链七项全过（cargo fmt/build/clippy/test + pnpm typecheck/test/lint）。

## 边界（不在本票）

- 不含 M2 新增命令/帧/事件的 derive（随各功能票增量：04 带 compact_session/compactDone、05 带 session_run_snapshot/attach_run、06 带 inject_run_message……）。
- 不引入 tauri-specta（01 号票 M2 复评已维持不引入；再评触发 = specta 2.0 stable 或命令面再翻倍）。
- 不改 manifest 结构（{commands,frames,events,tools,toolSpecs}）与 CI 顺序（cargo test → vitest）。

## 验收

- [x] `ui/src/ipc/generated/` 生成物进 git（78 文件），`types.ts` 无手写 payload 类型（仅 re-export），typecheck/test 全绿。
- [x] 故意改 Rust 侧任一契约字段不同步 TS，contract.test 或 typecheck 变红（漂移面收窄为单源的实证）——实测：AskChoice.label 改名 label_x → cargo test 重新生成 → vitest typecheck 立即在 AskPanel/ask-panel.test 红；回滚后全绿。
- [x] manifest 的 args_type/ret_type 与生成物一一对应有测试钉住（Rust ipc_manifest_test 正向 + 前端 contract.test 名义类型扫描）。
- [x] §3.6 增 E_IO 行、附录变更记录已回写（v2.10）；E_STORAGE_IO 行核对无出入（util/errors.rs 常量与 storage_io() 语义一致）。
- [x] 键面双向扫描（COMMANDS/EVENT_KEYS/toolSpecs）机制原样保留且全绿。

## 决议记录（2026-10-30 实施）

1. **导出驱动方式改为"清单宏统一导出"而非每类型 `#[ts(export)]`**：`#[ts(export)]` 会给每个类型生成独立导出测试，与 manifest 一致性测试在同一测试二进制内并行执行存在读写竞态；改为 `ipc_commands!` 宏扩展出 `export_contract_ts()`（宏内对每条 args/ret 直接调 `<T as ts_rs::TS>::export_all`，连带依赖闭包），由 `ipc_manifest_test::export_contract_types_and_match_manifest` 单点驱动。清单全集（含 `()`/bool/Vec<_>/Option<_>/HashMap<_,_>）在 ts-rs 12 均有 TS 实现，非名义类型导出自身时返回 `CannotBeExported` 占位、由测试按名义名单甄别。**「双向一致」的反向半边（生成物 ⊆ 消费面，防孤儿文件）落点在前端 contract.test**（票面「测试」节原写在 Rust 侧）：孤儿文件的清理动作发生在前端再导出列表，放在消费侧红得更准，且 vitest 在 CI 顺序上后于 cargo test，无读写竞态。
2. **派生写法**：`#[cfg_attr(test, derive(ts_rs::TS))]`（非测试构建完全不见派生，ts-rs 保持 dev-dependency，cargo build 不依赖生成步骤）；Option 且 serde skip_serializing_if 的字段加 `#[cfg_attr(test, ts(optional))]` 渲染 `field?: T`（ts-rs 默认 `T | null` 不符合线上"键省略"语义）。与手写镜像的两处有意出入（生成物更贴 Rust wire 实际形状）：`AppConfig.notifications` Rust 侧无 skip 恒序列化 → 生成必填（旧镜像误标可选，前端默认值补齐对齐 Rust `AppConfig::default`）；`schemaVersion` Rust 侧 u32 → 生成 `number`（旧镜像为字面量 `2`）。另有 `StartChatArgs` 生成物不含 `channel` 字段：Rust 命令的 channel 是 tauri `Channel<SequencedFrame>` 平铺形参（非结构体字段），前端 client.ts 已单独传参，旧镜像的 `channel: unknown` 文档锚点职责由 client.ts 注释承接。
3. **整数渲染**：ts-rs 12 默认 i64/u64 → bigint，与镜像 number 冲突；经 `ts_rs::Config::default().with_large_int("number")` 统一修正（导出测试内显式构造，无环境变量依赖）。
4. **补充导出根**：帧面（ChatFrame/SequencedFrame）、询问面（AskPayload）、存储信封（StoredEntry）、错误面（IpcError）、边车面（ToolOutcome）不经命令清单，且 `export_all` 对非名义根不递归依赖——显式补根（含只藏身 Vec<WorkspacePathHit> 的 WorkspacePathHit）。
5. **线帧类型对齐**：手写镜像把 seq/ts 并进 ChatFrame，生成物如实拆为 `SequencedFrame = {seq, ts?} & ChatFrame`（与 Rust `Channel<SequencedFrame>` 同名）——前端边界消费类型统一改为 SequencedFrame（frames.ts/runtime.ts/run-metrics.ts/client.ts 及测试），FrameBuffer 分发逻辑不变。
6. **镜像别名收编**（D14 两侧同名）：ToolOutcomeJson→ToolOutcome（client.ts/ToolCards.tsx 消费点改名）、ThemeName→Theme、WorkspaceReadRange→ReadRange；无消费面的 NotificationPermission 镜像删除（契约 status 为 string，设置页引导组件放宽为 string 按字面量比较）；StoredMessageEntry/StoredMetaEntry/StoredSessionEntry 以 `Extract<StoredEntry, {kind}>` 派生别名承载。ToolOutcome 收编 truncated 字段（原边车超限占位是绕过结构体的手拼 JSON；None 不序列化，线上字节不变）。
7. **残留手写面**（票内派生域清单未含，无 Rust 结构体来源）：§3.5 事件载荷族 5 个（AskOpenedEvent 等，Rust 发射点为 json! 字面量，随对应功能票收编）；MessageStatus 的 incomplete.error 在 assistant-ui（ReadonlyJSONValue）与生成物（JsonValue）间因 readonly 数组结构差异不可直接互赋，在 runtime.ts/tree-persistence.ts 两处边界显式收口（注释注明）。
8. **既有抖动（非本票引入）**：`thread-rendering.test.tsx`「建议填入实际 composer」用例在 main 基线即偶发失败（本分支 stash 验证复现），与本票无关，建议另立小票。
