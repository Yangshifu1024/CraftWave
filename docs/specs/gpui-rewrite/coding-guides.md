# GPUI Kit 编码指南（项目内化版）

> 来源：gpui-kit.com/zh-CN/docs/coding-guides/（2026-10-31 抓取，CC BY 4.0）。本文不是翻译，是把官方规范内化成**本仓库子代理的执行检查单**——每条都对应「写代码前想什么、写完查什么」。与 rendering.md 冲突时，渲染像素语义以 rendering.md 为准（见 §3 例外口径）。

## 1. 架构与所有权（写每个模块前先回答）

- **依赖只向下**：app shell（main.rs，只组合不承载功能逻辑）→ feature 模块（chat/、后续 sidebar/、panels/ 等，各自拥有 model Entity + view + command）→ gpui-kit。禁止反向依赖、禁止跨 feature 摸对方私有模块；
- **禁止全局 `models/`、`views/`、`commands/` 目录**——按业务能力组织（会话树的模型住会话树模块，不住全局模型目录）；
- 状态放**最窄 owner**：领域状态归 feature 的 model Entity；瞬态视图状态归绘制它的 view；可复用行为状态归组件状态；真正全应用共享的 service 才用 `Global`；
- 父持子用强 `Entity<T>`，子回指父**必须** `WeakEntity<T>`（强回指 = 所有权环）；`upgrade()` 返回 Option、`update` 返回 Result，父已释放是正常生命周期要处理；
- `Subscription` 保存在 owner 的 `_subscriptions: Vec<Subscription>`；`detach()` 默认不用（长寿命 emitter 上反复 detach 会让 callback 累积）；
- 不保存 `&mut Window`/`&mut App`/`&mut Context<_>` 到调用之外——只存 Entity/WeakEntity/FocusHandle/scroll handle/领域 ID。

## 2. 组成单元与渲染

- **值类型 UI 默认 `RenderOnce`**（props 进、元素出、用完丢）；**跨 frame 行为（订阅/异步/历史/子 Entity）才 `Entity<T>` + `Render`**。不要给每个视觉碎片建 Entity；
- `render` 保持声明式：读状态、派生展示值、拼元素；业务操作/解析/复杂变更进具名 method；render callback（list item renderer 等）**必须无副作用**（会被测量/重绘多次调用）；
- **render 里不能无条件 mutate/notify**（反馈环 = 永久重绘）；同一不变式的字段一次更新只 notify 一次；改变渲染结果后 `cx.notify()`，语义事件 `cx.emit(...)`；
- 受控值优先：传当前值、回调表达意图、owner 更新后重渲——禁止回调里再建第二份隐藏真相；外部设值不得再经 user callback 回传（防状态反馈环）；
- **稳定 `ElementId`**：行/页签/树节点用稳定领域 ID；可插入重排的数据**禁止**用可变下标或翻译 label 当 ID；render 里禁止生成随机 ID；
- `h_flex()` 交叉轴是**居中**、`v_flex()` 是 stretch——row 里的 column 若要吃满行高，显式 `items_stretch()` 或给 column `h_full()`（否则 header 被居中挤出上边缘裁掉，经典坑）。

## 3. 主题与像素红线（本仓库口径）

- **应用代码零 raw hex/rgb/rgba/hsla**——颜色只从 `cx.theme()` / CraftwaveTheme 取；缺语义槽位就补进 theme.rs，不许 call site 造色（theme.rs 本身与测试断言例外）；
- **布局优先 rem-based helper**（`p_2()`/`gap_3()`/`text_sm()`/`w_64()`），直接 `px(...)` 视为 review finding。**例外白名单**（须注释说明）：① theme.rs 的 token/字号定义本身；② rendering.md 已文档化的像素语义（气泡 max_w 540、阅读轴 READER_MAX_WIDTH、overscan 512、缩略图 104、思考缩进 30 等）；③ 平台物理边界（窗口默认/最小尺寸、traffic light 位置、几何记忆）；④ 运行时测量结果。其余一律迁移或改 rem；
- **zoom 的正确姿势**：base font（rem 源头）变 → 所有 rem 布局自动缩放；但**测量缓存必须把 `window.rem_size()` 纳入失效键**——变高列表 remeasure、text shaping、popup 几何在 rem 变化后全部失效重测（对应 spec 2.3 的 remeasure 钩子）；
- 主题写入只走 `Theme::update(cx, |theme| ...)`（它同步 colors/tokens/Base 三份并刷窗）；`Theme::global_mut` 只改你碰的字段，禁止；
- focus ring 要留空间，祖先 `overflow_hidden()` 会裁掉它；键盘焦点不可悄悄消失。

## 4. 事件、键位与焦点

- **先 `cx.bind_keys` 再 `cx.set_menus`**（set_menus 在调用刻固化 keymap；keymap 后来变了要重调 set_menus）；
- 一个逻辑命令只建模一次：菜单项/工具栏按钮/右键菜单/键位 dispatch 同一个 Action 或调同一 owner method，不许复制五份变更逻辑；
- `Link` 只给交给浏览器/邮件的 URL；应用内导航用导航组件、命令用 Button/Action；
- 焦点 owner 明确：键盘交互的 Entity 持 `FocusHandle`，`key_context` 与 `on_action` 附着同一 focused region；overlay 开时转移焦点、关时恢复；render 里禁止无条件 request focus；被 track 的 handle 要 `tab_stop(true)` 才是 Tab 停靠点；
- stop propagation 只在确实要阻止 parent 处理同一事件时用。

## 5. 异步与副作用

- Task 从事件/生命周期钩子/具名 method 启动，**不是 render 的副作用**；重复运行的 Task handle 存 owner（owner 释放即取消），默认不 detach；
- 异步状态用 idle/loading/loaded/failed 明确建模；刷新保留旧数据；错误显示在 UI 不只写 log；
- 昂贵计算走 background executor，Entity 变更回应用 context；结果绑 revision/ID 拒绝过期结果（流式快照的 seq 对账就是这一条）。

## 6. 布局、滚动与虚拟化

- 每个 scrollable region **只有一个 owner**；滚动条悬在内容与面板中间 = owner 错了或内边距放错层；
- 可收缩 child 在 flex 里要 `min_w_0()`/`min_h_0()`，否则不会围绕长内容收缩；
- 数据可能超小型有界集合就虚拟化（消息流用 ListState）；**row identity 与可见位置分开**，渲染路径不 clone 全量数据；宽度/字体/rem/行内容变化要 invalidate 测量。

## 7. 命名与词汇（API 是词汇表）

- Reader：布尔 `is_<形>`/`has_<名>`（禁止新增 `can_`）；非布尔 reader 用字段名词；builder `with_<field>`；就地变更 `set_<field>`；回调 `on_<事件>`；命名区域 `render_<区域>`；索引 `_ix`、标识 `Id`；
- 外层名字已说明的不要重复（`with_item_ix(ix)` 而非 `with_item_ix(item_ix)`）；
- 领域词精确：selected=持久选中 / focused=键盘目标 / hovered=指针悬停；open/close=overlay 状态 / show/hide=瞬态展示 / expand/collapse=结构；disabled=禁交互 / readonly=可导航不可编辑（一个词）；index=当前位置 / id=稳定身份；
- 禁模糊名：`data`/`handle_action`/`update_ui`/`process`/`manager`（除非真在协调集合）。

## 8. 测试策略（能证明行为的最低层）

1. 纯函数测试：状态迁移/几何/解析/排序；
2. GPUI context 测试：Entity/事件/订阅；
3. `VisualTestContext` 交互测试：焦点/键盘/指针/渲染结果；
4. 冒烟：完整工作流。

交互组件测**语义契约**（激活、受控值、disabled、焦点移动、事件次序、稳定 ID），不测实现细节；bug 可稳定复现时修复前先加回归测试。

## 9. 编码代理六问（每个改动交付前逐条回答）

① 行为 owner 与表现 owner 是谁；② retained identity 与状态生命周期是什么；③ 指针/键盘/焦点/无障碍契约是什么；④ 布局与溢出 owner 是谁；⑤ 用了哪些 theme token，例外为何存在；⑥ 哪个测试会在行为退化时失败。

改前必读与改动最相关的实现、测试、re-export seam 与组件文档；**必须在当前源码搜方法签名**，禁止从 React/CSS/旧 GPUI 示例类比翻译。

## 10. 常见失败模式（出现即打回）

一个 Entity 塞全应用状态；render 里混业务逻辑/网络请求；重排内容用随机/下标 ID；硬编码色/圆角破坏主题；有语义组件还用 clickable div 重造焦点/键盘/disabled/无障碍；受控模型旁复制本地状态漂移；render 变更引发 notify 循环；无 owner 的嵌套滚动；为一次性页面加组件 variant；低风险操作也弹确认框；测试只调内部方法从不执行指针/键盘行为。

## 11. 构建验证纪律（三档口径，混合跑 = 缓存变体膨胀）

背景：不同 cargo 子命令启用不同 feature 集，产物按 feature 组合留盘互不覆盖——曾把 target/ 膨胀到 67G（14G 是 feature 组合差异堆出的 incremental 缓存）。每批任务只允许三档命令，禁止混跑其他形态：

1. **日常迭代**：`MACOSX_DEPLOYMENT_TARGET=13.0 cargo check -p <包名>`（最小 feature 集，最快反馈）；
2. **收尾验收**：`MACOSX_DEPLOYMENT_TARGET=13.0 cargo clippy --workspace --all-targets -- -D warnings` + `MACOSX_DEPLOYMENT_TARGET=13.0 cargo test --workspace`（这两档共享同一套全 feature 产物）；
3. **完整构建**：`cargo build` 只在提交前检查与用户要求看运行效果时跑。

- 批量验证（测试套件/CI）设 `CARGO_INCREMENTAL=0`——测试与 clippy 不消费增量缓存，设了只产垃圾；本地手动开发不受影响；
- **依赖与 profile 变更一律显式提交**（版本号逐个写明理由，禁止 `cargo upgrade` 类批量升级；禁止顺手升依赖、禁止改了 Cargo.toml 不提交）——曾有批次被中途打断留下 rig 0.45/0.46 半成品混搭导致构建挂；
- dev profile 已降为行号级调试信息（根 Cargo.toml `[profile.dev]`），需要调试器看变量值时临时注释重编。

