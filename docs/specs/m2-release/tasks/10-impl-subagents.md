# 实现：子代理（subagent 工具、内置角色、$ 委派、治理与可见性）

Type: task
Status: open

## 要做什么

主 agent 可把任务派给专职子代理（探索/规划/实现/评审/测试等 7 个内置角色）：独立上下文与预算、并行干活互不踩文件、过程实时可见可回看；用户也能在输入框 `$explore 帮我找…` 直接点名。完成后「委派→过程直播→结构化汇报回父」全链可用。覆盖 FR-07-01/02/04/05/06/07/09 + §5.9 + §3.4 sub 帧组 + §3.5 sub 事件组。

## 具体做什么

### 后端（src-tauri/）

- **`subagent` 工具注册**（§8.3 M2 表，Meta 能力）：`{ role, task, budgetSteps? }`；工具 description 内嵌 7 角色清单与各自职责一句话（模型点名依据）。
- **角色表**（FR-07-01，`agent/roles.rs` 新建）：explore/planner/implementer/reviewer/tester/frontend-dev/backend-dev，各=系统提示文本 + 工具白名单 + 建议模型（缺省跟随父 run）；白名单经 `filter_for_tier` 同一实现裁剪（§8.1「工具可见面=执行许可=子代理边界」三处同源纪律）；步数预算默认 explore 40 / dev 类（implementer/frontend-dev/backend-dev）90 / reviewer 45 / tester 60（§5.9），`budgetSteps` 入参钳制 1–1000。
- **运行时**（§5.9，`agent/sub_registry.rs` 新建 + backend 接线）：工具执行=查角色表 → 构建 AgentBuilder（**默认 cleanContext**；可配带父历史尾 6 条，经防腐层 RequestPatch 透传，§5.9 末行）→ 独立 RunHandle 挂同会话 SubRegistry（全局共享并发上限，默认 4，读取 `AppConfig.agent.maxConcurrentSubagents`，满则排队）；帧面发 `subStarted{callId, subId, role}`（§3.4 M2 帧），子 run 的 ChatFrame 逐帧包 `subFrame{subId, frame}` 透传（仅主代理派发，子代理帧直接归属该主 run，不产生孙代理）。
- **仅主代理委派（2026-10-29 用户确认 Q13）**：子代理工具表过滤 `subagent`，执行入口同时拒绝子代理委派；进一步拆任务须汇报主代理协调。工具可见面与执行许可同源，禁止只靠提示词约束。
- **可配置全局并发（Q13 用户补充）**：应用内所有会话共用一个子代理并发限制，默认 4；`AppConfig.agent.maxConcurrentSubagents` 为正整数，缺省回填 4，非法值在保存边界拒绝；沿用 schemaVersion=2 的配置兼容与 `save_config` 原子持久化。已有工具批次并发限制独立，不被此字段改变。运行中修改上限：已启动任务继续，不强制取消；降低后暂停补入，直到运行数量低于新上限；提高后立即按新上限启动排队任务。
- **治理**（FR-07-04）：步数耗尽 → E_SUB_BUDGET 工具结果 + outcome=partial；`stop_subagent` 命令（§3.3 #58 `{subId}`）与父 run cancel_run 均级联子 CancellationToken 树（S5 取消传播原语复用，收齐语义同 §5.3）。
- **公共操作归主代理（2026-10-29 用户决策）**：全项目格式化等作用于共享项目、可能覆盖其他代理修改的公共操作，只允许主代理执行；子代理需要此类操作时向主代理汇报，由主代理协调执行。执行许可必须在工具调用入口检查，不能只依靠角色提示词；command/MCP 等入口遵循同一限制。独立工作区内局部测试属于局部操作；共享项目或外部资源的变更仍属公共操作，具体适用规则见下文。
- **公共操作时序（2026-10-29 用户确认 Q8）**：主代理等待相关子代理完成、收齐改动后再统一执行公共操作。子代理可在自己的 worktree 内执行局部测试；数据库、外部服务等共享资源操作仍属公共操作，worktree 不提供此类资源隔离。
- **worktree 适用范围（2026-10-29 用户确认 Q7）**：Git 项目中需要改文件的子代理使用独立 worktree；只读探索与评审读取当前工作区。非 Git 项目由主代理负责修改，子代理仅做只读任务。
- **派发起点（2026-10-29 用户确认 Q9）**：子代理获得派发时当前文件状态，包括未提交修改与未被忽略的新文件；后续主工作区变化不自动同步。记录实际派发起点供差量回收；不为创建副本自动提交代码，不改变用户已有暂存状态。快照创建失败不得静默退回旧 HEAD。
- **结果回收（2026-10-29 用户确认 Q10）**：主代理将子代理相对派发起点的改动汇回当前工作区；无冲突直接应用，有冲突保留双方内容，由主代理结合当前代码解决并验证，保留用户已有修改。全过程不自动 commit/push。
- **失败、取消与清理（2026-10-29 用户确认 Q11）**：失败或取消即停止执行，保留未汇回 worktree 与修改，标注未完成，不自动汇入主工作区；主代理可检查并回收有用部分。应用重启后仍保留这些成果但不自动续跑；成功汇回并验证后才清理对应 worktree。生命周期状态与恢复索引先补齐 §5.9/§7.7。
- **环境准备（2026-10-29 用户确认 Q12）**：子代理在独立 worktree 内按项目现有方式安装依赖、生成构建产物；不批量复制主工作区被忽略的文件。本地配置缺失时向主代理报告，由主代理协调；沿用既有权限、审批和公共操作边界。
- **替代共享目录认领**：独立工作区与回收冲突处理取代原 FR-07-05 的跨子代理共享目录认领方案，不再认领祖先目录；父级同批冲突检测仍遵循 §5.4。工作区路径、生命周期、起点元数据与回收契约先补齐 §5.9/§3.3/§7.7，再实现；与 26 号票区分应用受控工作区管理和模型 git 命令，默认禁止模型 commit/merge 的策略不变。
- **汇报与四象限**（FR-07-02/09）：子 run 终态 report/partial/no_report/cancelled 四象限；最后一次 assistant 文本即 `<report>` 结构化汇报 → 以父工具结果回喂 + `subReport{subId, report}` 帧 + `subDone{subId, outcome}` 帧（§3.4）；父级按象限处理：report 继续、partial 注明预算耗尽可续派、no_report/cancelled 注明失败原因。
- **存储与回看**（FR-07-06）：子代理历史独立落 `histories/subs/<parentRunId>/<subId>/`（§5.9；单 JSONL 最小子集，复用 §7.4 原子写）；`load_subagent_history` 命令（§3.3 #59 `{sessionId, subId} → WireMessage[]`）。
- **事件 6 键**（§3.5 M2 表）：`sub:spawn/sub:step/sub:report/sub:usage/sub:done/sub:error`，payload `{sessionId, runId, subId, role?, turn?, usage?, outcome?, code?}`——跨 Tab Inspector 订阅；`util/events.rs` 增键常量。

### 前端（ui/）

- **`$` 触发符**（FR-07-02）：`chat/composer-keys.ts` 增 `detectDollarQuery`（与 detectAtQuery 同构光标锚定）→ 角色菜单（静态 7 角色，↑↓/Tab/Enter/Esc 同 @ 菜单键位）；选中插入 `$角色名 ` 后用户续写任务，发送文本原样入会话（模型经 subagent 工具 description 理解点名语义；Tab 优先级并入 §10.6：触发符菜单按首字符 `/`/`@`/`$` 互斥 > hint）。
- **设置页**：复用 `agent` 页（工作区与智能体），增加“子代理全局并发上限”数字输入，缺省 4；保存经既有 get_config/save_config 接线并展示校验错误。
- **Inspector 子代理页**（§10.7 M2 五页之一，`chat/Inspector.tsx` 增 tab）：`sub:spawn/done` 事件 + subFrame 驱动列表（角色徽标/subId/状态/步数/用量）；运行中自动跟随展开活跃项；抽屉回看=`load_subagent_history` 拉全量只读渲染。
- **subagent 工具卡**（`chat/ToolCards.tsx` + `chat/toolkit.ts`）：角色徽标 + task 摘要 + 实时步数/状态（subFrame 驱动）+ `<report>` 折叠全文（subReport 后展开可见）；subDone 徽标按四象限着色。
- **store**：`stores/subagents.ts`（zustand 事件驱动：spawn/done 增删、step 计数、usage 累计、抽屉开合态）。
- 契约测试：manifest 增 subStarted/subFrame/subReport/subDone 4 帧、sub:* 6 事件、#58/#59 命令、subagent 工具行。

### 测试

- 热调整回归：4 个运行任务时降到 2 不取消已有任务、不补入直到少于 2；调高上限后排队任务立即获得新增名额。
- 并发配置：旧配置缺字段回填 4；非正整数拒绝；设置页保存/重载往返；多会话共同占用上限，满额排队；子代理工具表不含 subagent，绕过工具表直接调用亦拒绝。

- 生命周期回归：失败/取消成果跨重启保留且不自动汇回/续跑；成功回收验证前不清理。环境准备不批量复制忽略文件，缺失配置产生可见汇报。

- ScriptedBackend 驱动（§12.5 点名场景）：默认并发 4（第 5 个排队不拒）、配置更改后跨会话共用新上限、子代理委派被拒、预算耗尽 → partial + E_SUB_BUDGET、两个 worktree 修改同文件互不覆盖、汇回冲突保留双方、父取消级联（子 runEnded(cancelled) 帧收齐）、四象限各一例、帧序断言（subStarted 先于子帧、subDone 必收尾）。
- 单测：角色表白名单裁剪、派发起点包含已暂存/未暂存修改及未忽略新文件、相对起点的新增/修改/删除回收、budgetSteps 钳制、cleanContext 与父历史注入两形态的消息构造。
- worktree 回归：派发后主工作区变化不自动同步；只回收子代理新增差量；无冲突直接应用，有冲突保留双方并解决验证；创建/回收不自动提交，不改变用户已有暂存状态。
- e2e：explore 角色真实委派端到端（fixture 技能式脚本），过程实时可见（subFrame 逐帧）、stop_subagent 干净终止。

## 边界（不在本票）

- 压缩（04 号票）与并行批次执行（07 号票）既有机制直接复用不改动；子代理嵌套委派（Q13 明确不做）；M3 `wait`/`service` 工具（§5.10）；子代理历史进检索/索引。

## 验收

- [ ] 失败/取消保留未汇回 worktree 与修改并标注未完成，不自动汇入；重启仍可找回且不自动续跑；成功汇回并验证后才清理。依赖在独立 worktree 内按现有方式准备，缺失配置报告主代理，不批量复制被忽略文件。

- [ ] Git 项目的写入子代理使用独立 worktree；只读任务读取当前工作区；非 Git 项目禁止子代理修改。公共操作等待相关子代理完成并收齐改动，子代理独立 worktree 内的局部测试可执行。

- [ ] 子代理不能执行公共操作；请求返回主代理协调，工具调用入口有权限校验，command/MCP 入口不能绕过该限制。

- [ ] `$explore …` 点名与模型自发 `subagent` 委派均端到端可用，`<report>` 回父上下文
- [ ] 子代理过程在消息流（subFrame）与 Inspector 子代理页实时可见，抽屉可回看历史
- [ ] 默认第 5 个并发子代理排队；设置页可修改并持久保存全局上限，跨会话共同受限；子代理不能继续派发；步数预算耗尽得 partial；stop_subagent 与父取消级联干净
- [ ] 子代理获得派发时文件状态（含未提交修改和未忽略新文件），后续不自动同步；按起点差量回收，冲突由主代理解决并验证，保留用户已有修改，不自动 commit/push。
- [ ] 重启后 `load_subagent_history` 可回看已完成子代理历史
