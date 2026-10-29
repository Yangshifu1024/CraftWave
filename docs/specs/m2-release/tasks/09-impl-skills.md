# 实现：技能系统（SKILL.md 发现、渐进披露、/ 触发符、管理页）

Type: task
Status: open

## 要做什么

用户可装「技能」教 agent 干专项活：一个技能一个目录一份 SKILL.md，模型平时只看到一句话简介（不撑上下文），点到才加载正文；`/` 菜单快速触发，设置页可浏览、启停、重扫。兼容 Claude 技能目录格式，存量技能可直接用。覆盖 FR-09-02 + §8.3 `skill` 工具行 + §7.7 存储行 + FR-14-04 禁用技能设置项。

## 具体做什么

### 后端（src-tauri/）

- 新模块 `knowledge/skills.rs`（knowledge 域随本票新建，§2.2 域清单已有）：
  - **发现**：三来源扫描——内置（随应用资源分发）、全局 `~/.craftwave/skills/`、项目 `<home>/.craftwave/skills/`（§7.7 存储行）；每技能=目录 + `SKILL.md`（YAML frontmatter `name/description`）；**兼容 Claude 技能目录格式**：frontmatter 额外字段忽略、目录内附加资源文件按相对路径随正文可用；frontmatter 索引常驻内存（重扫才重建）；**覆盖优先级 内置 < 全局 < 项目**（同名后者胜，来源标记进 skill_list）。
  - **`skill` 工具注册**（§8.3 M2 表，Meta 能力）：参数 `{ name, args? }`（args 可选 string 透传）；结果=该技能 SKILL.md 正文全文（compact 截断同 §8.2）+ 头部标注「以下是技能 X 的正文」；工具 description 动态列出当前启用技能名清单（注册表重建时机随 toggle/reload）。
  - **渐进披露**（FR-09-02）：系统提示仅注入「技能名 + description」一行清单（启用项）；正文只经 `skill` 工具按需进上下文。
  - **信任接缝**（§9.8）：全局与项目技能均属信任资源——首用未信任技能 → `AskPayload{kind:"projectTrust"}` 确认（接 24 号票机制；24 未落地则最小版：AskSlot 三选项 + 决策写 `trust/project.json` 同构条目，票面记录收敛点）；拒绝 → 不注入清单且 `skill` 调用回 `E_PROJECT_UNTRUSTED`（§3.6 M2 错误码行已定义，随本票首次实装）。
- 命令 4 个（§3.3 #48–51 逐字）：`skill_list {} → {name, source, enabled, description}[]`、`skill_toggle {name, enabled} → void`、`skill_reload {} → void`（重扫三来源并重建索引）、`skill_inspect {name} → {markdown}`（正文预览，不走工具/不进上下文）。
- 启停持久化（FR-14-04「禁用技能」）：落 `~/.craftwave/skills-state.json`（禁用名清单，原子写复用 util/atomic）——**该文件路径与 schema 设计文档未给，先回写 §7.7 再实现**。

### 前端（ui/）

- 设置页技能组（§10.7 M2 清单）：`skill_list` 列表（名称/来源徽标：内置·全局·项目/启停开关 `skill_toggle`/一句话描述）、「重新扫描」按钮（`skill_reload`）、行点击 `skill_inspect` 弹 SKILL.md 预览（Markdown 渲染，只读）。
- **`/` 触发符**（FR-01-17 的技能半边；P2 斜杠命令面板不做）：`chat/composer-keys.ts` 增 `detectSlashQuery`（与 detectAtQuery 同构：光标锚定、往回到 `/` 无空白）→ 技能菜单（skill_list 驱动，↑↓ 导航、Enter/Tab 选中）；选中插入形态票内定案——建议展开为明确指令文本「请用 skill 工具加载技能 X」+ 用户后续输入，避免依赖模型对裸 `/名` 的理解；键位优先级并入 §10.6 既有链：触发符菜单（`/`、`@`、`$` 按首字符互斥触发）> ghost hint。
- skill 工具卡：`chat/ToolCards.tsx` + `chat/toolkit.ts` 增卡（技能名 + 加载状态 + 正文折叠预览），CW_TOOL_PARAMETERS 增 skill 行与 manifest 双向锁定。
- 契约测试：manifest 增 #48–51 命令与 skill 工具行。

### 测试

- 单测：frontmatter 解析（含 Claude 格式真实样例：额外字段/长 description）、三来源同名覆盖优先级、skills-state.json round-trip、渐进披露注入文本生成（仅 name+description，断言不含正文）。
- 集成（真实技能目录 fixture，最小技能随仓库测试资源入库）：description 进系统提示、`skill` 调用回正文、toggle 后不再注入且调用回禁用提示、reload 感知新增目录。
- 信任路径：未信任项目技能首用弹确认、拒绝后 E_PROJECT_UNTRUSTED、记住后免问。
- 前端 vitest：`/` 菜单触发检测与 @ 菜单互扰用例（同文本混合 @ 与 / 各归其菜单）、技能页列表/启停/预览交互。

## 边界（不在本票）

- 项目资源信任完整机制（变更重问、指纹存储收敛——24 号票）；斜杠命令面板与内置命令（FR-01-17 P2）；技能安装器/市场；M3 memory/RAG 同域资源。

## 验收

- [ ] 装 Claude 格式技能目录到项目 `.craftwave/skills/`，skill_list 三来源可见且项目覆盖全局同名
- [ ] 仅 description 常驻上下文（注入文本不含正文），`skill` 工具调用按需加载全文
- [ ] `/` 菜单可选技能，与 @ 菜单、ghost hint 互不干扰
- [ ] 设置页启停/重扫/预览可用，重启后禁用态保留
- [ ] 未信任项目技能首用弹信任确认，拒绝后 E_PROJECT_UNTRUSTED
