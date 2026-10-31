# 实现：桌面杂项 P1：窗口几何记忆、Toast、关闭行为配置、全局热键、滚动锚点

Type: task
Status: resolved

## 要做什么

FR-13-11/17/08 + FR-01-11 余量（PRD 行 539/545/536/288，出处 §10.7）。四项互相独立，可拆并行：

- 窗口几何记忆：关掉再开回到原位原尺寸，拔显示器也不丢窗。
- 应用内 Toast：操作反馈统一为顶部轻提示，收敛 M1 零散的行内反馈。
- 关闭行为配置：「关闭按钮=收进托盘还是退出」可配（17 号票挂账清偿）。
- 全局热键：⌥/Alt+Space 系统级唤起/隐藏窗口，可改可关、冲突提示。
- 滚动锚点：会话滚动位置随 Tab 持久化——FR-01-11 最后一项，至此全部收口（Tab/草稿/队列/面板开合 M1 已落）。

出处缺口声明：窗口几何记忆在设计文档**无任何章节**（grep 零命中）——本票票面给出实现口径，动工前先回写 §10.7 增补条目。

## 具体做什么

### 后端（src-tauri/）

- **窗口几何记忆**（实现口径，回写后生效）：
  - 存储：`~/.craftwave/window-state.json`，`{ version: 1, x, y, width, height, maximized }`（逻辑像素）——host 侧直读写（`util/atomic.rs` 原子写）；**不进 ui-state.json**（前端自管域，且 host 须在亮窗前读取）；文件损坏/缺字段 → 丢弃回默认不报错。
  - 保存：主窗 `WindowEvent::Moved/Resized` 防抖 500ms 落盘；`CloseRequested/Destroyed` 时同步强写（退出不丢最后一帧几何）；maximized 单独记布尔（最大化时仍记还原前几何）。
  - 恢复：tauri setup 阶段（窗口 `visible:false` 未亮）读文件 → 越界校验 → `set_position/set_size`（LogicalPosition/LogicalSize）→ maximized 则 `maximize()` → 既有激活协议不动（decoration 激活 → 亮窗，§2.1），"恢复先于亮窗"天然满足。
  - 越界校验口径：与全部显示器 bounds 的可见交集 ≥ 120×80 逻辑像素，否则丢弃整体、回退 tauri.conf.json 默认 1280×800 居中；判定写成纯几何函数便于单测。
- **关闭行为配置**（17 号票挂账：close 一律 hide）：
  - `AppConfig.ui` 增 `close_to_tray: bool`（缺省 true=现状；`providers/types.rs` 已有 `default_true()` 缺省函数；§3.2 UiConfig 未列字段——**先回写 §3.2 再实现**）。
  - 接线：`host/shell.rs` close 拦截处读 `AppState.config`（17 号票已改 `Arc<ConfigStore<AppConfig>>` 共享缓存）——true=hide（现状），false=走 `request_exit`（活跃 run 拦截协议原样生效：`app:exit_requested` → 前端确认 → `resolve_exit_request`）。
- **全局热键**（§10.7 一行：tauri-plugin-global-shortcut，默认 ⌥/Alt+Space，可改可关）：
  - 依赖 `tauri-plugin-global-shortcut` 2.x（crates.io 官方，Cargo + `lib.rs` 注册）；`capabilities/default.json` 增 `global-shortcut:default`（NFR-10 最小权限，§9.5「每接一个插件按需追加最小权限并同步 capabilities」纪律）。
  - 配置：`AppConfig.ui.hotkey: { enabled: bool（缺省 true）, accelerator: String（缺省 "alt+space"） }`——字段级缺失，随 close_to_tray 一并回写 §3.2。
  - 注册/改配：setup 时 enabled 才注册；设置页修改 = `unregister` + `register`（accelerator 归一化：修饰符顺序/大小写归一后比较去重）。
  - 冲突处理：注册失败（被他应用占用）→ 设置页该项红字「快捷键被其他应用占用」+ Toast 提示；应用不崩、不进重试循环；enabled=false 或失败时完全不占系统资源。
  - 回调：toggle 主窗——可见 `hide()`，不可见 `show()+set_focus()`（复用 `shell.rs` `activate_main_window`）；活跃 run 不受隐藏影响；macOS 系统授权差异实测记 Answer。

### 前端（ui/）

- **Toast 反馈层**（FR-13-17/§10.7：标题栏下方顶部居中、自动消失、不打断输入焦点、错误可点开详情）：
  - `stores/toast.ts`（zustand：`{ id, kind: success|error|info, title, detail?, duration? }`，push/自动出队，同屏最多 3 条堆叠）。
  - `components/ToastHost.tsx`：fixed top-10 居中；卡片本体才 `pointer-events-auto`，不抢键盘焦点（NFR-20）；成功/信息 3s、错误 6s 自动消失；错误卡点击 → 既有 `components/ui/dialog.tsx` 展开完整 detail。
  - 收敛接线（M1 零散反馈点盘点）：设置保存成功/失败（SettingsPage）、复制成功（Thread 代码块/工具卡复制钮）、send 失败、导出完成（27 号票）、热键冲突（本票）；
  - 04 号票挂账「压缩失败 Toast 反馈层属 29 号票」→ run:compact_failed 行内提示改走 Toast（压缩中指示条保留不动）。
- **滚动锚点**（FR-01-11 收口）：
  - ui-state 增 `scrollAnchors?: Record<sessionId, { leafId: string; offset: number }>`（schema 前端自管，票内即定义；随既有 300ms 防抖 ui-state 写通道落盘）；
  - SessionTab 挂载收养历史后按 leafId 滚动定位（leafId 已不存在 → 回底部）；Thread 滚动时防抖记账；运行中 autoScroll 跟随时不记（避免回放打底）。
- **设置页**：外观组（`ui/src/settings/SettingsPage.tsx` appearance 页）增「关闭按钮行为」开关、「全局快捷键」启用开关 + 快捷键输入框（录制态：按下组合键捕获显示、Esc 取消、与归一化配置比对去重）。

### 测试

- Rust：越界判定纯函数表驱动（正常位/半出界/全出界/负坐标/maximized/单显示器）；window-state.json 损坏容错；close_to_tray 旧 config 无字段缺省 true（serde default 兼容）；accelerator 归一化（"Alt+Space"=="alt+space"、修饰符重排等价）。
- 前端 vitest：toast store（自动出队 fake timers、堆叠上限、错误 detail 展开）；scrollAnchors roundtrip（sessions store 存取/坏数据容错/leafId 失效回底）；设置页新开关渲染与写回、快捷键录制交互。

## 边界（不在本票）

- 拖拽 FR-13-16（图片与目录由 17 号票承载，普通文件接 17/28 号票既有路径引用）；macOS 原生菜单栏（FR-13-04，随打包面打磨）；托盘菜单项扩展（每项目快捷入口等）；窗口几何跨设备同步（无云同步场景）；Dock 图标隐藏等深度托盘形态。

## 验收

- [ ] 移动/缩放窗口后退出重开，位置尺寸还原；外接显示器拔掉后重启回落主屏默认位，不丢窗不闪崩。
- [ ] 「关闭按钮=收进托盘还是退出」开关生效：关=hide、开=走退出拦截协议（活跃 run 仍弹确认框）。
- [ ] ⌥/Alt+Space 全局唤起/隐藏（应用在他应用前台同样生效）；组合键被占用时设置页红字提示且应用稳定；关掉开关后系统层面无残留热键。
- [x] 复制成功/保存失败/压缩失败等反馈统一为顶部 Toast，错误可点开详情；键盘焦点不被抢走。
- [ ] 会话 A 滚到中部切走再切回（含重启后），滚动位置还原；锚点失效时回落底部。
- [x] §10.7 窗口几何条目、§3.2 UiConfig 字段（close_to_tray/hotkey）回写完成。

## 决议记录（2026-10-30）

**实现完成、检查链七项全绿；未勾验收项均为 GUI 真实交互面（拖窗口/拔显示器/点关闭钮/按全局热键/真实滚动切 Tab/重启），代理不许假绿，留作用户验收。自动化已覆盖的数据面与逻辑面如下，逐项对应：**

- 验收 1（窗口几何）：越界判定纯函数表驱动 10 例（正常位/半出界/全出界/负坐标/纵向不足/压线/单显示器/空清单）、损坏文件容错 7 例（坏 JSON/空文件/缺字段/版本不符/尺寸非正）、serde roundtrip——`host/window_state_test.rs`；防抖 500ms 落盘/CloseRequested 强写/Destroyed 冲盘/setup 亮窗前恢复接线在 `lib.rs` on_window_event 与 setup（tauri 事件面不可单测）。
- 验收 2（关闭行为）：开关渲染与写回有 vitest（close-to-tray-switch）；serde 缺省 true 兼容旧配置有 Rust 单测；lib.rs CloseRequested 分流（true=hide / false=request_exit，活跃 run 拦截协议为 §9.6 既有已测面）——真实点击关闭钮属 GUI 面。
- 验收 3（全局热键）：归一化表驱动 14 例（大小写/修饰符重排/别名收编/重复去重/纯修饰符与空段拒绝）、录制捕获（KeyboardEvent→存储形态、纯修饰键等待、Esc/多媒体键排除）、applyHotkey 注册流 5 例（注册/换键先注销/关闭开关注销幂等/冲突不抛出+错误 Toast+不重试/非法静默）——`hotkey.test.ts`；设置页录制交互（录制态标记/Esc 取消/写回归一化）与占用红字有 vitest——真实按下全局热键（他应用前台生效、系统层无残留）属 GUI 面。
- 验收 4（Toast）：store 自动出队（fake timers，3s/6s 口径）/堆叠上限 3 挤最老/幽灵 timer 不复活有 vitest；接线点——设置保存成功/失败、导出完成/失败、压缩失败（04 号票挂账清偿）、发送失败（start_chat 命令面失败）、热键冲突——均入测试或既有路径；错误卡「查看详情」展开复用既有 Dialog；「焦点不被抢」由结构保证（容器 pointer-events-none、卡片无 tabindex、不调 focus）。视觉呈现细节留用户验收。
- 验收 5（滚动锚点）：computeAnchor/resolveScrollTop 纯几何表驱动、sanitizeScrollAnchors 坏数据清洗、DOM 采集换算与落位/回底（happy-dom）有 vitest；sessions store 桶 roundtrip/落盘带出/删除随清有 vitest；SessionTab 记账（滚动防抖 300ms、运行中 autoScroll 不记、display:none 不记）与恢复（首次收养 + active 沿，双 rAF）接线在 SessionWorkspace.tsx——真实滚动/切 Tab/重启属 GUI 面。

**实现偏差（相对票面，均已记录）：**

1. **热键注册发起侧在前端**（票面未规定侧别）：经官方插件 JS API（@tauri-apps/plugin-global-shortcut 2.4.1，与 Rust 插件 2.4.1 锁面一致）invoke 插件命令注册——注册冲突错误经 Promise 原生拿到，设置页红字 + Toast 零新增契约面；Rust 侧只挂 `with_handler` 回调（按下 → `shell::toggle_main_window`，插件源码核实 JS 注册后 Rust global_handler 仍触发）。WebView 重载后挂载逻辑重放，applyHotkey 幂等（先 unregister-all 再注册），比 Rust setup 侧注册多一层自愈。
2. **capabilities 权限集**：`global-shortcut:default` 为空集（插件 2.4.1 permissions/default.toml 实证），按 NFR-10 显式列 `allow-register/unregister/unregister-all/is-registered` 四项最小集。
3. **代码块复制反馈维持 Streamdown 内建**（按钮态「已复制」，Markdown.tsx translations）：Streamdown 内部组件无回调面，接 Toast 需改上游或包装，收益不抵风险；工具卡「复制错误」钮保留行内 `已复制` 反馈（卡片内即时态，与 Toast 场景互补），未强行 Toast 化。
4. **compaction store 的 phase 收敛**：`failed` 态删除（store 收敛为 `null | "compacting"`，CompactionIndicator 撤除常驻失败行）——失败反馈改走 Toast 后 failed 态无消费面，保留反而误导；CompactButton 禁用逻辑不变。
5. **滚动锚点恢复时机细化**：仅「挂载后首次历史收养」与「active false→true」两时机恢复；观察态收口的树重载不恢复（避免把运行中用户滚动位置拉回 run 开始前的锚点）。双 rAF 等布局——超大历史首屏渲染超两帧的极端场景，切一次 Tab 即恢复（诚实记录，未构造该场景测试）。
6. **窗口几何位置取 `outer_position`、尺寸取 `inner_size`**（tauri-plugin-window-state 同款口径；tauri.conf.json 的 width/height 语义即 inner size），越界判定显示器 bounds 按各自 scale_factor 转逻辑域。
7. **测试基建注记**：happy-dom 无布局引擎，scroll-anchor 的 DOM 用例以覆写 `getBoundingClientRect` 模拟布局（换算公式与回落链路已验）；SettingsPage 测试桩 mock 热键插件为 no-op（注册流单测独立在 hotkey.test.ts）。

**遗留（不影响本票收口）：** ① macOS 上 tauri-plugin-global-shortcut 的系统辅助功能授权差异（macOS 15+ 多数组合键免授权，个别场景请求授权弹窗）未在开发机构造，归验收 3 的用户真机面；② 窗口几何在 Wayland（Linux）下受窗口管理器协议限制可能不生效——三平台为 Windows/macOS 优先口径，票面未列 Linux 细化要求。
