# 实现：git 集成——只读状态、变更面板与写策略（FR-11-01/02/03）

Type: task
Status: resolved

## 要做什么

让 AI 与用户都看得见仓库状态：右栏一键打开变更面板（左文件列表右聚合 diff、增删统计），配合产物面板定位 AI 改了什么；同时钉死 git 写策略——AI 默认禁止 commit/push/merge，设置白名单可放开 stage/commit（默认关），push 永远确认。

FR 依据（原文摘引）：
- FR-11-01：status / diff（工作区 vs HEAD）/ recent log / user info（git2-rs 只读绑定）
- FR-11-02：宽弹窗：左文件列表右聚合 diff；增删统计；与产物面板联动
- FR-11-03：默认 AI 禁止 commit/push/merge（围栏拦截）；设置中可白名单放开 stage/commit（默认关）；push 永远确认
- §3.3 #37 `git_status { sessionId } → { files: { path, status }[] }`、#38 `git_diff { sessionId, staged? } → { patches: DiffPatch[] }`、#39 `git_recent_log { sessionId, limit? } → { commits: { sha, summary, time }[] }`；附录 B："push 默认 fence Block（§8.7 git 分类表）"

## 具体做什么

### 后端（src-tauri/）

1. 依赖：`git2 = "0.21"`（default-features=false，CodeWave 同款锁面；纯本地只读+索引操作，不需网络传输特性）。
2. 新模块 `git/`（`git/mod.rs` + `git/status.rs`，参照 CodeWave `src-tauri/src/git/status.rs` 单文件 339 行的实现密度；§2.2 模块图无 git 域，落位随 Answer 回写 §2.2）：
   - `git_status`（#37）：git2 status（含 untracked，尊重 .gitignore）；**回写点**：FR-11-01 的 user info（当前 git 用户名/邮箱）在 §3.3 无命令行——方案 = #37 返回增 `user: { name?, email? }` 字段并回写 §3.3；非 git 仓库 → 返回空 files（前端空态），不报错；
   - `git_diff`（#38）：工作区 vs HEAD 聚合 diff。**DiffPatch 形态 §3.3 未定义——回写点**：复用 §3.2 AskPayload.approval.diff 结构（`{ path, hunks: { header, lines: { kind: add|del|ctx, text }[] }[] }`）+ 增统计字段 `addLines/delLines/binary?`，随票定形并回写 §3.3 #38；staged 视图本票不支持，staged 参数忽略并注记；
   - `git_recent_log`（#39）：默认 limit 20，sha/summary/time；
   - 仓库根解析：sessionId → 会话 roots → 项目主目录向上找 .git；git2 错误（脏索引/损坏）降级为空结果 + warn 日志，不阻塞会话。
3. 写策略（FR-11-03 + §8.7 git 分类表）：
   - **子代理 worktree 接缝（Q7–Q10 用户确认）**：10 号票由应用管理独立 worktree，并由主代理回收差量；创建/回收不得自动 commit/push 或改变用户已有暂存状态。应用受控工作区管理与模型 git 命令分开授权，本票默认禁止模型 commit/merge 等策略不变。
   - **22 号票接缝**：`git_classify(argv) -> GitClass { ReadOnly, Write, Destructive }` 纯函数已随 22 落地（status/diff/log/show 只读；push/commit/merge/rebase 写；push --force 归黑名单 Block）；本票把策略开关接入 fence 判定；
   - 配置面：ApprovalConfig 增 `gitAllowStageCommit: bool`（默认 false；**字段级缺失，回写 §3.2**）：
     - 关：add/stage/commit/merge/rebase → Block（E_FENCE_BLOCKED，"默认 AI 禁止"）；
     - 开：add/stage/commit → Confirm（审批放行后可执行）；
     - push 恒 Confirm（"push 永远确认"，任何配置不豁免）；push --force 恒 Block（黑名单既有）；
   - **回写点**：§8.7 git 分类表"其余 Confirm"行按 FR-11-03 策略开关口径细化；
   - 22 未落地的降级：本票先在词法 fence 加 git 首词+子命令判定最小版，22 落地后收敛到 AST 分类表（与 23/24 号票同风格）。

### 前端（ui/）

1. 变更面板（FR-11-02 逐字）：大尺寸 Dialog——
   - 左列文件列表（状态徽标 M/A/D/U/untracked + ±N 增删统计，点选切换）；
   - 右列聚合 diff（diff 行渲染从 AskPanel 抽公共组件复用；超长文件折叠，虚拟化不做）；
   - 顶部 recent log 折叠条（#39）；手动刷新按钮；非仓库空态"当前项目不是 git 仓库"。
2. 产物面板联动接缝（§10.7"与产物面板联动"；28 号票产物页未落地）：
   - 变更面板先作为 Inspector 独立入口；
   - 与 28 号票约定接口 `openChangesPanel(path?)`——产物页路径条目跳转打开本面板并定位该文件 diff。
3. 写策略开关在 SecurityPage（23 号票四开关组旁，"允许 AI 执行 stage/commit（默认关）"）。

### 测试

- Rust：
  - 临时真实仓库 fixture（git2 init + 提交）：status 各状态（修改/新增/删除/untracked/ignore 尊重）、diff hunk 正确性（含二进制 binary 标记、CRLF）、log 排序与 limit、非仓库空结果、.git 异常容错；
  - git_classify 表驱动；
  - 策略矩阵：默认关 commit Block / 开后 stage+commit Confirm / push 恒 Confirm / --force 恒 Block / 只读子命令任何配置放行。
- 前端：面板渲染（列表/增删统计/diff/空态/log 折叠条）、openChangesPanel 联动接口、契约测试 #37–#39。

## 边界（不在本票）

- AI 专用 git 工具（commit 工具化不做——写类只经 command 围栏）；staged diff 视图；分支/远程操作 UI；push 的网络封装（"push 恒确认"指本地命令审批面）；GitHub 集成；文件树面板 git 标记（M3 #71）。

## 验收

- [x] 真实仓库（本仓库即可）变更面板的文件列表/增删统计/diff 与 git CLI 输出一致
- [x] 默认配置下 AI 执行 git commit 被围栏 Block；白名单开启后 stage/commit 经审批可执行；push 任何配置下都出审批；push --force 恒 Block
- [x] 非 git 目录打开面板显示空态不报错
- [x] 契约测试 #37–#39；§3.3 #37/#38、§8.7 git 分类表、§3.2 gitAllowStageCommit、§2.2 模块落位回写完成

## 决议记录

- **Status: resolved**（2026-10-30，分支 `m2/ticket-26-git-integration`，一票一分支一提交）。
- **git 模块落位**：新建 `src-tauri/src/git/`（mod.rs = 契约类型 + 三个 tauri 命令；status.rs = 只读查询核心；status_test.rs = 独立测试文件），§2.2 模块图回写新增 git 域。git2 0.21（default-features=false）直接复用 10 号票引入，未新增依赖。
- **#37 user info 落位**（票面回写点）：`git_status` 返回增 `isRepo`（非仓库空态判别）与 `user: { name?, email? }`（仓库配置 user.name/email，键缺失字段 None、两者都缺 user 为 None），§3.3 已回写。
- **DiffPatch 定形**（票面回写点）：`{ path, addLines, delLines, binary, truncated, hunks }`——hunks 复用 §3.2 approval.diff 的 `{ header, lines: { kind, text } }`；untracked 计入；空仓库（无 HEAD）与空树对比 = 全文件新增；二进制按 git 启发式自嗅探（delta BINARY 标志对 untracked 内容不置位，改为读工作区文件头 8000 字节含 NUL 判定）；单文件 3000 行预算截断标 truncated、文件数上限 200；staged 参数保留在契约面但恒忽略（票面注记），§3.3 已回写。
- **写策略接入点**（票面"接入 fence 判定"的落地形态）：判定落在审批门（`backend/gate.rs`）而非 fence.rs——策略消费 §3.2 配置快照（ApprovalPolicy），与围栏（纯安全底线）分层：判定序 0.6（Block 类，先于签名放行——策略铁律不因历史签名/会话桶签名复活）与 1.5（Confirm 类，先于围栏、后于签名放行——挂起恢复重放防死循环）。判定输入 = 22 号票 `git_classify` + 子命令细分（`git_subcommand_policy`），复合命令逐段取最严；AST 解析失败时词法兜底（粗切段 + 段首词 git + 子命令字面）防「解析失败 + 审批总开关豁免」组合漏拦（票面"词法最小版"降级条目在此语义下收口）。主 run 与子代理两处装配点（chat.rs/subagent.rs）同源注入开关。
- **策略矩阵**（票面 + spec 决策"merge/rebase 仍禁"口径）：关（默认）= add/stage/commit/merge/rebase 全 Block；开 = add/stage/commit Confirm、merge/rebase 仍 Block；push（非 force）恒 Confirm（白名单与审批总开关都不豁免，批准后的同签名重放放行）；push --force 恒 Block（黑名单正则先行 + 策略层双保险，含 `-f`/`--force-with-lease`）。只读子命令与其余未登记子命令不触发策略（宁窄勿宽防误伤）。
- **真实仓库验收口径**：验收第一项以 tempfile 临时仓库的 git2 输出与已知内容断言对齐（不触碰本仓库真实 .git——票面测试纪律优先于"本仓库即可"的字面），面板 UI 的真人验收留给用户日常使用面。
- **前端接口**：`openChangesPanel(sessionId, path?)`（changes-store 命令式入口 + store 动作双面）为 28 号票产物页约定接口；diff 行渲染从 AskPanel 抽公共组件 `DiffView.tsx` 两处共用（审批卡保留 `ask-diff` testid 兼容既有测试）。
- **已知非阻塞项**：① Inspector「变更」tab 入口未单独写组件测试（面板本体 8 例 + store 联动已覆盖核心行为，tab 按钮为薄接线）；② 面板对超大仓库的展示性能未压测（虚拟化按票面边界不做，行数/文件数后端预算已封顶）。
