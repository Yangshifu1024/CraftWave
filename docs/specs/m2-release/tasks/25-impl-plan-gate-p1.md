# 实现：计划门 P1——批准基线、范围门与提醒（FR-06-02/03/04）

Type: task
Status: resolved

## 要做什么

让计划门从"过期拦截"长成完整执行纪律：计划一经用户批准即冻结基线，执行跑偏（写计划外的新路径）触发范围门询问（允许/本会话始终允许/拒绝），磨蹭不更新计划会被注入提醒；CodeWave 式硬门保留为设置开关、默认关。M1 计划门基础（gate.rs 步骤 3 的 E_PLAN_STALE、approve_plan 冻结快照）不动。

FR 依据（原文摘引）：
- FR-06-02（评审定稿）：默认"有计划即守门"——无计划时不阻写（写操作仍受权限档 diff 确认保护）；计划一经用户批准，过期检查（E_PLAN_STALE：计划全部完成后继续写须先更新计划）与范围门即生效；严格硬门（空计划即拒 E_PLAN_REQUIRED）保留为可选开关，**默认关闭**
- FR-06-03：用户批准计划后冻结基线；执行中范围扩张触发范围门询问（允许/本会话始终允许/拒绝）
- FR-06-04：根会话每 N 次变更工具未更新计划则注入提醒——附录 B 定 **N=8**："批准基线 diff（新增写路径 ∉ 基线 → ask 允许/始终/拒绝）；每 8 次变更未更计划注入提醒"

## 具体做什么

### 后端（src-tauri/）

1. 基线与范围门（FR-06-03 + 附录 B 口径）：
   - `tools/plan.rs` PlanState 增 `baselinePaths`（用户批准的具体文件/目录范围，canonical 口径；目录范围含其子路径，不能把文件范围自动扩成顶层目录）；
   - **批准范围（2026-10-29 用户确认 Q22）**：批准计划时展示并冻结具体文件或目录范围；没有列范围时首次写入也须确认。新增范围再次询问，不由模型首写路径自动确定授权；文件范围不扩成整个顶层目录。实现前回写 §8.6/§3.2 的范围字段、显示及更新契约。
   - 判定位置：gate.rs 计划门步骤旁（FileWrite 直接取目标路径；command 的写目标走 **22 号票接缝**——AST WriteTarget 提取产物同判；22 未落地时 FileWrite 先行、command 侧随 22 收敛）；
   - 触发 → 挂起审批（AskPayload approval 形态：展示计划外路径与计划条目对照），选项 = 允许（allow_once）/ 本会话始终允许（always_session——用户明确同意的文件/目录范围入基线，会话级）/ 拒绝；
   - 与 E_PLAN_STALE 共存：先判过期（全部 completed 仍写，M1 逻辑）再判范围。
2. 批准基线冻结点：`approve_plan`（#26c 既有）冻结快照时同时冻结用户看到并批准的文件/目录范围，空范围不豁免首次写入。
3. 计划提醒（FR-06-04，N=8）：
   - 根会话每 8 次变更工具（FileWrite/Network 执行）未伴随 plan:update → 注入提醒；
   - 计数器挂 RunHandle（跨审批挂起延续，DriveState 同款机制）；子会话不触发（"根会话"口径）；
   - **注入形态回写点**：设计文档未定义——本票拟定 = 下一轮请求的 system 侧附加一次性提醒文本（backend 请求组装处拼装；plan:update 后计数清零；不产生工具结果帧、不阻塞），实现前回写 §8.6。
4. 硬门开关（FR-06-02）：
   - 配置增 `requirePlan: bool`（默认 false；**字段级缺失，回写 §3.2** approval 或计划域）；
   - gate.rs 判定顺序：硬门（开关开且无已批准计划 → E_PLAN_REQUIRED 硬拦回喂，§3.6 码已有）→ E_PLAN_STALE → 范围门 → 矩阵。

### 前端（ui/）

1. 计划呈现：消息内计划卡（M1 既有）展示基线态标记（已批准/基线条目数）和待批准的具体文件/目录范围；范围门询问走 AskPanel 既有 approval 形态（无需新卡）；
2. §10.7 M2 的 Inspector"计划"常驻页随五页票落地，本票只保证 plan:update 事件与批准按钮数据面兼容（M1 已有）；
3. SecurityPage：硬门开关（"严格计划模式：无批准计划时禁止写入"，默认关）。

### 测试

- Rust：
  - 场景链路：批准具体文件/目录 → 写范围外路径 → 范围门询问 → 三选项各自路径（allow_once 单次放行 / always_session 明确批准范围入基线后范围内免问 / 拒绝回喂）；
  - E_PLAN_STALE 优先于范围门；硬门默认关、开启后空计划写拦 E_PLAN_REQUIRED；
  - 提醒计数：第 8 次变更后下一请求携带提醒、plan:update 清零、跨挂起延续、子会话不触发；
  - 边界情况：无计划 + 开关关 = 写不阻（FR-06-02 默认策略锚定）；明确批准的目录内子路径不触发范围门；仅批准 src/a.rs 后写 src/b.rs 仍询问；已批准计划无范围时首写询问。
- 前端：审批卡渲染与载荷、开关接线、契约（无新命令；PlanState/AskPayload 键面增量同步）。
- 回归：M1 既有 plan 门测试（E_PLAN_STALE/approve_plan/plan:update）零行为变化（开关默认关、无已批准计划时范围门不触发；已批准但范围为空时首次写仍询问）。

## 边界（不在本票）

- M1 plan 工具语义/批准按钮/E_PLAN_STALE 基础（不动）；子代理的计划边界（§5.9，10 号票）；计划模板/自动生成（无此需求）；Inspector 计划常驻页本体（§10.7 五页票）。

## 验收

- [x] 批准计划时可见具体文件/目录范围；无范围时首写询问，范围外路径询问；"本会话始终允许"后仅在明确同意范围内免问
- [x] 计划全部完成后继续写 → E_PLAN_STALE（M1 行为零回归）
- [x] 硬门开关默认关；开启后无批准计划的写被 E_PLAN_REQUIRED 拒
- [x] 连续 8 次变更工具不更新计划 → 下一请求携带提醒；更新计划后计数清零
- [x] §8.6（基线语义、提醒注入形态、N=8）、§3.2（requirePlan、baselinePaths）回写完成

## 决议记录

- **范围声明的数据来源（Q22 落地形态）**：plan 工具参数增可选 `paths: string[]`（§8.3 回写）——模型在计划里声明要写的具体文件/目录，计划卡与 Inspector 展示；`approve_plan`（#26c）冻结为 `PlanState.baselinePaths`（批准时刻的快照副本）。set 全量覆盖（未带 = 清空声明）、update 非空才覆盖；批准后模型改 paths 不影响已冻结基线（只随用户重新批准更新）；≤64 条、单条非空校验（E_INVALID_ARG）。
- **范围门判定面**：基线条目与写目标统一归一（相对路径按项目主目录展开 + `canonicalize_best_effort`），"在基线内" = 目标等于条目（文件精确匹配）或以条目为前缀（目录含子路径）——文件范围绝不自动扩成顶层目录；基线为空时任何写目标命中（首写询问）。FileWrite 取 `path` 参数；command 走 22 号票 AST `WriteTarget` 同判（`TargetPath::Opaque` 值未知不猜，交围栏字面量掩码保守 Confirm，不进范围门）。未装配 workspace（headless 缺省）不判。
- **范围门与围栏 Confirm 的顺序裁定**：范围门挂起优先于围栏 Confirm 挂起（否则 command 写目标先弹围栏卡、恢复重放按签名放行会让范围门对 command 形同虚设）；围栏 Block（灾难级）仍先于一切挂起。范围门命中时待决的围栏 Confirm 原因一并附在范围卡 `fenceReason`（一次交互覆盖本次调用全部审批，恢复重放按签名放行）。计划三门内部顺序固定：硬门 → E_PLAN_STALE → 范围门（票面口径）。
- **always_session 的授权粒度**：把计划外写目标的**父目录**以目录范围（含子路径）并入会话基线扩展（与 24 号票外部目录授权的"该目录及子目录"粒度一致），询问卡选项文案明示「写入所在目录」；run 内经 `grant_plan_scope` 热追加（重放放行），跨 run 经会话桶 `PlanScopeMemory`（AppState 持有，进程内单例不持久化——退出即清、会话删除随清、备份恢复失效接线留 33 号票，同 `ApprovalMemory`/`SessionTrustSet` 口径）。
- **范围卡不设推荐项**：三选项（allow_once/always_session/deny）均非 recommended——不参与 autoConfirm 自动批准（计划范围须用户明确同意，与外部目录授权/项目信任同精神；否则 autoConfirm 用户会在无人查看时自动放行范围外写入，架空基线）。allow_once 不扩大基线（同签名免批照旧）。
- **批准基线的作用域**：与 M1 E_PLAN_STALE 一致为 run 级（PlanState 挂 RunHandle，run 结束即失）——跨 run 的免问靠 always_session 会话桶；这与既有批准快照/E_PLAN_STALE 的生命周期同构，未做会话级持久化（用户重新批准是廉价动作，且 26 号票 git 面与审计面按 run 装配读取批准范围）。
- **计划提醒注入形态**：与 §5.7 看门狗纠偏同机制——下一拍 CallModel 前 `history.push(System{..})`（hidden system，请求面生效不入史册、不产生工具结果帧、不阻塞），一次性 take；计数挂 `DriveState`（`plan_change_count` + `plan_reminder_enabled`，根会话装配 true、子代理缺省 false——"根会话"口径），跨审批挂起经 resume 链天然延续；plan 工具成功更新即清零（同批伴随清零），Blocked（未执行）与 Cancelled 不计数（只认真正进批次执行的 Completed）。提醒文本固定中文话术（「计划纪律提醒：已连续执行 8 次变更类工具而未更新计划……」）。
- **无挂起竞态丢提醒**：提醒置位发生在工具批收口（CallTools 内），下一拍 CallModel 即注入——两拍之间不存在挂起点（挂起只发生在门控预判），无丢失路径；取消/终态时未注入的提醒随 run 结束废弃。
- **plan:update 全量态**：事件载荷扩为 `{sessionId, items, paths, approved, baselinePaths}`（plan 工具执行发、`approve_plan` 冻结后同键补发一次）——复用既有事件键（无新键），前端 plan-state store 扩 `SessionPlanState`，PlanCard/Inspector 同源展示基线标记与范围。批准按钮落在 Inspector 计划区块（`inspector-plan-approve`，M1 `approvePlan` IPC 面接通 UI；未批准时展示，批准后替换为基线徽标）——§10.7 常驻页本体仍归五页票。
- **未做项（诚实记录）**：① 提醒计数对 Blocked/Cancelled 不计的直驱用例未单独成测（拼装逻辑 `executed_positions` 排除直白，由场景 A 阈值用例间接覆盖达阈值路径）；② command 写目标的 always_session 也按父目录粒度（与文件写一致）——删除类命令（rm x.js）授权后同目录其他文件免问，用户可在询问卡看到「写入所在目录」文案后选择拒绝；③ 真实多轮 mock（plan 轮 → write 范围外轮）受 MockOpenAI 双轮剧本结构限制，范围门全链以直驱 e2e（`e2e_plan_scope_suspend_resume_allow_once`）+ 门判定单测 + 载荷单测三段覆盖，start_chat 全链的宿主恢复循环（grant_plan_scope 调用）无独立 e2e——该调用为两行直排逻辑（归一 + parent + grant），risk 低，留 26/36 号票接线时顺带回归。

## 检查链（2026-10-30 全绿）

`cargo fmt --check` / `cargo build` / `cargo clippy --all-targets`（0 警告）/ `cargo test`（lib 726 + 集成 9 目标全过）/ `pnpm typecheck` / `pnpm test`（38 文件 378 用例）/ `pnpm lint`（biome 0 错误）。MACOSX_DEPLOYMENT_TARGET=13.0。已知抖动（thread-rendering 偶发、技能注册表单例时序）本轮未出现。
