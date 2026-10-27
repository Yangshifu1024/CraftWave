# 17. 实现：桌面体验

Type: task
Status: resolved

## Question

把「桌面体验」纵切片落地（FR-13 P0 域 + FR-01-10 折叠分组 + §10.6 Composer 细节 + 15/16 号票 shortcut 收敛）：宿主侧从「聊天窗口」补齐为「常驻桌面应用」（托盘/后台运行/系统通知/单实例/退出拦截），前端从「骨架窗」补齐为「三栏成品布局」（主题三态/右栏 Inspector/折叠分组/Composer 全套交互/滚动行为）。完成后应用可关闭到托盘后台跑长任务、通知直达会话、主题随系统、run 过程折叠总结。

范围（Rust 宿主侧）：

1. **单实例（FR-13-10，§2.1）**：tauri-plugin-single-instance——二次启动唤起既有主窗（show + set_focus），不起新进程。
2. **托盘与后台运行（FR-13-06/07，§9.6）**：tauri `tray-icon` feature——托盘菜单（显示主窗/退出）；主窗 close 请求拦截 → 隐藏（关闭到托盘默认开）；左键单击唤起。
3. **退出拦截（§9.6）**：退出请求（托盘退出/⌘Q 菜单退出）存在活跃 run → emit `app:exit_requested` → 前端确认 → `resolve_exit_request(ack: bool)` 命令放行或取消；无活跃 run 直接退出。
4. **系统通知（§9.6）**：tauri-plugin-notification——触发点：run 终态（窗口失焦/隐藏时）+ ask:opened；payload 锚点（sessionId/runId/anchor）；点击 → `notify:activate` 事件 → 前端路由到对应会话；防打扰：窗口聚焦且活跃会话即触发会话时不弹。设置项（通知总开关/仅人工介入）挂 config 域读取。
5. **崩溃 run 尾条 incomplete 回填（15 号票 shortcut）**：崩溃恢复读时修复时，该 run 尾部 assistant 条目 status 回填 `{type:"incomplete", reason:"error"}`（当前只补 marker 行）。

范围（前端）：

6. **主题三态落地（FR-13-03）**：设置页已有 theme 选择（config.ui.theme）；本票接线到 DOM——theme → `documentElement.classList`（dark/跟随系统 matchMedia），应用启动即生效；主界面（SessionWorkspace/Thread/工具卡/AskPanel）硬编码 neutral 色迁移到语义令牌（styles.css 已有亮/暗两套）；强调色中性化（替换 sky 蓝系）。
7. **三栏布局（FR-13-01）**：右栏 Inspector（M1 仅「用量」页：三数明细 + 会话累计 + 上下文进度条，§10.7），可折叠（开合入 ui-state）。
8. **折叠分组与总结（§10.4 / FR-01-10）**：assistant 消息内分层——过程 parts（reasoning/工具卡）运行中全量展开；runEnded 后折叠为摘要行 `⚙ N 次工具调用 · 时长`（最终答复 text 常显折叠区外）；点击摘要行展开/收起；历史加载默认收起；自动折叠开关（ui-state，默认开）。运行态实现形态钉死：一 run = 一条 assistant 消息（FrameBuffer 累积），折叠落在消息组件内部，非跨消息分组。
9. **Composer 全套（§10.6 / FR-13-02/05）**：↑↓ 空输入翻阅本人历史消息；Esc 停止运行/关浮层；Shift+Tab 循环权限档（档位仅内存态，重启回默认；Tab 条/Composer 显示当前档）；`@` 路径菜单（search_workspace_paths 驱动，光标锚定，↑↓ 导航，Tab 优先级：菜单 > ghost hint）；草稿按 Tab 防抖存 ui-state（重启恢复）。Enter 发送/Shift+Enter 换行/IME isComposing 防护由 `ComposerPrimitive.Input` 原生承担（0.15.31 实证内建）。
10. **滚动行为（FR-13-15）**：`ThreadPrimitive.Viewport autoScroll`（官方 controller：跟随最新 + 上滚暂停）+ 回到底部按钮（`ThreadPrimitive.ScrollToBottom`）。
11. **项目改名入口（16 号票 shortcut）**：左栏项目标题行内编辑 → `save_project`（注册表 name 更新）。
12. **键位表文档化（FR-13-05）**：完整键位表写入票 Answer 并在技术设计文档 §10.6 增补。

不做（记录边界）：附件按钮（FR-13-02 列有但设计文档 §10.6 未承载，M1 无附件管线，M2 再立）；全局热键/i18n/窗口几何记忆/Toast/首次引导/拖拽（P1）；右栏 M2 五页（产物/计划/变更/子代理/配额）；虚拟滚动（P1）；macOS 原生菜单栏（FR-13-04 提及，随打包票打磨）。

验收：cargo 三件套 + 前端三项全绿；manifest 契约覆盖 `resolve_exit_request` 新命令；mock run 结束后消息折叠/展开往返；关闭窗口进程存活（托盘唤起）；通知失焦触发、聚焦抑制。

## Answer

全部落地，检查链全绿（cargo fmt/build/clippy/test 233 lib + 集成；前端 typecheck / vitest 106 例 / Biome；manifest 双向契约 27 命令 10 事件键）。

**Rust 宿主侧**：

- 单实例：`tauri-plugin-single-instance` 2.5.4 最先注册；二次启动 `activate_main_window`（show + unminimize + focus）。
- 托盘：tauri `tray-icon` feature；菜单「显示主窗口 / 退出 CraftWave」；左键单击唤起（Windows/Linux；macOS 左键弹菜单，平台差异随打包票）。
- 后台运行：主窗 close 一律拦截为 hide（FR-13-07 默认；「直接退出」配置项随设置域票）。
- 退出拦截：`shell::request_exit` 无活跃 run 直接 `app.exit(0)`；有则置 `AppState.exit_pending` + emit `app:exit_requested`；前端确认框 → `resolve_exit_request(accepted)`（§3.3 #23，参数名对齐文档）放行/取消。
- 系统通知：`tauri-plugin-notification` 2.7.0。**单一旁路点**——`TauriEventSink::emit` 对 run:done/error/cancelled + ask:opened 四键触发 `NotifyGate`（RecordingEventSink 测试天然无副作用）。防打扰：总开关/仅人工介入（AppConfig.notifications 新域，serde default 兼容旧 config）+ 窗口聚焦且活跃会话相同则抑制；活跃会话由 `set_ui_state` 顺带写 `AppState.active_session`（与通知门共享 Arc）。锚点随通知 `extra("anchor")` 携带；`on_action` 点击回调（含 tap）→ 唤起主窗 + emit `notify:activate` → 前端路由 openTab。
- **run 终态三键补实装**（§3.5）：chat.rs run 任务收尾发 run:done（durationMs）/run:error（code/message）/run:cancelled；错误信息经 `errored` 标志从错误兜底路径带出。
- AppState：config 改 `Arc<ConfigStore<AppConfig>>`（通知门共享缓存，`&Arc<T>` deref 强转调用点零改动）；新增 active_session / exit_pending。
- 新模块：`host/notify.rs`（通知门）、`host/shell.rs`（托盘/退出/唤起）、`host/commands/system.rs`（resolve_exit_request）。

**前端**：

- 主题三态：`theme.ts`（applyTheme → html.dark 切换 + matchMedia 跟随）；App 启动读 config 应用；设置页草稿即时预览（丢弃/保存随 config 回基线自然回滚）；主界面全部硬编码 neutral/sky 色迁移语义令牌（中性强调色 --primary，替换默认蓝）。
- 折叠分组（§10.4/FR-01-10）：**实现形态钉死**——一 run = 一条 assistant 消息（FrameBuffer 累积），折叠落在 AssistantMessage 组件内部（useAuiState(s.message)），非跨消息分组。运行中（isRunning && isLast）全展开；结束后 3s 收起为摘要行 `⚙ N 次工具调用 · 时长`；历史默认收起；点击展开；text parts 恒常显；收起态过程 part 渲染隐藏空元素（非 null）抑制工具卡 fallback（Parts children `?? DefaultPartFallback` 语义实证）。自动折叠开关在 Inspector 头部（ui-state collapseRuns）。摘要数据：marker 收口摘要（`deriveRunSummaries`，SessionTab 收养时注入 RunSummariesContext）优先，run-metrics lastRun（新增 runId 字段）兜底。
- **崩溃尾条中断态（15 号票 shortcut 定案）**：**渲染层推导**——marker endedAt 为空的 run 标「已中断」红字；不改存储 status（append-only 与 canonical 比对零影响；曾评估追加同 id 覆盖行，因 ExportedMessageRepository 对重复 id 行为不明而否决）。
- Composer 全套：`composer-keys.ts`——@ 菜单（detectAtQuery 光标锚定 + 防抖 150ms searchWorkspacePaths + ↑↓/Tab/Enter/Esc）、↑↓ 空输入翻阅本人历史（Esc 恢复）、Esc 停止/清 hint、Shift+Tab 循环权限档（permission-tier store，发送时经 runConfig 惰性求值生效）、Tab 优先级 菜单 > ghost hint；Enter/IME/Shift+Enter 由 ComposerPrimitive.Input 原生承担。档位徽标常驻 Composer。草稿记账（composer text → sessions store → ui-state 防抖 300ms）+ DraftRestorer 挂载恢复。
- 三栏：右栏 Inspector（M1 用量页：三数四口径明细、缓存命中率、上下文进度条 0.6 标线、最近运行）挂活跃 SessionTab 内（随会话取数），开合入 ui-state inspectorOpen；左栏「用量」按钮切换。
- 滚动：Viewport autoScroll（官方 controller 跟随+上滚暂停）+ ScrollToBottom 回底按钮。
- 项目改名：左栏项目标题行内编辑 → save_project。
- 键位表：技术设计文档 §10.6 增补完整键位表（FR-13-05 文档化）。

**新增测试**：composer-keys（@ 触发检测/插入 5 例）、deriveRunSummaries（2 例）、run_manager has_active（1 例）；既有测试适配 3 处（lastRun runId 字段、Send aria-label、manifest 事件 10 键）。

**已知取舍（shortcut 清单）**：

- 通知正文不带会话标题（SessionStore 未暴露 index 查询）；macOS 通知权限申请依赖插件 show 时自动触发，未授权引导随打包票；同会话同类通知合并/频率节流以「每 run/ask 天然唯一」承担。
- 退出确认无超时（前端不回应则保持托盘常驻，下次再触发）。
- 权限档初始值 = DEFAULT_RUN_CONFIG（autoEdit），与 §10.6「重启回 confirm_each」口径的取舍随安全域票复核。
- Inspector 上下文窗口经 get_config 拉激活模型（无激活模型用文档默认 128000）。
- 主题启动有短暂亮色首帧（config 异步读取；防闪随窗口几何记忆 P1 票）。
- 「关闭到托盘改为直接退出」配置项未做（close 一律 hide，P1）。
- 折叠态工具卡 unmount/remount（卡片内部展开状态不保留；过程数据经边车恢复不受影响）。

**新依赖**：`tauri-plugin-single-instance` 2.5.4、`tauri-plugin-notification` 2.7.0（crates.io 官方插件，rsproxy）；tauri features += `tray-icon`。前端零新依赖。
