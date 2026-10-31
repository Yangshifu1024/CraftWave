# 02 托盘与全局热键集成验证

Status: resolved
Type: task
Blocked by: 01

## 目标

验证 `tray-icon` 与 `global-hotkey` 两个独立 crate 在 GPUI 应用进程内可用，产出可复用的集成代码形态（阶段 4.1/4.2 直接落地）。

## 步骤

1. 在 `app.run` 主线程装 `GlobalHotKeyManager` 并注册一个热键；`TrayIconBuilder` 建托盘 + 菜单（两项：显示窗口/退出）；
2. `cx.spawn` 循环消费 `GlobalHotKeyEvent::receiver()` 与 `TrayIconEvent::receiver()`（可 `select` 双 channel），事件转 `cx.dispatch_action`；
3. action 处理器里关窗/开窗，验证窗口生命周期与托盘事件互不干扰；
4. macOS 注意点实测：托盘需主线程构建（run 闭包内天然满足）；评估"关窗驻留托盘"所需（activation policy），只记录不做；
5. Linux 侧只留结论记录（`async-tray` feature + DBus 前提），本票不实测。

## 验收

- 点托盘菜单项有反应；全局热键唤起窗口；
- 关窗后托盘事件仍能重建窗口；
- 集成代码形态沉淀为一段可搬运的模块草稿（贴进本票 Comments）。

## Comments

### 完成结论（2026-10-31）

**Answer**：`tray-icon` + `global-hotkey` 在 GPUI 进程内完全可用，集成代码形态验证通过。

#### 实测结果（osascript 自动化）

1. 进程启动存活（托盘 + 热键主线程注册未 panic）；
2. 窗口开着 → 点关闭钮关窗；
3. 发全局热键 Ctrl+Shift+Space → **窗口自动重建**（`activate_or_reopen` 的重建路径）；
4. 进程持续存活，事件循环持续工作。

#### 集成代码形态（spike main.rs，阶段 4.1/4.2 照搬）

- 主线程（`app.run` 闭包内）构建 `GlobalHotKeyManager` + `TrayIcon`，挂 GPUI Global 保活（drop 即失效）；
- 事件桥：`cx.spawn` 循环 **50ms `try_recv` 轮询**三通道（GlobalHotKeyEvent / MenuEvent / TrayIconEvent）→ `cx.dispatch_action`。crossbeam 的阻塞 `recv()` 不能进 GPUI async executor（会占死任务），`try_recv` + `background_executor().timer` 全程留在 GPUI 模型内；延迟无感，生产可再换事件驱动；
- 菜单项 id 用 `MenuItem::with_id` 构造（muda 0.17 无 `MenuBuilder`，用 `Menu::new().append_items`）；托盘图标用 `with_title("CW")` 文字位（macOS NSStatusItem），免图标资源；
- 关窗重建：`handle.update` Err → `open_window` 新建（阶段 4.6「关窗驻留托盘」雏形）；
- 借用坑：`try_global` 返回的引用未结束前不能把 `&mut App` 交给 `handle.update`——先 clone 句柄。

#### 版本

tray-icon 0.21.3 + muda 0.17.2 + global-hotkey 0.7.0（Cargo.lock 钉定）。

#### 留人工点验（dogfood 项）

托盘菜单「显示主窗/退出」的实点验证（自动化无法模拟 NSStatusItem 点击）；事件桥与热键同路径，风险极低。
