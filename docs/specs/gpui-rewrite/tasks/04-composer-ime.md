# 04 Composer IME 验证

Status: resolved
Type: task
Blocked by: 01

## 目标

验证 gpui-component Input 作为 composer 输入框时中文输入法全流程可用（Zed 文本栈的 IME 路径），确认 Enter/Shift+Enter 行为可编程。

## 步骤

1. Input 组件接最小 composer：Enter 触发发送回调、Shift+Enter 换行；
2. 中文拼音输入全流程：候选窗跟随光标、选字、整句输入、中途删除、英文中文混排；
3. 验证输入法组合期（marked text）Enter 不误触发发送；
4. 粘贴多行文本、超长文本的表现；
5. @ 触发（输入 `@` 弹补全浮层）的最小原型，验证组合期与浮层键盘导航不冲突。

## 验收

- 中文输入全流程无异常（候选窗位置正确、无重复字符、无丢键）；
- 组合期 Enter 不发送；
- Enter/Shift+Enter 行为正确；
- 发现的坑记入 Comments（含规避方式）。

## Comments

### 完成结论（2026-10-31）

**Answer**：用户实测验收通过——中文拼音候选窗跟随、选字上屏、组合期 Enter 不误发、Enter 发送、Shift+Enter 换行全流程正常。**GPUI/Zed 文本栈的 IME 路径可用，composer 最大风险项解除。**

#### 组件选型与踩坑（正式版 composer 的直接规范）

1. **必须用 `Textarea`（多行）而非 `Input`（单行）**——单行模式下 Shift+Enter 被吞（首个验收轮实测失败项）。多行 + `auto_grow(1, 6)`（1 行起步、6 行封顶内部滚动）+ `submit_on_enter(true)`：组件内建分流，裸 Enter 发 `PressEnter{shift:false}`、Shift+Enter 自行插换行；
2. **`set_value`/`set_placeholder`/`InputState::new` 都要 `&mut Window`**——清空输入框必须走 `cx.subscribe_in(&input, window, ...)`（闭包 5 参带 `&mut Window`）；普通 `subscribe` 闭包 4 参无 window，拿不到；
3. 事件：`InputEvent::{Change, PressEnter{secondary, shift}, Focus, Blur}`（定义在 gpui-base，gpui-component re-export）；
4. 列表 Top 对齐时发送消息要 `insert(0, …)` 而非 push（视口可见性，流式调试同款教训）。

#### 顺带修复的两个 UI 问题（同为正式版经验）

- **traffic light 与自绘标题重叠**：`TitleBar::window_options()`（title: None + appears_transparent + traffic_light_position (9,9) + app_owns_titlebar_drag）配 `TitleBar::new()` 组件——红绿灯定位、拖拽区、双击缩放组件全包；
- **标题带与内容区主题撞色**：内容区颜色硬编码导致与 TitleBar 的系统主题不同源。修复 = 全部颜色走 `ActiveTheme`（`cx.theme().background/foreground/secondary/border`），亮暗自动跟随系统——正式版渲染规范「语义 token」路线的直观验证。

#### 噪音记录

stderr 偶发 `IMKCFRunLoopWakeUpReliable` mach port 报错——macOS 输入法服务切换噪音，无害。
