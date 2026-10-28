# 19. 实现：CI/平台欠账收敛

Type: task
Status: resolved

## Question

收敛 17 号票与 Windows CI 修复期挂账的四项平台欠账（HandOff 二节欠账清单）：

1. **e2e 双平台 executor**：进程树收割 e2e 当前 cfg(unix) Windows 跳过——按 Windows Job Object 路径补 executor，让批取消收齐语义有真实覆盖（当前 Windows 无任何取消传播 e2e 实跑）。
2. **macOS 托盘左键单击**：当前 macOS 左键弹菜单，与 Windows/Linux 左键唤起主窗不一致——`show_menu_on_left_click(false)` + `TrayIconEvent::Click` 唤起主窗，右键弹菜单，三平台行为对齐。
3. **macOS 通知权限引导**：未授权时设置页通知区给引导（含打开系统设置入口）；通知触发侧静默降级不报错。
4. **退出确认超时兜底**：`app:exit_requested` 后前端无响应则退出流程挂死——加超时（默认 10s）自动放行退出（用户退出意图优先；活跃 run 由 append-only 崩溃恢复语义兜底，重启标「已中断」）。

验收：cargo 三件套全绿；Windows CI 上取消传播 e2e 真实执行；托盘行为三平台口径一致（macOS 人工验证）；退出超时兜底有测试。

## Answer

四项全部落地（2026-10-28，feat/t19-platform-debts 分支）：

### 1. e2e 双平台 executor（Windows 批取消语义真实覆盖）
- `tests/backend_e2e.rs` 的 `e2e_tool_batch_cancel_keep_completed_mark_rest` 去掉 `#[cfg(unix)]`，执行器按平台分派真实进程树：
  - Unix：原样 `sh -c 'sleep 900 & echo CHILD_PID=$!; wait'` + 进程组收割（SIGTERM→宽限→SIGKILL）；
  - Windows：PowerShell `Start-Process -PassThru ping …` 起已知 pid 的孙进程并 `Wait-Process` 挂住，spawn 后即 `JobGuard::assign` 入 Job，取消时 `job.reap`（TerminateJobObject 整树）；孙 pid 走 stdout 广播，收割后以 `pid_alive` 探测消亡。
- `tools/process.rs` Windows 面补 `pid_alive`（`OpenProcess(PROCESS_SYNCHRONIZE)` + 0ms 探测等待，对齐 Unix `kill(pid,0)` 语义）并重导出——scratch 交叉编译检查揪出「win_job 未重导出 pid_alive」这颗必炸 CI 的雷后补上。
- 孙 pid 通道类型统一 `u64`（i32/u32 双源归一）；`TOOL_COMMAND` 的 `cfg_attr(dead_code)` 放行删除；顺手清掉重复的 `#[cfg(unix)] #[cfg(unix)]`。
- S5 spike（`spike_s5_cancel.rs`）保持原型冻结口径不动（其文件头已声明 Windows 只验可编译，语义覆盖由 backend e2e 承接）。
- **Windows 验证程度（本机 macOS）**：整仓 `cargo check --target x86_64-pc-windows-msvc` 不可行（aws-lc-sys 需 Windows 工具链，HandOff 五节既知）；改用 scratch 独立 crate 真拷 `tools/process.rs` + e2e Windows 执行器同构代码，`cargo check --target x86_64-pc-windows-msvc --all-targets` 通过（windows-sys 0.59 同 feature 面）。语法/类型/cfg 面已尽本地所能；运行行为待 Windows CI 实跑（若 PowerShell 起进程在 CI 环境有出入，预计改的只是 `spawn_command_tree` 一处）。

### 2. macOS 托盘左键对齐
- **核实结论：代码行为已对齐，欠账是注释口径**。以依赖源码核实（tauri 2.12.5 `TrayIconBuilder::show_menu_on_left_click` → tray-icon 0.25.1 macOS `on_tray_click`：`menu_on_left_click=false` 时左键只发 `Click{Left, Up}` 事件、菜单仅在右键时挂到 NSStatusItem（`performClick` 按需弹出））——17 号票记的「macOS 左键弹菜单」与依赖实现不符，属未实跑的保守记录。
- 代码面无功能改动（`show_menu_on_left_click(false)` + `TrayIconEvent::Click` Left+Up 唤起本就在位）；重写 `install_tray` 与模块注释为三平台一致口径，注明 Linux AppIndicator 能力边界（点击事件/左右键区分不受支持，唤起降级为菜单项）。**macOS 实机点击行为待用户验收**（acceptance.md 走查）。

### 3. macOS 通知权限引导
- 摸底：tauri-plugin-notification 2.7/2.8 桌面端 `permission_state()` 恒返回 Granted（源码核实），前端 `is_permission_granted` 拿不到 macOS 真实授权态——引导数据面自建。
- 新增两命令（`host/commands/system.rs`，manifest 单一事实源同步）：
  - `notification_permission_status → NotificationPermissionStatus { status, hasSettingsEntry }`：macOS 直查 UNUserNotificationCenter（`getNotificationSettingsWithCompletionHandler`，主线程发起 + channel 回传 + 2s 超时按 `unknown`；dev 裸进程无 bundle id 时早退 `unknown` 防 ObjC 异常）。Provisional/Ephemeral 归 granted。objc2 系依赖已在树内（objc2-user-notifications 0.3.2 等），零新增下载。
  - `open_notification_settings`：macOS 深链 `x-apple.systempreferences:com.apple.preference.notifications`，Windows `ms-settings:notifications`（CREATE_NO_WINDOW），Linux 无统一入口报 E_INTERNAL（前端按 hasSettingsEntry 不渲染按钮，正常不触达）。
- 设置页「系统通知」区（`ui/src/settings/SettingsPage.tsx`）：prompt/denied 时出 warning 引导条（含「打开系统设置」按钮），窗口重聚焦（从系统设置返回）自动重查；granted/unknown 不打扰。
- 通知触发侧静默降级已在位（`NotifyGate::send` 的 `let _ = show()`），补注释钉死口径。
- 契约面：manifest 双侧锚点（ipc_manifest_test.rs + contract.test.ts 增 `notification_permission_status`/`open_notification_settings` 双向接线断言）；前端 `types.ts`/`client.ts` 接线。

### 4. 退出确认超时兜底
- `host/shell.rs` 新增 `ExitConfirm` 状态机（request 计时/resolve 解除/timed_out 判定，时间显式注入可确定性单测）+ `EXIT_CONFIRM_TIMEOUT = 10s`；`AppState.exit_pending: Mutex<bool>` 升级为 `exit_confirm: Mutex<ExitConfirm>`。
- `request_exit` 发出 `app:exit_requested` 后排定定时任务：到点复查仍挂起 → `app.exit(0)`（用户退出意图优先）；前端已回执则 no-op。回执与兜底竞态无害：两边都先解除挂起，只有先到者生效。重复点退出刷新计时起点。
- 活跃 run 数据安全不靠拦截退出：append-only 崩溃恢复语义兜底（重启标「已中断」，15/17 号票既有），注释同步。

### 测试
- Rust：lib 241（+8：shell_test 6 例超时口径——时限内不兜底/到点放行/拒绝取消兜底/确认立即退出/重复请求刷新计时/无请求不兜底；notify_test 2 例授权态映射与字面值契约）；集成含重写的双平台批取消 e2e。断言消息中文、方法名英文 snake_case。
- 前端：vitest 111（+4：引导条 prompt 出现并走深链/denied 文案/granted 与 unknown 不打扰/无设置入口不出按钮）。
- 检查链全绿：cargo fmt --check / build / clippy -D warnings / test（macOS 本地）；pnpm typecheck / test / lint。manifest 契约顺序先 cargo test 后 pnpm test。

### 取舍与遗留
- 取舍①：授权态直查 objc2-user-notifications 而非升级插件（2.8.0 桌面端仍是恒 Granted 桩）或加 npm 包——依赖树内已编译、无新增下载面。
- 取舍②：unknown（dev 裸进程/超时）一律不显示引导——宁可少打扰；生产 bundle .app 才有真实口径。
- 遗留①：Windows e2e 运行行为与 `spawn_command_tree` 的 PowerShell 依赖待 CI 首跑确认（预期 0–1 轮修正）。
- 遗留②：托盘左键 macOS 实机点击待用户验收（本票人工验证项）。
- 遗留③：S5 spike 保持 unix-only 冻结（原型存档，不随产品 e2e 平台化）。
