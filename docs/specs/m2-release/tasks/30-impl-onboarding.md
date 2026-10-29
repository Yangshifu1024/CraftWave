# 实现：首次启动引导

Type: task
Status: open

## 要做什么

FR-13-18（PRD 行 546，出处 §10.7）：全新用户第一次打开 CraftWave 不用自己摸设置——四步向导（语言 → 供应商与 Key（含连通测试）→ 项目 → 权限档简介）把「能开始干活」的最小配置走完：

- 每一步都可跳过；跳过后顶部常驻「继续配置」入口随时回来补。
- 引导绝不阻塞临时会话直接开聊（PRD 原文口径）。
- 二次启动不再自动弹。

## 具体做什么

### 后端（src-tauri/）

- 零新命令零新存储——向导所需 IPC 全部已注册或他票在途：
  - `get_config`/`save_config`（#6/#7）、`save_provider`（#8，newKey 通道）、`select_dir`（#5，已实装 lib.rs）、`save_project`（#11）、`create_session`（#13）、`get_ui_state`/`set_ui_state`（#30/#31）；
  - 连通测试用 `test_provider`（§3.3 #32：`{ providerId, baseUrlOverride? } → { ok, latencyMs?, models?, error? }`，19 号票实装）。
- 本票 Rust 侧唯一可能的增量：19 号票未先行时 `test_provider` 未注册——前端按钮降级为「稍后在设置中测试」（不阻塞向导；软依赖记 Answer，不新增 Blocked by 行）。

### 前端（ui/）

- **向导组件** `ui/src/onboarding/OnboardingWizard.tsx`（全屏覆盖层 + 顶部 stepper 进度指示，复用 `components/ui/*` 既有基件：input/select/button/dialog）：
  1. **语言**：zh-CN / en-US 单选 → `save_config` 写 `config.ui.language`（`providers/types.rs` Language 枚举已有 zhCn/EnUs）；
     - 31 号票 i18n 落地前界面文案维持中文，本步只落配置值——31 落地后该步自动获得即时切语言效果（联动注记回写 §10.7）。
  2. **供应商与 Key**：
     - 预设下拉（复用 `settings/presets.ts`）+ 表单——**从 `settings/ProvidersPage.tsx` 抽取供应商表单为共享组件**（字段校验/newKey 密码框逻辑单源，避免两处漂移）；ProvidersPage 改为消费共享组件，既有 SettingsPage.test.tsx 用例保绿为回归红线；
     - 「测试连接」按钮 → `test_provider`；失败红字展示 error（错误分类提示沿用 19 号票口径），「仍要保存」与「跳过」并存；
     - 保存走 `save_provider`（newKey 通道）。
  3. **打开/创建项目**：`select_dir` 原生选目录 + 项目名输入 → `save_project`（`{ id?, name, homeDir }`）；跳过 = 用临时会话（roots=数据目录 workspace/，C-8 口径）。
  4. **权限档与安全模式简介**：静态说明页——四档循环 Shift+Tab 各自语义一句话、审批弹窗怎么应对、计划门何时出现、设置页入口指引；仅一个「完成」钮，纯展示无写操作。
  - 每步右上「跳过」单步跳过（step 前进）；顶部关闭钮（×）整包关闭 = 跳过未完成。
  - 键盘可达：全流程 Enter/Tab/Esc 可操作（NFR-20，§10.6 既有键位纪律）。
- **状态持久化**（ui-state 前端自管域，票内即定义 schema，无 Rust 改动）：
  - `onboarding: { completed: boolean; step: number; skippedAt?: number }`；
  - 完成（走完第 4 步）或单步跳过均即时 `set_ui_state`（既有防抖写通道）；`completed=true` 后不再自动弹。
- **触发与入口**：
  - SessionWorkspace 挂载恢复会话列表后 `get_ui_state` 判定；
  - 无 `onboarding` 记录（全新数据目录）→ 自动弹向导（覆盖层可关，主界面在底层已可用，**不阻塞临时会话**）；
  - `completed=false` → 主窗卡头上方常驻细横幅「继续初始配置 →」，点击重开向导（从记录 step 续走），完成后横幅消失。
- 与 29 号票的接缝：向导内操作反馈（保存失败/测试超时）先 console.warn，29 号票 Toast 落地后改走 Toast（留 TODO 锚点）。

### 测试

- vitest（mock get/set_ui_state、save_provider/test_provider，SettingsPage.test.tsx 已有同款 mock 模式可复用）：
  - 四步导航（前进/回退/单步跳过/整包关闭）与 step 记账；
  - 完成态写入 `onboarding.completed`；跳过后重开从记录 step 续走；
  - 「继续配置」横幅三态渲染（无记录/未完成/已完成）；
  - test_provider 失败时「仍要保存/跳过」分支；test_provider 未注册时降级按钮渲染；
  - ProvidersPage 表单抽取后的既有用例迁移全绿（防两处表单漂移的回归面）。

## 边界（不在本票）

- i18n 文案多语言化（31 号票收尾期执行，向导文案先中文进 zh-CN 基准包）；Key 有效性深度校验（连通测试仅 19 号票口径，不做模型拉取全量校验）；多供应商批量引导。
- 「重新看一遍引导」重置入口（随 03 号票设置页余量，不在本票）；macOS 通知权限引导（SettingsPage 已有通知授权引导条，17 号票面，不动）；项目 roots 快照细节（既有 save_project 语义）。

## 验收

- [ ] 全新数据目录首启自动弹向导，四步走完：语言已写入 config、供应商可用且测试通过、项目已注册、临时会话可正常发消息。
- [ ] 每一步单独跳过均可用；跳过供应商后顶部「继续配置」横幅出现，点击从跳过步续走。
- [ ] 向导打开状态下关闭它直接发消息（临时会话）不被阻塞；向导全程键盘可操作。
- [ ] 二次启动（completed 或已跳过整包）不再自动弹；completed 后横幅消失。
- [ ] 引导不产生脏数据：未保存即跳过 → 无 provider/project 残留；保存过一次再跳过 → 已存项保留。
