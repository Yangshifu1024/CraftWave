# 19. 实现：CI/平台欠账收敛

Type: task
Status: claimed

## Question

收敛 17 号票与 Windows CI 修复期挂账的四项平台欠账（HandOff 二节欠账清单）：

1. **e2e 双平台 executor**：进程树收割 e2e 当前 cfg(unix) Windows 跳过——按 Windows Job Object 路径补 executor，让批取消收齐语义有真实覆盖（当前 Windows 无任何取消传播 e2e 实跑）。
2. **macOS 托盘左键单击**：当前 macOS 左键弹菜单，与 Windows/Linux 左键唤起主窗不一致——`show_menu_on_left_click(false)` + `TrayIconEvent::Click` 唤起主窗，右键弹菜单，三平台行为对齐。
3. **macOS 通知权限引导**：未授权时设置页通知区给引导（含打开系统设置入口）；通知触发侧静默降级不报错。
4. **退出确认超时兜底**：`app:exit_requested` 后前端无响应则退出流程挂死——加超时（默认 10s）自动放行退出（用户退出意图优先；活跃 run 由 append-only 崩溃恢复语义兜底，重启标「已中断」）。

验收：cargo 三件套全绿；Windows CI 上取消传播 e2e 真实执行；托盘行为三平台口径一致（macOS 人工验证）；退出超时兜底有测试。
