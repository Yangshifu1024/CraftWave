# 实现：ts-rs 12 契约生成落地与错误码补行

Type: task
Status: open

Blocked by: 01

## 要做什么

把 C-2 定案的 payload 类型生成真正落地（M1 12 号票明确挂账、01 号票复评维持定案）：Rust 契约类型经 ts-rs 12 自动生成 TS 声明并进 git，前端手写镜像逐步改为消费生成物——M2 命令面 31→72+、帧 9→15、事件 18→31 键的扩张不再每票双侧手抄。本票同时清偿 §3.6 错误码表的 M1 遗留补行（E_IO）。纯契约面工程，不改任何运行行为。

## 具体做什么

### 后端（src-tauri/）

- 引入 ts-rs 12.0.1（01 号票定案版本，stable、不加 specta 双 derive）为 dev-dependency + derive feature。
- 对 §3.2/§3.4 契约结构体/枚举 derive `TS` 并 `#[ts(export)]`，按现有镜像面逐域覆盖：
  - `providers/types.rs`：AppConfig/ProviderConfig/ProviderModel（含 Proxy/Approval/Ui/Log/Notification 子配置——§3.2 供应商与应用配置域）；
  - `storage/entries.rs`：WirePart/MessageEntry/MetaEntry/SessionEntry/SessionMeta/SessionTreeData/PlanItem（serde `tag = "kind"` + `rename_all = "camelCase"` 的判别联合形态须由 ts-rs 的 serde 兼容属性保真）；
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

- [ ] `ui/src/ipc/generated/` 生成物进 git，`types.ts` 无手写 payload 类型（仅 re-export），typecheck/test 全绿。
- [ ] 故意改 Rust 侧任一契约字段不同步 TS，contract.test 或 typecheck 变红（漂移面收窄为单源的实证）。
- [ ] manifest 的 args_type/ret_type 与生成物一一对应有测试钉住。
- [ ] §3.6 增 E_IO 行、附录变更记录已回写；E_STORAGE_IO 行核对无出入。
- [ ] 键面双向扫描（COMMANDS/EVENT_KEYS/toolSpecs）机制原样保留且全绿。
