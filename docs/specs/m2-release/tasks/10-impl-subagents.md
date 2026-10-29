# 实现：子代理（subagent 工具、内置角色、$ 委派、治理与可见性）

Type: task
Status: open

## 要做什么

主 agent 可把任务派给专职子代理（探索/规划/实现/评审/测试等 7 个内置角色）：独立上下文与预算、并行干活互不踩文件、过程实时可见可回看；用户也能在输入框 `$explore 帮我找…` 直接点名。完成后「委派→过程直播→结构化汇报回父」全链可用。覆盖 FR-07-01/02/04/05/06/07/09 + §5.9 + §3.4 sub 帧组 + §3.5 sub 事件组。

## 具体做什么

### 后端（src-tauri/）

- **`subagent` 工具注册**（§8.3 M2 表，Meta 能力）：`{ role, task, budgetSteps? }`；工具 description 内嵌 7 角色清单与各自职责一句话（模型点名依据）。
- **角色表**（FR-07-01，`agent/roles.rs` 新建）：explore/planner/implementer/reviewer/tester/frontend-dev/backend-dev，各=系统提示文本 + 工具白名单 + 建议模型（缺省跟随父 run）；白名单经 `filter_for_tier` 同一实现裁剪（§8.1「工具可见面=执行许可=子代理边界」三处同源纪律）；步数预算默认 explore 40 / dev 类（implementer/frontend-dev/backend-dev）90 / reviewer 45 / tester 60（§5.9），`budgetSteps` 入参钳制 1–1000。
- **运行时**（§5.9，`agent/sub_registry.rs` 新建 + backend 接线）：工具执行=查角色表 → 构建 AgentBuilder（**默认 cleanContext**；可配带父历史尾 6 条，经防腐层 RequestPatch 透传，§5.9 末行）→ 独立 RunHandle 挂同会话 SubRegistry（**全局并发信号量 ≤4**，满则排队）；帧面发 `subStarted{callId, subId, role}`（§3.4 M2 帧），子 run 的 ChatFrame 逐帧包 `subFrame{subId, frame}` 透传（一层嵌套；嵌套子代理展平=孙帧仍以 subFrame 直挂父面，subId 区分归属）。
- **治理**（FR-07-04）：步数耗尽 → E_SUB_BUDGET 工具结果 + outcome=partial；`stop_subagent` 命令（§3.3 #58 `{subId}`）与父 run cancel_run 均级联子 CancellationToken 树（S5 取消传播原语复用，收齐语义同 §5.3）。
- **文件写认领**（FR-07-05，`backend/claims.rs` 新建）：子代理 FileWrite 前登记 canonical 路径认领（**含祖先目录**）；他者写已认领路径 → E_FILE_CLAIMED 工具结果回喂；子代理终态后认领解除；父级同批冲突检测复用 §5.4 ② 既有机制。
- **汇报与四象限**（FR-07-02/09）：子 run 终态 report/partial/no_report/cancelled 四象限；最后一次 assistant 文本即 `<report>` 结构化汇报 → 以父工具结果回喂 + `subReport{subId, report}` 帧 + `subDone{subId, outcome}` 帧（§3.4）；父级按象限处理：report 继续、partial 注明预算耗尽可续派、no_report/cancelled 注明失败原因。
- **存储与回看**（FR-07-06）：子代理历史独立落 `histories/subs/<parentRunId>/<subId>/`（§5.9；单 JSONL 最小子集，复用 §7.4 原子写）；`load_subagent_history` 命令（§3.3 #59 `{sessionId, subId} → WireMessage[]`）。
- **事件 6 键**（§3.5 M2 表）：`sub:spawn/sub:step/sub:report/sub:usage/sub:done/sub:error`，payload `{sessionId, runId, subId, role?, turn?, usage?, outcome?, code?}`——跨 Tab Inspector 订阅；`util/events.rs` 增键常量。

### 前端（ui/）

- **`$` 触发符**（FR-07-02）：`chat/composer-keys.ts` 增 `detectDollarQuery`（与 detectAtQuery 同构光标锚定）→ 角色菜单（静态 7 角色，↑↓/Tab/Enter/Esc 同 @ 菜单键位）；选中插入 `$角色名 ` 后用户续写任务，发送文本原样入会话（模型经 subagent 工具 description 理解点名语义；Tab 优先级并入 §10.6：触发符菜单按首字符 `/`/`@`/`$` 互斥 > hint）。
- **Inspector 子代理页**（§10.7 M2 五页之一，`chat/Inspector.tsx` 增 tab）：`sub:spawn/done` 事件 + subFrame 驱动列表（角色徽标/subId/状态/步数/用量）；运行中自动跟随展开活跃项；抽屉回看=`load_subagent_history` 拉全量只读渲染。
- **subagent 工具卡**（`chat/ToolCards.tsx` + `chat/toolkit.ts`）：角色徽标 + task 摘要 + 实时步数/状态（subFrame 驱动）+ `<report>` 折叠全文（subReport 后展开可见）；subDone 徽标按四象限着色。
- **store**：`stores/subagents.ts`（zustand 事件驱动：spawn/done 增删、step 计数、usage 累计、抽屉开合态）。
- 契约测试：manifest 增 subStarted/subFrame/subReport/subDone 4 帧、sub:* 6 事件、#58/#59 命令、subagent 工具行。

### 测试

- ScriptedBackend 驱动（§12.5 点名场景）：并发上限 4（第 5 个排队不拒）、预算耗尽 → partial + E_SUB_BUDGET、认领冲突 → E_FILE_CLAIMED、父取消级联（子 runEnded(cancelled) 帧收齐）、四象限各一例、帧序断言（subStarted 先于子帧、subDone 必收尾）。
- 单测：角色表白名单裁剪、认领登记/祖先目录命中/终态释放、budgetSteps 钳制、cleanContext 与父历史注入两形态的消息构造。
- e2e：explore 角色真实委派端到端（fixture 技能式脚本），过程实时可见（subFrame 逐帧）、stop_subagent 干净终止。

## 边界（不在本票）

- 压缩（04 号票）与并行批次执行（07 号票）既有机制直接复用不改动；子代理内再委派的治理参数（展平透传已定，不设深度上限配置）；M3 `wait`/`service` 工具（§5.10）；子代理历史进检索/索引。

## 验收

- [ ] `$explore …` 点名与模型自发 `subagent` 委派均端到端可用，`<report>` 回父上下文
- [ ] 子代理过程在消息流（subFrame）与 Inspector 子代理页实时可见，抽屉可回看历史
- [ ] 第 5 个并发子代理排队；步数预算耗尽得 partial；stop_subagent 与父取消级联干净
- [ ] 跨认领写文件被拒 E_FILE_CLAIMED，子代理终态后认领释放
- [ ] 重启后 `load_subagent_history` 可回看已完成子代理历史
