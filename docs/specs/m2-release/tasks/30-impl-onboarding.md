# 实现：首次启动引导

Type: task
Status: resolved

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
- [x] 引导不产生脏数据：未保存即跳过 → 无 provider/project 残留；保存过一次再跳过 → 已存项保留。

## 决议记录（2026-10-30 实现收口）

**实现落点**：前端三新文件——`ui/src/onboarding/OnboardingWizard.tsx`（四步向导覆盖层 + `OnboardingResumeBanner` 横幅 + `resumeStep` 续走步序）、`ui/src/onboarding/onboarding-state.ts`（schema/清洗/判定/记账纯函数）、共享表单层 `ui/src/settings/ProviderFormShared.tsx`（自 ProvidersPage 抽取：字段校验/凭据 newKey 策略/连接测试状态机/全部表单控件单源）；`ProvidersPage` 改为消费共享层（既有 ProvidersPage.test 12 例 + SettingsPage.test 27 例全绿，防漂移红线守住）；`stores/sessions.ts` ui-state 增 `onboarding` 字段（sanitize 清洗后入 store，随既有 300ms 防抖 set_ui_state 通道落盘）+ `setOnboarding` 动作；`SessionWorkspace` 挂载 bootstrap 就绪后 `shouldAutoOpen` 判定一次自动弹、卡头上方常驻横幅（completed=false 时）、条件渲染向导覆盖层。**Rust 侧零改动零新命令**（票面预判成立：test_provider 等 IPC 全部已注册）。

**自动化与 GUI 走查面分界（诚实声明）**——前端 vitest 27 例（onboarding-state 12 + 向导组件 15）已钉住：四步导航/单步跳过/整包关闭与 step 记账、语言步 save_config 写 `config.ui.language`、供应商步 save_provider（newKey 通道）+ test_provider 三分支（连通/探测失败「仍要保存并重测」与「跳过此步」并存且已存项保留/命令未注册降级「稍后在设置中测试」）、保存失败 Toast 停留本步、项目步 select_dir + save_project（名称留空取目录名兜底）、末步「完成」收口 completed=true、横幅三态渲染（无记录不渲染/未完成渲染/已完成不渲染）、未保存即跳过零写调用。**未勾四项均为 GUI 真实运行面，留用户验收**：①首启自动弹的 SessionWorkspace 接线（SessionWorkspace 渲染依赖 assistant-ui 全量资源树，组件级 mock 成本远超收益；判定纯函数 shouldAutoOpen/sanitize 已测）与「临时会话可正常发消息」；②横幅点击续走的真实主窗链路；③关闭向导后直接发消息不被阻塞 + 全流程键盘走查（Esc 整包关闭已显式实现，Tab/Enter 走原生控件语义）；④跨重启二次启动不再自动弹（判定逻辑已测，重启面未自动化）。

**实现口径决策**：
1. **记账语义**：末步（安全简介）无论「完成」还是右上跳过都整包收口 `completed=true`——其后无续走目标，否则落出「重开直接在末步且横幅常驻」的怪状态；整包关闭（×）= `completed=false` + step 保持当前步（重开续走），关闭后不再自动弹（有记录即不弹，符合票面「已跳过整包不再自动弹」）。单步跳过打 `skippedAt` 时间戳，正常前进不打。
2. **供应商步测试时序**：test_provider 按 providerId 探测已保存配置，故向导交互为「保存并测试」一钮（先 save_provider 落盘再探测）——探测失败时配置已保留，「仍要保存并重测」= 改后重提（同 id upsert）、「跳过此步」= 已存项保留并前进，正好承接验收「保存过一次再跳过 → 已存项保留」。
3. **共享层接缝**：`runConnectionTest` 增可选 `onReject` 接管参数（返回 true = 调用方已处理不写 error 槽）——设置页现状（reject 兜底 protocol 分类）零行为变化，向导借此实现 test_provider 未注册降级；`onCredentialMode` 改可选（向导恒新增语义）。`ProvidersPage` 对外保留 `probeErrorLabel`/`ConnectionTestState` re-export，既有 import 面不断。
4. **Toast 接缝收编**：票面写「先 console.warn，29 号票 Toast 落地后改走 Toast（留 TODO 锚点）」——29 号票已先行合入，直接走 `pushToast`（语言保存失败/供应商保存失败），console.warn 保留为诊断面，无遗留 TODO。
5. **项目步「跳过 = 用临时会话」**：纯文案提示（临时会话 roots=数据目录 workspace/ 的 C-8 语义为后端既有行为，跳过无需任何前端动作）。
6. **语言步与 31 号票联动**：向导界面文案中文先行（31 号 i18n 落地 zh-CN 基准包后自然接管），本步只落 `config.ui.language` 配置值；en-US 文案（如「Use English for the interface.」）为该步局部说明，不扩为全量 i18n。

**检查链**：七项全绿——cargo fmt --check / build / clippy --all-targets / test（lib 803 + e2e 46 + 其余集成全过，MACOSX_DEPLOYMENT_TARGET=13.0）；pnpm typecheck / test（48 文件 510 例）/ lint（Biome 无告警）。已知无关抖动（thread-rendering 偶发、SkillRegistry 单例时序）本轮未复现、未追修。

**遗留**：无阻塞项。「重新看一遍引导」重置入口按票面边界不属本票（随 03 号票设置页余量另议）。
