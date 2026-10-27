# 16. 实现：多会话与项目

Type: task
Status: resolved

## Question

把「多会话与项目」纵切片落地（FR-01-01/02、FR-10-01/02/07）：会话从单例走向一等公民——项目注册表、临时/项目会话双形态、会话 CRUD、workspace 只读命令、多 Tab 前端、每会话独立运行态与历史收养。完成后用户可同时开多个会话（含跨项目）并行运行，重启后 Tab 布局恢复。

范围（Rust）：

1. **项目注册表（FR-10-01，§7.1）**：`storage/project_store.rs`——`~/.craftwave/projects/directories.json`（`{version:1, projects:[{id,name,homeDir}]}`）锁内读写；`<homeDir>/.craftwave/project.json` 注册写入（含 rootsSnapshot）与删除。命令 #10 `list_projects` / #11 `save_project`（新建/更新；homeDir 变更重绑 project.json）/ #12 `delete_project`（不删项目数据目录）。
2. **会话命令**：#13 `create_session`（projectId 可选；默认标题「新会话」）、#16 `delete_session`（运行中拒绝 E_SESSION_RUNNING；histories/prefs/toolres/index 条目全清）、#17 `rename_session`（index 更新）。#15 `load_session` 不单独实现——`list_sessions`（meta）+ `load_session_tree`（树）组合已承担同样职责，§3.3 该行标注口径修订。
3. **workspace 只读命令**：#27 `search_workspace_paths`（BFS 遍历会话 roots，条目数上限防大仓库；大小写不敏感子串匹配；返回相对首根路径 + kind）、#28 `read_workspace_file`（safe_join 校验 + 与 read 工具共用解码器 + 512KB 截断 + range 按行翻页 + 前 8KB NUL 判二进制报错）。
4. **ui_state 域**（#30/#31，从 12 号票余量就近划入）：`AppState.ui_state: ConfigStore<Value>`（ui-state.json Json 原样存取；防抖前端负责）。
5. **会话 roots 泛化**：start_chat 的 `ToolsContext::new(project_root, data_root)` 按会话解析——项目会话 project_root=项目 homeDir，临时会话=数据目录（§6.2 钉死口径）；data_root 恒为数据目录。**M1 简化（本票钉死）**：会话运行时直接按项目表当前 homeDir 解析 roots，不在会话上固化 roots 副本（§9.9 的会话 roots 快照是 M2 外部授权域）。
6. **15 号票 shortcut 收敛**：index.json ≤2000 条归档——upsert 超限时最旧条目追加写入 `sessions/archive.jsonl` 后从 index 移除（历史 JSONL 不动）。崩溃 run 尾条 incomplete 回填**不在本票**（15 号票原文钉死随桌面体验票）。

范围（前端）：

7. **多 Tab（FR-01-01）**：每会话一个 `SessionTab` 组件实例（keep-mounted，非活跃隐藏）——独立 runtime（`createTauriChatAdapter` 工厂化，per sessionId）、独立草稿（DOM 保留天然成立）、独立运行态与滚动锚点；Tab 条（标题 + 运行中指示 + 关闭）；会话persist store（revision/runtimeNodes）与 run-metrics、ask store 改按 sessionId 分桶；ToolCards 恢复按所属会话（SessionContext 注入）。
8. **左栏（§10.7）**：临时会话区 + 项目分组会话树（展开/折叠）、新建按钮（临时会话 / 新建项目（select_dir）/ 项目下新建会话）、标题过滤搜索框（前端本地）、会话重命名/删除、项目删除（其会话转临时——**本票钉死**：projectId 置空、历史保留）。
9. **启动恢复**：ui-state 存 Tab 列表 + 活跃会话 id，启动重建；无存档时自动建一个临时会话。草稿不持久化（shortcut，随桌面体验票）。

不做（记录边界）：@ 路径补全菜单 UI、Composer 细节、三栏布局/标题栏重绘（桌面体验票）；#24/#25 session_prefs（设置域余量票）；#27/#28 的前端消费 UI（产物面板 M2）；会话标题自动生成（M2）。

验收：cargo 三件套 + 前端三项全绿；manifest 双向契约覆盖 9 个新命令；多会话并行 run（mock）互不串帧；重启 Tab 恢复。

## Answer

全部落地，与 Question 范围一致；文档回写 v2.7（§3.3 #15 口径修订 + 各命令实装注记）。

**Rust**（新增/改动）：

1. `storage/project_store.rs`（新）：ProjectMeta + directories.json（Mutex 内读-改-写，原子写）+ `<home>/.craftwave/project.json` 注册清单（rootsSnapshot=[homeDir]）；`save` 校验一目录一项目（homeDir canonicalize 后查重）、homeDir 变更重绑（旧 project.json 删除）；`delete` 只删注册与 project.json。7 例测试。
2. `storage/index_store.rs`：`get`/`mutate`/`remove` 三原语 + **2000 条归档**——upsert 超限时按 updatedAt 升序取溢出条目追加 `sessions/archive.jsonl` 后移出索引（归档 IO 失败降级为保留超限，索引写不因归档抖动失败；行序按时间序稳定写入）。4 例测试。
3. `storage/session_tree.rs`：`delete_session`（会话锁内 index 摘除 + histories/prefs/toolres 磁迹清理，尽力而为；不存在 → Ok(false)）。3 例测试。
4. `host/commands/project.rs`（新）：#10/#11/#12；**delete_project 名下会话转临时**（projectId 置空、历史保留），运行中 run 不受影响（roots 是 start_chat 时的解析快照）。
5. `host/commands/session.rs`（新）：#13（projectId 校验存在；默认标题「新会话」）/ #16（运行中拒绝 E_SESSION_RUNNING——RunManager 查询；不存在 → E_NOT_FOUND）/ #17（trim 非空；updatedAt 推进、revision 不动）。
6. `host/commands/workspace.rs`（新）：#27（BFS 遍历双根、条目预算 20000、基名大小写不敏感子串、跳过 .git/node_modules/target 等；**项目主目录下条目返回相对路径**（read 工具解析基准可直接消费），数据目录专属条目绝对路径）/ #28（PathGuard 校验 + decode_text 共用解码器 + 前 8KB NUL 判二进制 E_INVALID_ARG + range 按行翻页 + 512KB 字符边界截断）。7 例测试（含临时/项目双形态 roots 解析）。`resolve_session_roots` 同供 start_chat 复用。
7. `host/commands/ui_state.rs`（新）：#30/#31——`ConfigStore<Value>`（ui-state.json，`impl ConfigPayload for Value` 无归一化）。
8. `host/commands/chat.rs`：start_chat roots 泛化——`ToolsContext::new(project_root, data_root)` 按会话解析（项目会话=项目主目录、临时=数据目录，§6.2 原口径）；**会话不在运行时固化 roots 副本**（按项目表当前 homeDir 解析，§9.9 会话 roots 快照属 M2 外部授权域）。
9. `lib.rs`：#5 `select_dir` 以 **rfd 0.15** 实装（同步命令主线程执行——macOS NSOpenPanel 线程约束；免 JS 插件与 capability，注释已记）；manifest 26 命令，双向契约全绿。

**前端**：

10. 多 Tab（FR-01-01）：`SessionWorkspace.tsx`——左栏（临时区 + 项目分组树、新建/搜索/重命名/删除、项目删除确认）+ Tab 条（运行指示 + 关闭）+ keep-mounted `SessionTab`（每会话独立 `sessionAdapter(sessionId)` runtime 与 `useLocalRuntime` 实例；非活跃 display:none，帧照收、草稿/滚动锚点 DOM 天然保持）；`RunningReporter`（useAuiState → store）桥 Tab 指示。
11. 分桶：persist store（scopes: Record<sessionId,{revision,runtimeNodes}>）、run-metrics（bySession + `useSessionMetrics` 稳定零值引用）、ask（bySession/hints；ask:closed 载荷无 sessionId 按 askId 全桶定位）；`SessionScopeContext` 注入 ToolCards 边车恢复/AskPanel/ghost/计数条。
12. `stores/sessions.ts`（新）：bootstrap（ui-state `{tabIds,activeId}` 恢复、已删除剔除、空则兜底最新会话或新建临时）、openTab/closeTab（右邻补位）/setActive、refresh 同步 Tab 快照；**落盘 300ms 防抖**（§3.3 #31 防抖前端负责）。6 例测试。
13. `ipc/client.ts` +10 命令函数（9 + selectDir）；`chat/runtime.ts` 增 `sessionAdapter` 工厂（tauriChatModel 单例保留供测试/镜像）。

**验证**：cargo 232 lib + 25 集成全绿（含 manifest 双向）；前端 vitest 99 例、tsc、biome 全绿。多会话并行 run 的帧隔离由 mock 链路 + 分桶测试覆盖；真实 GUI 冒烟待用户 `pnpm tauri dev`。

**已知取舍（shortcut 清单）**：

- 草稿不落盘（keep-mounted DOM 保存，重启丢失）——随桌面体验票按 §10.6 草稿持久化补。
- 新建项目名称取目录 basename，不可即时改名（改 homeDir 重绑可达）——设置页/项目编辑器打磨随桌面体验票。
- #15 load_session 不单独实现（文档 §3.3 已标注口径）：meta=list_sessions（#14）+ 消息=load_session_tree（26b）组合承担。
- delete_session 的磁盘清理尽力而为（index 已摘除，半删残留不阻塞用户操作）。
- @ 补全菜单 UI、Composer 细节、#27/#28 的前端消费（产物面板预览）在桌面体验/M2 票。
