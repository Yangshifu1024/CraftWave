# 实现：桌面杂项 P1：窗口几何记忆、Toast、关闭行为配置、全局热键、滚动锚点

Type: task
Status: open

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

- 拖拽 FR-13-16（图片/文件附件拖入随 17 号票图片输入；「拖拽目录=提议添加项目」全图无票，需要时另立）；macOS 原生菜单栏（FR-13-04，随打包面打磨）；托盘菜单项扩展（每项目快捷入口等）；窗口几何跨设备同步（无云同步场景）；Dock 图标隐藏等深度托盘形态。

## 验收

- [ ] 移动/缩放窗口后退出重开，位置尺寸还原；外接显示器拔掉后重启回落主屏默认位，不丢窗不闪崩。
- [ ] 「关闭按钮=收进托盘还是退出」开关生效：关=hide、开=走退出拦截协议（活跃 run 仍弹确认框）。
- [ ] ⌥/Alt+Space 全局唤起/隐藏（应用在他应用前台同样生效）；组合键被占用时设置页红字提示且应用稳定；关掉开关后系统层面无残留热键。
- [ ] 复制成功/保存失败/压缩失败等反馈统一为顶部 Toast，错误可点开详情；键盘焦点不被抢走。
- [ ] 会话 A 滚到中部切走再切回（含重启后），滚动位置还原；锚点失效时回落底部。
- [ ] §10.7 窗口几何条目、§3.2 UiConfig 字段（close_to_tray/hotkey）回写完成。
