# 06 engine 解耦预演（4 个 tauri:: 文件的 trait 化改法）

Status: resolved
Type: task

## 目标

对主仓 `src-tauri/src/` 中引用 `tauri::` 的 4 个引擎侧文件（tools/knowledge/mcp/git 各 1 个，阶段 1.3 要解耦的对象）各产出一份 trait 化改法草稿，确认引擎可整体脱离 Tauri。只写草稿，不改主仓。

## 步骤

1. 逐文件定位 `tauri::` 引用的确切用法（类型？事件？状态句柄？），按 4 类给出改法：
   - 抽什么 trait（方法签名）；
   - 谁实现 trait（旧壳继续实现 / 新 UI 直接实现 / 测试 fake）；
   - 注入点（构造函数参数 / 泛型参数 / dyn 对象）；
2. 检查是否牵连 host 层调用方，列出每个调用方的适配量；
3. 顺带盘点 storage 模块里的死代码（旧格式升级/备份保护逻辑，33 号票产物）——标记"搬移时剥离"清单；
4. 若某文件耦合面超出预期（比如 tauri State 深入类型签名），记录并升级讨论。

## 验收

- 4 个文件各有可执行的改法草稿（贴 Comments）；
- 调用方适配量表完整；
- storage 死代码剥离清单成文（阶段 1.3 的输入）。

## Comments

### 预演结论（2026-10-31）

**Answer**：预演完成，结论如下——阶段 1.3 无需任何 trait，工作量降为一天内。

**核心发现：不需要任何 trait。** 代码库已是「`*_core` 函数 + `#[tauri::command]` 薄壳」双层结构（`_core` 明确标注 headless 测试直驱），引擎逻辑与 Tauri 零耦合。所谓解耦实为**剥离命令壳 + 挪 AppState**。

#### 四处引用的改法

| 位置 | 现状 | 改法 |
|---|---|---|
| `git/mod.rs:18,145-165` | `use tauri::State` + 3 个 command 薄壳（git_status/diff/recent_log），壳内只做 `resolve_session_roots` + 调 `status_core/diff_core/log_core` | 3 个 command **剥出留在旧壳**（`host/commands/` 下）；`*_core` 与类型随 engine 走 |
| `knowledge/commands.rs` | 同款双层（skill_list/toggle/revoke_trust/reload 等），壳 = 宏 + `tauri::State<AppState>` 转发 `*_core` | 同上：command 壳留壳，`*_core` 进 engine |
| `mcp/commands.rs` | 同款双层（8 个命令） | 同上 |
| `tools/render_html.rs:114` | 唯一真耦合：`preview_url()` 按 Tauri 自定义协议拼 `preview://` / `preview.localhost` | `preview_url` **留壳**（预览承载是壳关注点，GPUI 下另定）；`PreviewRegistry`/token/`is_valid_token` 进 engine |

#### AppState 挪移

`host/mod.rs:53` 的 `AppState` 字段全部是引擎组件句柄（ConfigStore/RunManager/SessionStore/ProjectStore/McpHost/SubRegistry/HttpClientFactory/UsageLedger/ApprovalMemory/FollowUpManager/EventSink…），**零 Tauri 类型**。整结构挪进 engine（含 `share_handles`）。Tauri 特性只存在于 `EventSink` 的生产实现（tauri emit）——trait 在 `util::events`，生产 sink 是壳的实现，engine 只认 trait。

#### 调用方适配表（阶段 1.4 旧壳接线）

- 旧壳 `lib.rs` 的 `manage(AppState)` → 改 manage engine 的同一类型；
- 4 处剥出的 command 壳 import 改 `engine::…`；
- `host::IpcError` 已是 `util::errors::IpcError` 的 re-export（host/mod.rs:31），util 随 engine 走，壳继续 re-export 即可；
- `host/commands/workspace::resolve_session_roots` 依赖 `AppState`——属引擎域（会话 roots 解析），随 engine 走；
- `render_html` 协议 handler（lib.rs 注册）留壳，调 engine 的 registry/token 校验。

#### storage 死代码剥离清单（数据不迁移策略下）

- `storage/backup.rs`（整文件，33 号票「存储格式升级备份守卫」）——其存在前提是「旧版应用要能读」；新栈数据从零开始，无意义。剥离时同步清掉各写入入口的 `ensure_upgrade_backup` 调用点与 `.upgrade-<reason>.done` 标记逻辑（grep `ensure_upgrade_backup` 得调用面）；
- storage 其余文件（`stats.rs`/`entries.rs`/`index_store.rs` 等命中"升级"字样处）经查为文档注释提及，非守卫依赖，随 engine 正常搬移。

#### 对阶段 1 的修订建议

1.2 搬模块时逐模块执行：`commands.rs` 剥出留壳 → 其余进 engine → `cargo test`。1.3 由「trait 化」改为「剥壳 + 挪 AppState + 剥 backup」。预估从"数天"降到**一天内**。
