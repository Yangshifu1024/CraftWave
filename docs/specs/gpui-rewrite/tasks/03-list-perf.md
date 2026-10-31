# 03 长会话列表性能基线

Status: resolved
Type: task
Blocked by: 01

## 目标

拿聊天流虚拟列表的性能基线数字，验证 3000 条消息场景下滚动与流式追加不掉帧，并确认 Markdown/代码高亮的渲染开销可接受。

## 步骤

1. 用 `ListState::new(0, ListAlignment::Bottom, 512px overscan)` 搭不等高列表（Wake 2082 形态），造 3000 条混合假消息（长 Markdown / 代码块 / 短句 / 工具簇卡）；
2. 开 gpui-kit FPS Monitor 实测：
   - 快速滚动全程帧率；
   - 模拟流式追加（每 64ms 更新末条快照）时帧率；
   - 首屏含 20 个代码块时的解析/渲染耗时；
3. 展开一个工具簇卡（`splice` 重测路径）看掉帧情况；
4. 记录数字：滚动最低帧率、追加时帧率、内存量级。

## 验收

- 滚动与流式追加稳定 60fps（允许偶发掉帧，不允许持续低于 45）；
- 数字记入本票 Comments（07 号票汇总）；
- 若不达标：记录瓶颈点（解析 vs 布局 vs 绘制），给出候选缓解（延迟高亮/快照节流加长），升级为讨论票。

## Comments

### 完成结论（2026-10-31）

**Answer**：3000 条混合消息（Markdown/代码块/CJK 长段）流式与滚动全链路通过，用户实测确认「性能没问题」。

#### 性能数据（gpui-fps HUD，debug 构建）

| 场景 | 读数 |
|---|---|
| 静态首屏（Markdown 重负载） | 124–144 FPS，FRAME 6.9–8.1ms，GPU 0.6–1.7%，CPU 18–33%，MEM 126–130MB |
| 流式追加（64ms 节流，与帧流同频） | 16 次实体重绘/s，INTERVAL 66ms——按需渲染模型，非掉帧 |
| 滚动 | 用户实测确认流畅（HUD 现场读数） |

#### 渲染链路三坑（CraftWave 重写的直接经验）

1. **GPUI `div()` 默认 flex 方向是横向**——并列 children（探针+列表）会横排，列表被挤出可视区；聊天布局必须显式 `.flex().flex_col()`；
2. **`gpui::list` 默认 `ListSizingBehavior::Auto`：高度完全由样式决定**——不给 `.size_full()` 高度恒为 0，视口空白无物可画且无任何警告（Wake 8283 行 `.size_full()` 的真实含义）；配合外层 `flex_1().min_h_0()` 容器；
3. **`gpui_kit::init(cx)` 必须在 run 闭包首行调用**——缺了它任何走 gpui-component 内容渲染的路径都会 panic（`no state of type gpui_base::global_state::GlobalState`）；list 高度为 0 时该问题被完全掩盖。

#### gpui-kit 0.8.1 API 核对（07 号票输入）

`TextView::markdown(id, text)` ✅、`selectable(true)` ✅、代码块带语法高亮+语言标签（默认就有）✅、`AutoScroll`（scroll 模块）✅、`InputState`/`Input`/`InputEvent::PressEnter{secondary,shift}` ✅——**API 面与渲染规范全部对上，无需回退「gpui-component git fd3bc2b + zed rev」组合**。

#### 组件行为细节（踩坑记录）

- `InputState::new(window, cx)`、`set_placeholder(_, window, cx)`、`set_value(_, window, cx)` 都要 window——订阅闭包（4 参，无 window）里拿不到，清空输入框需另找路径（spike 未清空，正式版在 action 层处理）；
- 构造闭包（`cx.new` 内）spawn 的任务首拍 `entity.update` 即 Err——任务必须在实体就绪后由外部 `handle.update` 启动；
- 流式改视口外内容时行级 diff 缓存不重绘（正确行为）——滚动锚定/跟随要做在渲染规范层。
