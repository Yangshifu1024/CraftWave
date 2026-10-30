# 实现：AST 命令围栏与写后检查（FR-05-05 P1 + write/edit 完整性自检）

Type: task
Status: resolved

## 要做什么

把 command 围栏从"词法猜"升级为"语法看"：tree-sitter 解析命令、提取真正的写目标（重定向/tee/管道），按四象限判定 Allow/Confirm/Block——引号里的 `>` 不再误拦，变量拼接的写不再漏放，解析失败保守降审批（非 fail-open，改进 CodeWave 已知不足）。同时给 write/edit 加落盘后完整性自检。M1 黑白名单框架（`safety/fence.rs` 规则 1/2）不动。

FR-05-05 原文：P1：tree-sitter Bash/PowerShell AST 写目标分析 + 字面量掩码防误报 + 四象限 symlink 判定；判定 Allow/Confirm/Block；**非 fail-open：AST 失败按保守处理**（改进 CodeWave 已知不足）。
§8.7【M2】AST 管线（本票实现蓝本，逐条落实）：解析失败 → fail-closed 按 Confirm；写目标提取（> >>、tee、sed -i、rm/mv/chmod/chown、install/cp、mkfs、dd of=、管道左侧含写命令保守合并）；字面量掩码（引号外变量在写路径 → Confirm，防 $HOME/.. 逃逸）；四象限（词法在 roots 且 canonical 在 roots→Allow；词法在 roots 但 canonical 逃逸→Confirm；词法不在 roots→Confirm/Block（删除类直接 Block）；无法 canonicalize→Confirm）；git 子命令分类表（status/diff/log/show 只读；push/commit/merge/rebase 写，默认 Block push --force、其余 Confirm）。

## 具体做什么

### 后端（src-tauri/）

1. 依赖：
   - `tree-sitter` + `tree-sitter-bash`（crates.io 现行 0.2x 系；C 源码随 crate 经 cc 编译，注意 MACOSX_DEPLOYMENT_TARGET=13.0）；
   - **PowerShell grammar 选型随票内 spike**：crates.io 若无维护良好的 tree-sitter-powershell，降级路径 = Windows 命令保留现行词法启发（fence v0.1 规则 3 作为 PowerShell 分支），回写 §8.7 注记——不得为凑 grammar 引入无人维护的移植（AGENTS.md 调研纪律）。
2. fence v0.2（`safety/fence.rs` 增 `ast.rs` 子模块；**替换规则 3 的写信号启发**；规则 1 黑名单正则、规则 2 plan 档白名单、规则 4 fail-closed 精神不变；gate.rs 调用点不变）：
   - 解析：tree-sitter bash（Windows 分支视 grammar 调研）；单命令解析设深度/节点数上限防炸；解析失败/超限 → **Confirm**（fail-closed）；
   - 写目标提取：§8.7 清单全量（重定向 > >>、tee、sed -i、rm/mv/chmod/chown、install/cp、mkfs、dd of=）；管道左侧含写命令时保守合并整条判定；
   - 字面量掩码：引号外变量（`$VAR`/`$(...)`）出现在写路径 → 保守 Confirm；
   - 四象限判定：词法在 roots 且 canonical 在 roots → Allow（auto_edit+ 档直写，最终结论仍由 §8.5 矩阵合成）；词法在 roots 但 canonical 逃逸（symlink）→ Confirm；词法不在 roots → Confirm，删除类（rm/mv 到界外）直接 Block；无法 canonicalize（目标不存在且父链异常）→ Confirm；
   - git 子命令分类表：纯函数 `git_classify(argv) -> GitClass { ReadOnly, Write, Destructive }`（status/diff/log/show 只读；push/commit/merge/rebase 写；push --force 归黑名单 Block）——**26 号票接缝**：策略开关消费此函数；
   - **23 号票接缝**：WriteTarget 提取产物（目标路径列表 + 判定）供内部状态保护复用，接口在本票定形。
3. 写后检查（内置完整性校验）：
   - write/edit 经 `util/atomic.rs` 落盘成功后重读 bytes 校验：sha256 与写入内容一致 + 严格 UTF-8 解码成功 +（edit）EOL 风格保持（§8.4 步骤 6 口径，C-10"落盘一律 UTF-8"）；
   - 失败 → 工具结果 ok=false，错误码用 E_INTERNAL（§3.6 通用兜底；若评审要专用码先回写 §3.6），message 含路径与校验阶段；
   - 纯内存比对（重读一次文件），无配置面。
4. **勘误回写**：§8.7 末把 FR-04-06（用户自定义检查命令）标为【M2】，与 map.md"PRD P2 全量"出边冲突——本票不做用户自定义检查命令（P2），随 Answer 把 §8.7 末该段标注修订为【M3】。

- **未知副作用接缝（Q23，经 Q24–Q26 更新）**：AST 可识别禁区写入硬拒；未提取到目标不等于证明只读。向 36 号票提供明确硬拒、可靠只读、灰区待决及依据，区分“必须人工确认”与“可交审计定性的灰区”。审计不豁免硬规则或明确人工确认；36 未完成前灰区保持原保守审批，不能自行放行。

### 前端（ui/）

- 无新 UI。
- 审批挂起载荷的 fenceReason 文案随新判定更具体（如"重定向目标越出工作区（symlink 逃逸）"、"写路径含未解析变量"）；AskPanel 既有 approval 形态直接消费，无需改。
- 工具卡片（command 卡）的确认徽标沿用 `toolResult` 的 ok/error 面，无需感知判定来源是词法还是 AST。

### 实现顺序建议

1. AST 解析与 WriteTarget 提取纯函数（含表驱动单测）→ 2. 四象限判定接入 Fence::judge（bash 先行）→ 3. git_classify 纯函数（26/23 号票依赖，先行交付）→ 4. 写后检查 → 5. PowerShell spike 结论与回写。

### 测试

- Rust：
  - **绕过向量测试集**（票面验收三例必含）：变量拼接写路径（`$HOME/x`）→ Confirm；symlink 穿越界外 → Confirm；heredoc 重定向 → 提取判定；
  - 引号内 `>`/`|` 不误拦（v0.1 已知误报消除）；`tee -a`、`sed -i` 越界、`dd of=/dev/…` Block、`cat x | tee 界外` 保守合并；
  - 畸形命令/超深嵌套 fail-closed Confirm；git_classify 表驱动；
  - 写后检查：正常写、故障注入读回不一致、CRLF 保持、拒绝写二进制；
  - 既有 fence v0.1 测试零回归（黑名单/plan 白名单语义不变）。

## 边界（不在本票）

- 黑名单正则与 plan 档白名单内容调整（M1 语义冻结）；FR-04-06 用户自定义检查命令（P2 → M3）；Windows PowerShell AST（视 grammar 调研降级词法）；E_DELETE_GUARDED 删除守卫（独立机制不动）；web 工具的 SSRF（12 号票）。

## 验收

- [x] 绕过向量测试集（变量拼接写、symlink 穿越界外、heredoc 重定向）全拒或正确降审批——`ast_test::bypass_vectors_reduce_conservatively`：`echo x > $HOME/evil.txt` → Confirm（原因「未解析变量」）、`echo x > link/evil.txt`（link 指向界外）→ Confirm（原因「symlink 逃逸」）、`cat <<EOF > out.txt` heredoc 目标提取成功并按四象限判定（roots 内 Allow / 界外 Confirm）
- [x] 引号内的 `>`/`|` 不再触发 Confirm（v0.1 已知误拦消除）——`fence_test::quoted_pipe_and_redirect_are_not_write_signals`（`grep "a|b" file` @ plan → Allow、`echo "hello > world" | grep x` → Allow）+ `ast_test::quoted_redirect_and_pipe_are_not_targets`
- [x] 构造 AST 解析失败路径，行为为 Confirm（fail-closed 实测），无 fail-open——`ast_test::malformed_and_deeply_nested_fail_closed`（未闭合引号 / 80 层嵌套超限 → Failed）+ `unix_fs_tests::parse_failure_is_fail_closed_not_open`（gate 面挂起且原因「解析失败」）
- [x] write/edit 落盘后完整性自检生效（故障注入产出错误回喂）——`tools/postcheck_test` 6 例（重读不一致 → 哈希阶段、二进制 → UTF-8 阶段、CRLF 裸 LF / LF 引入 CRLF → EOL 阶段、错误构造含路径与阶段）+ `edit_test::group_postcheck_injection_fails_with_internal_error`（合并提交出口注入篡改落盘 → 整组 ok=false 回喂 E_INTERNAL 含阶段）
- [x] Windows 围栏行为不劣化于 v0.1（grammar 降级时词法分支兜底）——spike 结论 PowerShell grammar 可用，**未降级**，Windows 同走 AST：`ast_test::powershell_redirect_and_cmdlets` 9 例覆盖重定向/Set-Content/Remove-Item/Copy-Item/Tee-Object/流合并 2>&1/变量掩码；安装类词法信号跨平台保留。诚实注记：Windows 真机端到端实跑（powershell.exe 实际执行路径）无法在 macOS 开发机验证，判定面由纯内存解析单测保证，真机行为归用户验收面

## 决议记录

1. **PowerShell grammar spike（票面点名调研项）**：`tree-sitter-powershell 0.26.6`（Airbus CERT 维护，crates.io 下载 488 万，2026-10-28 仍发版更新）可用且节点面齐全（pipeline/command/redirection/redirected_file_name）——**不触发降级路径**，Windows 分支同样走 AST；已回写 §8.7 落地注记与变更记录 v2.28。选型组合：`tree-sitter 0.27.1` runtime + `tree-sitter-bash 0.25.1` + `tree-sitter-powershell 0.26.6`（两个 grammar crate 均经 `tree-sitter-language` 提供语言静态量、C 源码随 crate 经 cc 静态编译，零系统依赖；新增下载 5 包）。
2. **勘误回写（票面第 4 点）**：设计文档 §8.7 末段 FR-04-06（用户自定义检查命令）标注已由【M2】修订为【M3】，与 map.md「PRD P2 全量」对齐；M2 实际交付内置写后完整性自检（`tools/postcheck.rs`：重读 → sha256 → 严格 UTF-8 → EOL 保持，失败 E_INTERNAL ok=false 回喂）。落地通道注记：write/edit 的工具面落盘通道是 `tools/write.rs::atomic_write_utf8`（util/atomic.rs 的 `write_atomic` 为存储面通道），校验挂在工具面通道的调用出口（write::run / edit::run / apply_edit_group 合并提交出口三处）。
3. **安装类命令保留词法信号**：npm/pnpm/yarn/pip/make/cargo（build/install）会写工作区文件（node_modules/target 等）但无 shell 级重定向语法，§8.7 AST 写目标清单覆盖不到——保留 v0.1 词法信号（`WRITE_SIGNALS` 正则集语义收窄为安装类）防判定放松，既有测试 `write_signals_require_confirm_outside_plan` 零回归。
4. **fence v0.1 测试的预期语义更新**：`quoted_pipe_is_split_conservatively` 原断言「引号内 `|` 误拦 Confirm」是 v0.1 词法局限的钉子，本票按验收 2 消除该误拦，测试改写为 `quoted_pipe_and_redirect_are_not_write_signals`（断言 Allow）——属票面明文的预期行为变化，非回归；黑名单/plan 白名单/fail-closed 测试全部原样保持绿。
5. **git_classify 只读表扩展**：票面表格列 status/diff/log/show，实现另加 rev-parse/blame/describe/shortlog/ls-files/cat-file/ls-tree/reflog 八个客观只读子命令（26 号票策略开关消费的表完备性；plan 档白名单不受影响，仍只认 status/diff/log/show 四个）；未登记子命令保守归 Write，非 git 输入保守归 Write。
6. **故障注入的诚实边界**：`write::run`/`edit::run` 无落盘闭包注入点，故障注入在两级覆盖——`verify_written` 单元注入（重读不一致/CRLF 破坏/二进制，`tools/postcheck_test`）与 `apply_edit_group` 写闭包注入（篡改落盘 → 整组 E_INTERNAL 回喂，`edit_test`）；真实磁盘级故障时序（坏块/杀毒篡改窗口）不可在单测稳定复现，未做。
7. **cwd 基准的宽松解析**：审批门对 command 的 cwd 只做「相对项目主目录拼接」的基准解析（存在性/归属校验在执行层 command::prepare），解析异常回退项目主目录——不影响拦截方向（cwd 指向界外时由执行层 safe_join 拒绝）。
8. **接缝定形（23/26/36 号票）**：`WriteTarget`/`parse_command`（23 号票内部状态保护消费目标路径列表做禁区命中）、`git_classify`（26 号票策略开关）、`TargetVerdict.reason` 分类文案（36 号票审计区分「必须人工确认」与灰区定性依据——未提取到目标不等于证明只读，灰区不放行，36 未完成前维持保守审批）。
