# 实现：MCP 客户端接入（双传输、连接池、信任、管理页）

Type: task
Status: open

## 要做什么

用户把自己的 MCP 服务器接进 CraftWave：全局或项目作用域配置一次，工具自动发现并进入模型工具表，调用有审批与启动信任两道防线，断线自动重连，设置页可编辑、启停、试连。完成后接一个真实 MCP server（如 filesystem）即可端到端干活。覆盖 FR-08-01/02/04/05/06/08 + §9.7 字段级。

## 具体做什么

### 后端（src-tauri/）

- 新模块 `mcp/`（§2.2 域归属：与 host 同层的独立模块，Cargo.toml 增 `rig-rmcp`）：
  - `mcp/config.rs`：双作用域配置——全局 `~/.craftwave/mcp.json` + 项目 `<home>/.craftwave/mcp.json`，同名覆盖（FR-08-02）；服务器条目字段逐字：`command/args/env/cwd/url/headers/enabled/timeout/read_only/always_allow/tools{allow,deny glob}`；**未知字段 round-trip**（serde `flatten` + 保序 map）。项目作用域 mcp.json 属 §9.8 信任资源——加载接 24 号票信任门（24 未落地则先按最小版：首次读项目配置弹确认，票面记录收敛点）。
  - `mcp/pool.rs`：连接池。key=`(scope, projectId, name, canonicalPath)`（§9.7），LRU ≤16；状态机 Starting→Ready→Stopped/Error，每次迁移发 `mcp:status` 事件（§3.5 M2 键，payload `{scope, name, state: "starting"|"ready"|"stopped"|"error", detail?}`；`util/events.rs` 增键常量）；generation 计数防旧握手复活（重连后旧初始化回调丢弃）；stdio 子进程复用 `tools/process` 进程组/Job Object 收割原语（断开/超时整树收割）；断线指数退避重连（1s/2s/4s…封顶 60s）。
  - `mcp/trust.rs`：TOFU 启动信任——指纹=`(scope, projectId?, name, canonicalConfigPath, sha256(config))`；stdio 启动 / HTTP 握手前无信任记录 → AskSlot + `ask:opened` 携 `AskPayload{kind:"mcpTrust"}`（§3.2 已有字段级：`{server, transport, fingerprint, commandOrUrl}`，options `allow_once/remember/deny`）；"记住"持久化与"指纹变更重问"必须实现。**出入回写点：§9.7/§3.3#46 写"决策写 keyring"——v2.5 已废除钥匙串（§9.4），按用户决策落 `~/.craftwave/trust/mcp.json`（对齐 §7.7 trust 域），回写 §9.7/§3.3 两处 keyring 字样。**
  - `mcp/tools.rs`：工具接入——tools/list 转 DynamicTool；**工具名归一**（FR-08-06）：非法字符替换为 `_`（合法集 `[a-z_][a-z0-9_]{0,63}`，§8.1）、跨 server 碰撞加 `__2` 序号后缀、维护「归一名 → (server, 原名)」反向映射供调用还原；`tools/list_changed` 通知 → 重拉清单热更新注册表并重导 toolSpecs；能力标注：server 声明 `read_only` → ReadOnly，否则 Network；annotations 透传；`tools.allow/deny` glob 在注册时过滤。
- 注册表动态化（§8.1「MCP 动态工具并入 ToolServer 模式」）：`ToolSpec.name` 现为 `&'static str`（tools/mod.rs:82）——动态段需改 `String`（或归一名驻留池，票内定案）；`filter_for_tier` 增 MCP 规则：**plan 档全部过滤**（§8.5 注）；`TOOL_NAMES` 钉死测试改「静态段 + 动态段」两断言。`backend/exec.rs` dispatch 增动态分支：归一名反查 → 池内 server 工具调用（executor trait 化形态票内定案）。
- **断连后结果未知（2026-10-29 用户确认 Q14）**：连接可自动恢复；已发出而结果未知的工具调用不自动重放，read_only 声明也不豁免。向主代理返回“可能已执行”的可辨识结果，由主代理先核查实际结果再决定是否重试；不得经通用失败重试路径重发。错误码/结果字段实现前先回写 §3.6/§8.2。
- **worktree 信任接缝（Q19 用户确认）**：受应用管理的 worktree 按原项目归一资源身份，同资源同内容沿用既有信任；仅副本路径改变不重复询问，但命令/环境/cwd 等实际启动配置改变仍须重核，跨项目不继承。与 24 号票一起回写指纹字段。
- 调用审批（FR-08-04）：`backend/gate.rs` 判定链增 MCP 分支——server 未声明 read_only 且未开 always_allow 时**即便 full_access 也挂起确认**（AskPayload approval，title 带 server 名与工具名）；"始终允许"热更新不重启连接。
- **动态工具授权（Q44）**：工具清单仍自动同步，但新增工具或输入 schema 变化时，不继承先前免审批许可，首次调用须重新确认；旧 server 级 read_only/always_allow 不自动覆盖这些新版本。许可须绑定服务器身份、原始工具名与已确认输入 schema，不能只绑定会因归一或碰撞而变化的展示名；具体指纹与存储字段先补齐 §9.7，再实现。
- read_only/always_allow 只取用户确认的配置，远端 annotations 仅作元数据，不得据此升级为只读或免审批；仅描述文字变化可提示更新，不扩大许可。配置变更与启动信任仍遵守既有 TOFU，工具变更许可与服务器启动信任分别校验。
- 命令 8 个（§3.3 #40–47 逐字）：`mcp_list_config {} → {global, project}`、`mcp_save_config {scope, config} → void`、`mcp_connect {scope, name} → void`（触发 TOFU）、`mcp_disconnect {scope, name}`、`mcp_reconnect {scope, name}`、`mcp_test {draft} → {ok, tools:[{name,description}], error?}`（临时连接 initialize+tools/list，**总预算 60s**，不进池）、`mcp_set_launch_trust {fingerprint, decision}`、`mcp_snapshot {} → {servers:[{name, state, toolCount}]}`。
- 传输：stdio（`command+args+env+cwd`，tokio process 承载）与 streamable HTTP（`url+headers`）双实现（FR-08-01）。

### 前端（ui/）

- 设置页 MCP 管理组（§10.7，`settings/SettingsPage.tsx` 增分节/页签；草稿态入 `stores/settings.ts`）：双作用域编辑器（保存经 `mcp_save_config`，保存前后未知字段不丢的 diff 提示）；服务器行=名称/作用域徽标/启停按钮（mcp_connect/disconnect）/状态徽标（订阅 `mcp:status` 实时）/工具数（mcp_snapshot）；「测试连接」按当前草稿调 `mcp_test`；信任状态列（已记住指纹 / 待决策）。
- 通用 MCP 工具卡：`chat/toolkit.ts` 无法静态枚举动态工具——增 FallbackCard 兜底渲染（工具名 + 参数 JSON 折叠 + uiSummary + 错误态），未命中专用卡的工具调用一律落此卡（assistant-ui 无匹配 UI 时的兜底入口形态票内验证）。
- `chat/AskPanel.tsx` 增 mcpTrust 第三形态：server/transport/commandOrUrl/fingerprint 摘要 + 三选项按钮 → `resolveAsk`。
- 契约测试：manifest 增 #40–47 命令、`mcp:status` 事件、mcpTrust 载荷类型。

### 测试

- Q44 fixture：已许可服务器新增工具、或原工具输入 schema 改变，首次调用重新询问，旧 read_only/always_allow 不自动放行；仅描述变化不扩大许可；远端 annotations 声称只读不能提高权限；碰撞重命名不能串用另一工具许可。
- fixture 在记录已执行操作后、返回响应前断连：连接恢复但调用计数保持 1；结果明确标记可能已执行，通用失败重试不重放原调用；read_only 声明下同样覆盖。

- 本地 stdio fixture echo server（§12.5 定的测试基建）：覆盖 connect→TOFU allow/remember/deny 三路径、工具发现注册、调用回喂、断连重连、`mcp:status` 四态、`tools/list_changed` 同步。
- 单测：配置合并与未知字段 round-trip、工具名归一/碰撞后缀/反向还原、池 LRU 驱逐与 generation 失效、审批判定表（read_only × always_allow × tier 四维）。
- e2e（ScriptedBackend + fixture）：审批挂起→allow 续跑全链（复用 M1 14 号票模式）。
- 手测验收：接真实 filesystem server 端到端读写文件。

## 边界（不在本票）

- MCP prompts/resources（`mcp_read_resource` #72 是 M3）；项目资源信任通用机制与外部目录授权（24 号票，本票仅最小信任存储并在其落地时收敛）；用量计量/配额无关；MCP server 注册为斜杠命令（FR-01-17 P2）。

## 验收

- [ ] 动态新增/输入 schema 变化不继承旧免审批许可，首次调用重新确认；仅用户确认的 read_only/always_allow 可决定权限，远端 annotations 不提升权限。
- [ ] 已发出但结果未知的调用不自动重放；连接恢复后主代理收到“可能已执行”的明确结果，先核查再决定重试。

- [ ] 全局与项目双作用域 mcp.json 加载、同名覆盖、未知字段保存不丢
- [ ] stdio 与 streamable HTTP 各接通一个 server，工具进模型工具表且调用结果回喂
- [ ] 未声明 read_only 的 server 调用即便 full_access 也弹确认；plan 档 MCP 工具全过滤
- [ ] 首次连接 TOFU 三选项可用，「记住」后免问、配置指纹变更重问
- [ ] kill server 子进程后自动重连，`mcp:status` 状态迁移全程可见
- [ ] 管理页编辑/启停/测试连接可用；fixture 测试 + 检查链全绿
