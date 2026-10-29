# 实现：M1 欠账清偿与设置页打磨

Type: task
Status: open

## 要做什么

清偿 M1 挂账的设置页与配置面余量，让"设置"成为日常可依赖的入口：预设清单 Rust/前端双份副本收敛为单源（消灭漂移）；补数据目录入口（FR-14-07：一键打开数据/日志/项目数据目录，排障第一入口）；补自定义提示词注入层（FR-09-06）。原票面所列"设置页补外观/安全/诊断组、脏状态三选"经代码核对已随 M1 后续工作落地（见下核对表），本票只做回归确认；"pending_intent 意向残留提示"一项作废（依据见下）。

## 具体做什么

### 现状核对（动工前先确认，避免重复实现）

- 已落地（本票仅回归）：
  - `ui/src/settings/registry.ts`：三组 10 页导航（appearance/providers/network/security/mcp/skills/tools/agent/logs/about）、页序与页名；
  - `SettingsPage.tsx`：逐页脏标记（pageSlice 字段差集 + 假脏归一化）与"保存/放弃/留下"三选离开拦截（切页/返回/Esc 共用）；
  - 外观页（主题/系统通知/语言含 aiLanguage）、网络页（代理三模式）、安全页（审批开关/autoConfirm/命令白名单只读）、日志页（级别四档）、关于页。
- 作废项：原票"pending_intent 意向残留 UI 提示"——v2.5 用户决策废除 keyring 后，keyring 时代的 intent.json 意向协议已随 §9.4 一并移除（`storage/config_store.rs` 头注释明载；ConfigStore 现仅 new/path/load/save/update 五个 API，无 pending_intent）。本票不实现，并在 M1 12 号票 Answer 对应行加"已作废（v2.5）"注记（该票已收官则仅在 Answer 记录，不改历史票 Question）。

### 后端（src-tauri/）

- 预设单源化，定案为"生成物"方案（与 02 号票 ts-rs 面衔接，命令面不增长）：
  - `providers/presets.rs` 为权威（现 168 行，6 家内置预设含建议模型），预设清单类型 derive TS + 导出测试，生成 TS 常量进 `ui/src/ipc/generated/`；
  - §3.3 回写注记"预设经生成物单源，不设 list_presets 命令"（§3.3 现无该命令席位；M1 12 号票曾决议"不加命令、两侧测试钉住漂移"，本票收敛掉该临时状态）；
  - 若实现时发现生成常量不适合运行时结构，改走命令方案前须先回写 §3.3 增补行。
- `open_dir` 命令（§3.3 #3，`{ path: string } → void`，P0 表已定义、当前未注册）：
  - 注册进 `ipc_commands!`（lib.rs 单一事实源）；
  - 校验 = path 落在固定白名单内：数据目录 `~/.craftwave/`、日志目录 `~/.craftwave/logs/`、当前项目数据目录 `<homeDir>/.craftwave/`（语义见 §7.1 目录布局）；越界返回 E_PATH_OUTSIDE_ROOTS；
  - OS 打开失败归 E_IO（依赖 02 号票补行）；
  - 打开载体：仓库现无 opener 依赖（无 tauri-plugin-opener、无 open crate），按 AGENTS.md "非必要不手写"约定调研后引入 tauri-plugin-opener（官方插件）或 open crate，二选一并在 Answer 记录。
- 自定义提示词注入层（FR-09-06）：
  - AppConfig 增字段——§3.2 未定义字段级（现配置只有 providers/activeModelId/proxy/approval/ui/log 七组），先回写 §3.2 再实现；建议形态 `ui.customPrompt?: string`（或独立 `agent` 组，回写时定）；
  - 注入点 = start_chat 的 wire 重建出口（`backend/wire.rs` history 构建处）作为 system 前缀追加；与 04 号票压缩 summary、计划快照的层叠次序在回写 §6.2 时一并钉死（建议次序：customPrompt → 压缩 summary → 当前计划）；
  - 空/未配置零注入；配置经 save_config 即时生效于下次 run（ConfigStore 缓存热替换，`storage/config_store.rs` 既有机制）。

### 前端（ui/）

- `settings/presets.ts` 删除本地副本（现 145 行），改 re-export 生成物；`ProvidersPage.tsx` 预设选择逻辑不变；"slug 对齐由两侧测试钉住"的临时防线随单源消亡一并撤除。
- 数据目录入口三处（按钮统一走 open_dir，失败行内提示）：
  - 关于页加"打开数据目录"（open `~/.craftwave/`，页内已展示该路径文本，按钮与之并列）；
  - 日志页加"打开日志目录"（open `~/.craftwave/logs/`，目录不存在则先 create_dir_all）；
  - "工作区与智能体"页（`settings/registry.ts` 的 agent 页）加"打开项目数据目录"。
- 自定义提示词设置项：多行文本框落"工作区与智能体"页（字段归属随 AppConfig 字段落点定），走既有 settings store 草稿 + saveAll 提交（`stores/settings.ts` patchConfig/saveAll 机制）；页内一行说明"注入为每次对话的系统级前缀"。

### 测试

- Rust：
  - open_dir 白名单表驱动测试（三个合法目录 + 越界拒绝 E_PATH_OUTSIDE_ROOTS，临时目录构造）；
  - presets 生成物导出测试（02 号票机制复用）；
  - customPrompt 注入后 wire history 含 system 前缀的单测（`backend/wire_test.rs` 扩展，有/无 customPrompt 两态）。
- 前端：
  - `SettingsPage.test.tsx` 回归（预设消费生成物后零回归）；
  - open_dir 三按钮接线测试（mock ipc/client）；
  - customPrompt 草稿-提交-回显 store 测试（`stores/settings.test.ts` 扩展）。
- 契约：`contract.test.ts` 增 open_dir 接线（命令 ⊆ manifest + client COMMANDS 注册）。

## 边界（不在本票）

- 诊断组深度功能：read_session_log 查看器、list_log_files、open_logs_dir 独立命令（§3.3 #54–56）随 32 号票（会话日志）；本票日志目录入口用 open_dir 承载。
- MCP/技能/写入后检查/工作区与智能体四占位页的实装（四页仍 PlaceholderPage）——随 08/09/22 号票各自票。
- custom_prompt 的按会话/按项目覆盖层级（FR-09-06 只要求全局注入层）；提示词模板变量。
- §10.7 M2 设置增项（Key 池状态、网络页扩展等）归 18–21 号票。

## 验收

- [ ] 预设清单仅 `providers/presets.rs` 一处权威，前端零副本；改一处预设字段前端类型即时感知（编译期）。
- [ ] 设置页可一键打开数据目录、日志目录、项目数据目录；白名单外路径被拒。
- [ ] custom_prompt 配置后新 run 的模型请求含该系统前缀（mock 后端断言），清空后不注入。
- [ ] 脏状态三选与各设置页回归全绿（SettingsPage.test.tsx 既有用例零回归）。
- [ ] §3.2（customPrompt 字段）、§3.3（open_dir 接线与预设单源注记）、§6.2（注入次序）回写完成；检查链七项全过。
