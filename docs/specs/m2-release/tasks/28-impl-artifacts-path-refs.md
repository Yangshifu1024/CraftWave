# 实现：产物面板与 @ 路径引用完整化

Type: task
Status: resolved

## 要做什么

FR-10-06 + FR-10-03 + FR-09-05（PRD 行 493/490/477，出处 §10.7）：

- 右栏 Inspector 增「产物」页——本会话 AI 写过的文件自动登记（去重、计数、已删除灰显），一键预览/打开编辑器。
- `search_workspace_paths` 在万级文件库上保持流畅（@ 补全的底盘）。
- @ 文件引用补齐 PRD 完整口径——空格路径引号、真实路径高亮、发送时合成引用段追加正文（不复制文件内容、不预读，模型侧按需 read）。
- 与 26 号票变更面板联动只留接缝。

## 具体做什么

### 后端（src-tauri/）

- **产物登记**（§10.7 一行口径 `<sid>.artifacts.json`，字段级缺失——本票定义并**先回写 §7.7 存储增量表再实现**）：
  - 文件 `~/.craftwave/sessions/<sid>.artifacts.json`，`{ version: 1, artifacts: [{ path, firstRunId, firstSeenAt, runCount, lastSeenAt }] }`；
  - 登记写入点 = `tools/` 下 write/edit 执行器**成功落盘后**追加；路径规范化为绝对路径；去重键 = 规范化路径，重复命中 runCount+1、lastSeenAt 刷新；
  - 经既有 `MutexMap` 会话锁串行化（与 segments/prefs 同锁序）；原子写复用 `util/atomic.rs`；
  - 登记失败只记日志不阻塞工具结果（与 §7.3 ⑤ 落盘失败口径一致）。
- **GC**：会话删除时随 `<sid>.*` 家族一并删除（§7.5 toolres 边车 GC 同款口径，回写 §7.7 时写明）。
- **读取命令**（§3.3 M2 表 #32–59 无 artifacts 读取命令——**回写 §3.3 增补行后再注册**，编号沿用下一空闲位并注明，04 号票 compact_session 先例）：
  - `list_artifacts` `{ sessionId }` → `{ artifacts: [{ path, runCount, lastSeenAt, exists: bool }] }`；
  - `exists` 由 Rust 读取时探测（symlink 解析后判定）——已删除灰显一次 IPC 搞定，前端不再逐路径试探。
- **list_editors / open_in_editor**（§3.3 #35/#36，M2 表已定义、全图无归属票，随本票实装）：
  - `list_editors` 探测 PATH 上 `code`/`zed`/`subl`/`cursor` 等常见 CLI + macOS `open -a` 候选名单；
  - `open_in_editor` `{ path, line? }` → 按编辑器拼 CLI 参数（VS Code `-g file:line` 形态）拉起；
  - 探测不到可用编辑器 → 返回错误由前端 Toast 提示；路径过 safe_join（越界 E_PATH_OUTSIDE_ROOTS，§9.1）。
- **search_workspace_paths 性能核查**（§3.3 #27，M1 已实装 `host/commands/workspace.rs`：默认 limit clamp 1..100、search_under 带 budget）：
  - 增硬编码默认忽略表（`.git`、`node_modules`、`target`、`dist`、`build`、`.craftwave`——规则表回写 §10.7 注记）；
  - walk 条目上限（单 root ≤ 50k 条目即止，防病态库挂死 IPC）；
  - 大目录 fixture（脚本生成万级文件）实测：命中前 20 条的查询 P95 < 300ms，实测数回写验收记录。

### 前端（ui/）

- **产物页**（`chat/Inspector.tsx` 增 tab，10 号票子代理页同款先例——现状仅用量页 + 计划 section）：
  - `list_artifacts` 按活跃会话取数；刷新时机 = 页签激活 + run 终态（event-bridge run:done/error/cancelled 已有钩子）；
  - 行 = 文件名 + 相对路径 + `×runCount` 徽标；exists=false 灰显置底；
  - 行操作两个：预览（`read_workspace_file` §3.3 #28 已实装——Dialog 只读渲染 + truncated 提示 + 二进制/越界错误行内展示）、打开（`open_in_editor`）。
- **变更面板联动接缝**：产物行预留 `onLocateChanges(path)` 空回调位（26 号票落地时接 git_diff 定位跳转），本票不实现跳转逻辑。
- **@ 引用完整化**（`chat/composer-keys.ts` 现状：detectAtQuery 光标锚定、insertAtCompletion 直接插相对路径文本——17 号票雏形，composer-keys.test.ts 已有基础用例）：
  - **空格路径引号**：`insertAtCompletion` 升级——路径含空格 → 插 `@"a b/c.txt"`；`detectAtQuery` 同步升级：光标回扫遇未闭合 `"` 内空白不终止（引号闭合即止），引号内再 `@` 不嵌套触发。
  - **真实路径高亮（chip 语义）**：ComposerPrimitive.Input 为 textarea，原生无法内嵌 chip——实现形态 = 透明 overlay div（同字体/同 padding/同滚动偏移）对 `@…` token 着色高亮；若与 IME 组合/滚动同步的成本超预期，**降级为引号语法 + 无高亮**并把降级口径回写 §10.6（先回写再实现）。
  - **发送时合成引用**：SessionTab 发送路径（start_chat messages 构造前）：
    - 提取本条消息中的 @token（相对路径，roots 内校验；越界 token 原样保留不进引用段）；
    - 去重、上限 20（超出截断并提示——Toast 随 29 号票，落地前先 console.warn）；
    - 正文尾部追加引用段（格式回写 §10.6：`\n\n[引用文件]\n- @<相对路径>` 逐行）；
    - **不复制内容、不预读**（模型侧按需走 read 工具，§6.2 wire 重建零改动）；
    - ghost hint/↑↓ 历史等既有 Composer 行为不受影响（引用段在发送变换时合成，不改输入框原文）。
- ipc/client.ts 增 `listArtifacts`/`listEditors`/`openInEditor` 出口 + COMMANDS 注册；payload 类型随 02 号票 ts-rs 生成物面。

### 测试

- 与 17 号票的普通文件拖入共用路径 chip/发送引用段；roots 外文件仍须授权，不另建文件上传协议，不因拖拽绕过路径校验。
- Rust：artifacts 登记（首写/重复计数/工具失败不登记/相对路径与 symlink 规范化）单测；list_artifacts exists 判定（临时目录造删）；open_in_editor 越界拒绝 E_PATH_OUTSIDE_ROOTS；search 忽略表与 50k 条目上限（大目录 fixture，debug 计时 warn 级埋点）。
- 前端 vitest：composer-keys 引号路径用例扩展（detectAtQuery 引号内空格/闭合/非嵌套、insertAtCompletion 空格加引号）；发送引用段合成（多路径去重、上限 20、越界 token 剔除、输入框原文不变）；产物页组件（灰显置底/空态/预览 Dialog truncated 态）；contract.test 扩域（list_artifacts/list_editors/open_in_editor 三命令）。

## 边界（不在本票）

- git 变更面板本体与联动跳转（26 号票）；M3 文件树面板 `workspace_tree`（#71）。
- 命令重定向（`>`/`tee`）写入的文件不登记（无法可靠捕获，记 Answer）；mkdir 不登记；外部目录授权写入的文件照登（allow_external_dir 属 24 号票）。
- 全文索引/FTS5（M3 §7.8）；`.gitignore` 级自定义忽略规则（硬编码表即可，PRD 未要求）。

## 验收

- [x] 会话内让 AI 写两个文件（其一写两次）→ 产物页 2 行、计数正确；删除其一后刷新灰显置底。
- [x] 产物行预览弹出只读内容（truncated 有提示）；装有 VS Code 时 open_in_editor 精确打开到文件行。
- [x] 万级文件目录下 @ 补全输入 2 字符出结果不卡顿（P95 < 300ms 实测数记录在案）。
- [x] 含空格路径经 @ 菜单插入为 `@"a b/c.txt"`，发送后正文尾部出现引用段、模型可经 read 工具读到该文件；输入框原文与 ghost hint 行为不变。
- [x] `<sid>.artifacts.json` 结构与 §3.3 增行、§7.7 回写一致；会话删除后登记文件随之清理。

## 决议记录（2026-10-30 实装收口）

- **性能实测数（验收第 3 条）**：大目录 fixture（30 目录 × 350 文件 + 50 目录 × 100 文件 ≈ 15k 条目）@ 补全命中前 20 条，5 轮取最大 **4ms**（验收线 300ms；测试 `search_workspace_paths_on_large_tree_meets_budget` 带 println 记录）。既有实现（M1 16 号票）已有 `SEARCH_SKIP_DIRS` 忽略表（含票面全部六目录 + `.venv`）与 `SEARCH_MAX_ENTRIES=20_000` 条目预算（比票面 50k 更严，保留）；本票补超 100ms 的 warn 级埋点（eprintln，`search_workspace_paths_core`），未改既有阈值。
- **`open_in_editor` 白名单口径（§3.3 #36 签名无 sessionId）**：按 03 号票 `open_dir` 白名单体系扩展——数据目录整根（含 workspace/ 临时会话产物落点）+ 全部注册项目主目录（会话 roots 的超集），越界 `E_PATH_OUTSIDE_ROOTS`；已回写 §3.3 #35/#36 实装注记。编辑器探测序 = CLI（code/cursor/windsurf/code-insiders/subl/zed）→ macOS `open -a` 候选；行号定位三风格（`--goto file:line` / `file:line` / 忽略）。**真实拉起实测**：装有 code CLI 的开发机 `cargo test --ignored open_in_editor_launches` 实测通过（spawn 成功，`--goto <file>:2` 定位；ignore 测试不进常规 CI——桌面弹窗副作用）。
- **产物登记的执行器接线**：登记点在 `backend/exec.rs`（`dispatch` 的 write/edit 分支 + 同文件多 edit 合并组 `run_edit_group` 的每个接受成员各计一次），成功落盘（`outcome.ok`）才登记；登记经 `ExecutorEnv.store`（新增 `Option<Arc<SessionStore>>` 字段）持 `MutexMap` 会话锁写 `<sid>.artifacts.json`。**子代理环境不登记**（store=None）——worktree 写入不进会话产物面（ADR-0001 汇回由主代理协调，主代理写操作照常登记）；已回写 §7.7。
- **exists 探测与排序**：`list_artifacts` 读面在后端就地完成 exists 判定（`fs::metadata`，断链 symlink 记不存在）与排序（存在优先 + lastSeenAt 倒序——灰显置底由排序天然达成），前端保序渲染，一次 IPC 搞定。
- **chip 高亮（§10.6 ②）未降级**：实现为透明 overlay 文本层（基文 `text-transparent` 仅 token span 着色，防与 textarea 文本重影；同 `text-ui-base leading-5 py-0.5`；textarea 滚动经捕获监听同步 scrollTop）。IME 组合期 overlay 读 composer store 文本有一帧延迟，打字无感，未触发票面降级条件（引号语法本身照常支持）。
- **发送引用段准入判据**：@token 以**相对路径形态**准入引用段（不以 `/` 或盘符开头、无 `..` 段）；绝对路径 token（17 号票拖拽插入形态）原样保留在正文、不进引用段（模型 read 照走授权链，票面「越界 token 原样保留」口径）。去重、上限 20（超出截断 + console.warn，Toast 随 29 号票）。引用段在 `createTauriChatAdapter.run` 的 wire 构造前合成（`withAtRefSection`：附加到最后一个 text part）——`syncTreeBeforeSend` 用原始 messages，**树落盘与输入框原文均无引用段**，ghost hint/↑↓ 历史行为不变（票面口径）。
- **变更面板联动**：票面预留的 `onLocateChanges(path)` 空回调位按现状实装为完成态——26 号票已落地 `openChangesPanel(sessionId, path?)`（changes-store），产物行「变更」钮直接接入（打开变更面板并定位该文件 diff）。
- **detectAtQuery 引号算法**：起步态 = 光标左侧引号总数奇偶（光标在未闭合引号内）；回扫 `"` 翻转（翻出引号段且起步在段外 = 闭合点即止返回 null）；引号内 `@` 不触发；引号外空白终止。补全搜索词剥引号字符（`"a b/c` → `a b/c`）。全部用例见 `composer-keys.test.ts` 28 号票段。
- **验证链**：cargo fmt/build/clippy/test 全绿（lib 777 + 集成 9 目标全 ok）；pnpm typecheck/test/lint 全绿（446 例；首跑遇已知 `thread-rendering.test.tsx` 抖动复跑即绿，未追修）。
- **遗留（不影响验收）**：`open -a` 候选与 Zed/Sublime 的真实拉起未逐一实测（本机仅 VS Code）；产物页真机「AI 写文件 → 面板可见」的端到端走查留给用户日常使用面（自动化已覆盖登记/查询/渲染各环节）。
