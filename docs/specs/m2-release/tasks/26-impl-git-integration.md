# 实现：git 集成——只读状态、变更面板与写策略（FR-11-01/02/03）

Type: task
Status: open

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

- [ ] 真实仓库（本仓库即可）变更面板的文件列表/增删统计/diff 与 git CLI 输出一致
- [ ] 默认配置下 AI 执行 git commit 被围栏 Block；白名单开启后 stage/commit 经审批可执行；push 任何配置下都出审批；push --force 恒 Block
- [ ] 非 git 目录打开面板显示空态不报错
- [ ] 契约测试 #37–#39；§3.3 #37/#38、§8.7 git 分类表、§3.2 gitAllowStageCommit、§2.2 模块落位回写完成
