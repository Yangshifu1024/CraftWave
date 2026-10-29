# 实现：M1 欠账清偿与设置页打磨

Type: task
Status: resolved

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

- [x] 预设清单仅 `providers/presets.rs` 一处权威，前端零副本；改一处预设字段前端类型即时感知（编译期）。
- [x] 设置页可一键打开数据目录、日志目录、项目数据目录；白名单外路径被拒。（校验面与前端接线经测试验证；OS 打开动作本体未做真实桌面人工点验，见决议记录 11）
- [x] custom_prompt 配置后新 run 的模型请求含该系统前缀（mock 后端断言），清空后不注入。（断言落 wire 层单测，见决议记录 6）
- [x] 脏状态三选与各设置页回归全绿（SettingsPage.test.tsx 既有用例零回归）。
- [x] §3.2（customPrompt 字段）、§3.3（open_dir 接线与预设单源注记）、§6.2（注入次序）回写完成；检查链七项全过。（cargo fmt/build/clippy/test 282 例 + pnpm typecheck/test 152 例/lint 全绿）

## 决议记录（2026-10-30 实施）

1. **预设"生成 TS 常量"的落点**：ts-rs 12 只生成类型声明、不生成运行时常量，票面预留的"不适合运行时结构"分支出线——常量改由 `ipc_manifest_test::export_contract_types_and_match_manifest` 在 cargo test 内以 serde 序列化 `builtin_presets()` 写 `ui/src/ipc/generated/BuiltinPresets.ts`（与 02 号票"生成物进 git、cargo test 统一导出"机制同构）；类型声明 `ProviderPreset.ts`/`PresetModel.ts` 由 export_all 补根导出（预设不在任何命令负载闭包内）。`&'static str`/`&'static [T]` 字段 ts-rs 12 原生支持（`str` 基础实现 + `&T` 包装实现），Rust 权威清单保持零拷贝静态形态不动。前端 `settings/presets.ts` 收敛为消费层（re-export 生成物 + 实例化辅助），"slug 两侧测试钉住"防线撤除并改为生成物消费冒烟（settings.test.ts）。
2. **预设内容收敛取并集**：单源化核对发现 M1 实际漂移——前端副本的 zhipu 预设含 glm-5.3/glm-5.3-flash 而 Rust 权威只有 glm-4.6。收敛时把前端较新模型回填 Rust 权威清单（避免单源化造成"从预设添加"功能回退），内容以 Rust 侧为唯一权威。
3. **open_dir 打开载体选 open crate 5.4.5**（票面二选一）：与 select_dir 用 rfd 免 JS 插件与 capability 的既有取舍一致——tauri-plugin-opener 会引入整套插件 IPC 面与 capability 配置，而本票只需要"Rust 校验后打开"一个动作；open crate 纯函数 `that_detached`（macOS 走 /usr/bin/open 即交即返，Windows/Linux detached 不阻塞）。
4. **open_dir 校验面拆分**：`resolve_open_dir`（headless 可测、无系统副作用）+ `open_dir_core`（校验后调用 open crate），表驱动测试全部走校验面。白名单 = 数据目录（启动即建）+ 日志目录（`open_dir_roots` 幂等补建——票面"目录不存在则先 create_dir_all"落点在后端而非前端）+ 各注册项目的 `<homeDir>/.craftwave`（save_project 注册即建）；残缺注册（主目录被外部删除）自动退出白名单报越界不 panic。前端不感知 home 绝对路径：`~`/`~/…` 前缀由后端 `expand_home_prefix`（HOME/USERPROFILE）展开后照走白名单（安全语义由"展开路径仍被白名单拒绝"用例钉住）。错误分层沿用 PathGuard 既有口径：越界 E_PATH_OUTSIDE_ROOTS、白名单内目标不存在/非目录 E_NOT_FOUND。
5. **customPrompt 落 `agent.customPrompt`（agent 组）而非票面建议的 `ui.customPrompt`**：语义属智能体行为（注入为对话系统级前缀），对应设置页"工作区与智能体"页；§3.2 agent 组 M2 已预定义（maxConcurrentSubagents/commandAudit），customPrompt 是该组首个实装字段。归一化空白→None（与 apiKey 同口径）。
6. **注入实现与断言落点**：`backend/wire.rs` 新增 `with_system_prefix`（history 头部插 System 消息），`start_chat_core` 在 wire 切分后从 ConfigStore 读取注入——即票面"注入点 = start_chat 的 wire 重建出口"；未走 rig `RunSpec.preamble`（两条路在模型请求面等价，选 wire 层切入使 04 号票的压缩 summary 可在同点续接 §6.2 次序）。"mock 后端断言"落为 wire 层单测（有/无/空白三态 + trim 归一 + prompt 不受影响）——仓库无现成 HTTP mock 基建（S2/S3 的 axum mock 服务未进主干），不发真实网络请求的单元断言即本票可用的 mock 口径。
7. **agent 页从纯占位收窄为"承载本票两项"**：边界节"四页仍 PlaceholderPage"按票面前端小节的明确要求（该页加 customPrompt 文本框与项目数据目录入口）收窄——agent 页改为真实页体但只含本票两项，Shell 选择/压缩阈值等余项仍留后续票（占位提示文案相应移除）。
8. **"当前项目数据目录"的口径**：白名单收各注册项目的 `<homeDir>/.craftwave`（非仅"活跃会话所属项目"）——校验面与用户可开面等价（均为用户自行注册的目录，不越权），前端按钮按活跃 Tab 所属项目计算路径，无项目会话时按钮禁用并说明原因。
9. **§3.2 顺带回写既有漂移**：AppConfig 结构回写时把 M1 已实装但文档漏记的 `ui.fontSize`（12–18px）与 `notifications` 组一并补入，使字段级设计与代码一致（仅文档回写，无代码改动）。
10. **检查链**：七项全过——`cargo fmt --check`、`cargo build`、`cargo clippy --all-targets`（零警告）、`cargo test`（lib 257 + 集成 25，含 system_test 表驱动 6 例、wire 注入 3 例、agent normalize 1 例）；`pnpm typecheck`、`pnpm test`（152 例，新增 open_dir 三按钮接线 3 例 + customPrompt 页面 1 例 + store 单源/customPrompt 2 例 + 契约锚点）、`pnpm lint`。已知既有抖动（thread-rendering.test.tsx 建议预填用例偶发）本轮未出现。
11. **未人工点验项（诚实声明）**：OS 打开动作本体（open::that_detached 调起 Finder/Explorer）与三按钮在真实桌面的视觉落位未经人工运行验证——逻辑、校验与接线均有自动化测试覆盖，真实桌面体验待用户日常使用确认。
