# 实现：i18n（zh-CN / en-US）

Type: task
Status: open

## 要做什么

FR-13-09 + FR-14-04 语言项（PRD 行 537/557，出处 §10.8/NFR-19）：

- 界面全量双语——zh-CN 为基准语言包（从已落地 UI 抽词），en-US 全量翻译。
- 语言跟随设置项即时生效，并与首启引导语言选择（30 号票）联动。
- `ai_language` 偏好接线到模型侧（控制模型回复语言，缺省跟随界面语言）。
- NFR-19「所有 UI 文案经 i18next，新增文案不得硬编码」自此成为门禁。

**执行约束（保留原文）**：收尾期执行——多数 UI 票（08–30 的页面/卡片/对话框）落地后再抽词，避免反复翻译；Rust 侧命令/错误/通知文案的 i18n 口径在本票一并定案。

## 具体做什么

### 后端（src-tauri/）

- **Rust 侧用户可见文案定案与落地**：
  - 范围一：托盘菜单（`host/shell.rs` install_tray 的「显示主窗口/退出 CraftWave」两项）与系统通知正文（`host/notify.rs` NotifyGate）。
    - 实现：按 `config.ui.language` 查 Rust 侧字符串表——新模块 `host/strings.rs`（`fn t(key, lang) -> &str`，zh/en 两表，两表 key 集合一致性单测钉死）；
    - 即时性：语言切换后下次事件/通知触发时读最新 config 缓存（共享缓存已有）；托盘菜单在 save_config 钩子处重建一次。
  - 范围二：**错误面口径（本票定案，回写 §3.6 注记）**——
    - `E_*` 错误码机器面不变；错误 message 归一为**英文技术串**（现状多为中文——逐个过 `util/errors.rs` 与各命令报错点，清单进 Answer）；
    - 前端展示层按错误码 key 映射本地化文案（`errors.json`），映射缺失回退原始 message 不空白；
    - runEnded.error.message 同口径（backend 错误分类面 §11.3 产出点同步过一遍）。
  - 范围三：**ai_language 注入点**——
    - `backend/wire.rs` wire 重建 system 前缀（§6.2 系统注入既有落点，planSnapshot/custom prompt 同通道）；
    - `config.ui.aiLanguage`（`Option<String>`，字段与设置页输入框均已存在：types.rs L188 / SettingsPage L631）非空 → 注入「请始终使用 <ai_language> 回答」行；
    - 空 = 跟随 `config.ui.language`（zh-CN→中文固定一行、en-US→英文固定一行）；
    - 注入排序与 04 号票压缩 summary 前缀的关系（建议：custom prompt → ai_language → compaction summary）在 §6.2 一并回写钉死。

### 前端（ui/）

- **框架**：`i18next` + `react-i18next`（前端标准库，非必要不自研）：
  - 初始化挂 `main.tsx`（App 渲染前完成，默认 zh-CN 兜底防裸 key 闪现）；
  - 资源文件 `ui/src/i18n/zh-CN/`、`ui/src/i18n/en-US/`，按域拆 json：`workspace`（SessionWorkspace/Thread/Composer）/ `settings` / `inspector` / `toolcards` / `dialogs` / `toasts` / `onboarding` / `errors`——拆分随抽词自然成形；
  - key 规约 `<域>.<语义>`（如 `settings.providers.title`）；插值用 i18next `{{var}}` 语法（工具调用次数/时长等现有模板串）。
- **抽词流程**：
  1. 盘点硬编码中文面（SessionWorkspace.tsx ≈997 行、SettingsPage.tsx、ToolCards.tsx、AskPanel.tsx、Inspector.tsx、OnboardingWizard.tsx、Dialog/确认框、Toast 文案、`stores/` 内用户可见消息）；
  2. 逐文件替换为 `t()`（组件 `useTranslation`；store/工具函数内 `i18n.t`）；
  3. 替换即产出 zh-CN 基准包（key=英文语义路径，value=现中文），en-US 全量翻译同一 PR 提交（key 集合必须一致）；
  4. 生效链：App 启动读 `config.ui.language` 初始化 → 设置页语言选择（该项已存在）→ `i18n.changeLanguage` 即时生效并 `save_config`；30 号票向导第 1 步写同字段自动联动。
- **CI key 完整性检查**（§10.8 明文要求），新增 vitest 两测：
  - zh/en 两包深度 key 集合双向 diff 为空；
  - 硬编码回退扫描：启发式扫描 `ui/src` 非测试文件的字符串字面量中 CJK 字符，白名单豁免（注释、正则、i18n 资源目录自身），新增硬编码即红（NFR-19 门禁）。

### 测试

- 前端 vitest：语言包 key 完整性（上述两测）；语言切换冒烟（changeLanguage 后关键组件文本断言——设置页标题/Composer 占位/工具卡按钮/确认框）；errors.json 错误码映射覆盖（对照 §3.6 错误码注册表抽样）；插值串渲染。
- Rust：`host/strings.rs` 两语言 key 集合一致；ai_language 注入单测（非空/空跟随/en 三态 + 前缀顺序断言）；错误 message 英文化抽查（代表性报错点 5 个，断言输出无 CJK）。
- 人工走查（验收面）：en-US 全量切换无中文残留（设置页/托盘菜单/系统通知/工具卡/对话框/向导/Toast），zh-CN 回切同理。

## 边界（不在本票）

- 新增第三语言（结构就位，翻译不排期）；docs/ 与票面文档不翻译；日期/数字本地化格式（Intl）不做。
- Rust 日志内部诊断串（非用户可见）不抽词；MCP/技能等外部来源动态文案（SKILL.md 内容本体、MCP 工具 description）不翻译，仅应用 chrome 文案。
- 复数/性别等 ICU 高级语法（中英场景少，`{{var}}` 插值够用，记边界）。

## 验收

- [ ] 设置页切 en-US：全界面无中文残留（含托盘菜单与系统通知正文）；切回 zh-CN 同理。
- [ ] zh/en 语言包 key 集合一致有测试钉住；故意删一个 en key → CI 红。
- [ ] 新增硬编码中文字面量的文件被扫描测试拦下（NFR-19 门禁生效）。
- [ ] ai_language 填「English」→ 模型回复英文；清空 → 跟随界面语言；注入行位置与排序按 §6.2 回写口径（测试断言）。
- [ ] 前端可见错误提示按错误码本地化，未知错误回退原始 message 不空白。
- [ ] §3.6 错误面口径、§6.2 ai_language 注入点、§10.7 引导联动注记回写完成。
