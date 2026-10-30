# 实现：技能系统（SKILL.md 发现、渐进披露、/ 触发符、管理页）

Type: task
Status: resolved

## 要做什么

用户可装「技能」教 agent 干专项活：一个技能一个目录一份 SKILL.md，模型平时只看到一句话简介（不撑上下文），点到才加载正文；`/` 菜单快速触发，设置页可浏览、启停、重扫。兼容 Claude 技能目录格式，存量技能可直接用。覆盖 FR-09-02 + §8.3 `skill` 工具行 + §7.7 存储行 + FR-14-04 禁用技能设置项。

## 具体做什么

### 后端（src-tauri/）

- 新模块 `knowledge/skills.rs`（knowledge 域随本票新建，§2.2 域清单已有）：
  - **发现**：三来源扫描——内置（随应用资源分发）、全局 `~/.craftwave/skills/`、项目 `<home>/.craftwave/skills/`（§7.7 存储行）；每技能=目录 + `SKILL.md`（YAML frontmatter `name/description`）；**兼容 Claude 技能目录格式**：frontmatter 额外字段忽略、目录内附加资源文件按相对路径随正文可用；frontmatter 索引常驻内存（重扫才重建）；**覆盖优先级 内置 < 全局 < 项目**（同名后者胜，来源标记进 skill_list）。
  - **`skill` 工具注册**（§8.3 M2 表，Meta 能力）：参数 `{ name, args? }`（args 可选 string 透传）；结果=该技能 SKILL.md 正文全文（compact 截断同 §8.2）+ 头部标注「以下是技能 X 的正文」；工具 description 动态列出当前启用技能名清单（注册表重建时机随 toggle/reload）。
  - **技能整体信任（2026-10-29 用户确认 Q21）**：信任版本覆盖 SKILL.md 及随附脚本、模板等资源，不能只核验说明文件；附属资源变化同样在下一 run 重新核验。目录清单、路径边界与版本摘要算法实现前先回写 §9.8；仍按 Q20 固定 run 内版本。
  - **版本一致性（2026-10-29 用户确认 Q20）**：run 开始时固定技能版本集合，description 与按需加载的正文必须同版本；重扫可更新管理页与下一 run 使用的版本，不能使活跃 run 混用旧描述和新正文。旧版本不可得则停止相关加载并提示；下一 run 加载新版本前核验信任，安全权限撤销立即生效。
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

- 技能整体版本：SKILL.md 不变而附属脚本/模板变化时，下一 run 仍须重核信任；活跃 run 不混用新附属资源。

- 版本一致性：run 中修改 SKILL.md 并 reload 后，活跃 run 仍使用原版本 description/正文；下一 run 重核新版本信任；原版本不可取得时停止相关加载，不混用版本。

- 单测：frontmatter 解析（含 Claude 格式真实样例：额外字段/长 description）、三来源同名覆盖优先级、skills-state.json round-trip、渐进披露注入文本生成（仅 name+description，断言不含正文）。
- 集成（真实技能目录 fixture，最小技能随仓库测试资源入库）：description 进系统提示、`skill` 调用回正文、toggle 后不再注入且调用回禁用提示、reload 感知新增目录。
- 信任路径：未信任项目技能首用弹确认、拒绝后 E_PROJECT_UNTRUSTED、记住后免问。
- 前端 vitest：`/` 菜单触发检测与 @ 菜单互扰用例（同文本混合 @ 与 / 各归其菜单）、技能页列表/启停/预览交互。

## 边界（不在本票）

- 项目资源信任完整机制（变更重问、指纹存储收敛——24 号票）；斜杠命令面板与内置命令（FR-01-17 P2）；技能安装器/市场；M3 memory/RAG 同域资源。

## 验收

- [x] 装 Claude 格式技能目录到项目 `.craftwave/skills/`，skill_list 三来源可见且项目覆盖全局同名
- [x] 仅 description 常驻上下文（注入文本不含正文），`skill` 工具调用按需加载全文
- [x] `/` 菜单可选技能，与 @ 菜单、ghost hint 互不干扰
- [x] 设置页启停/重扫/预览可用，重启后禁用态保留
- [x] 未信任项目技能首用弹信任确认，拒绝后 E_PROJECT_UNTRUSTED

## 决议记录（2026-10-30 实施完成）

**设计文档回写（实现前，§7.7/§9.8，文档版本 v2.17）**：

- `skills-state.json` 路径与 schema（票面点名的字段级缺口）：数据目录根 `~/.craftwave/skills-state.json`，`{ version: 1, disabled: string[] }`，util/atomic 原子写；文件缺失/损坏按全启用处理（宁缺勿炸）。
- §9.8 技能信任与版本字段级细则：三来源目录清单（内置=打包态 `<Resource>/resources/skills/`、开发态回退源码树 `src-tauri/resources/skills/`；全局 `<data>/skills/`；项目 `<项目主目录>/.craftwave/skills/`）、frontmatter 校验（name ≤64 字符 `[a-zA-Z0-9_-]`、description 必填、额外字段忽略，name 不要求等于目录名——以 frontmatter 为准，覆盖与启停按它寻址）、路径边界（symlink 不追随、每技能 ≤512 文件 / 64MiB 参与指纹，超限不入索引）、Q21 版本摘要算法（`sha256("craftwave-skill-v1|" + name + "|" + Σ(相对路径=sha256(文件)) )` 按路径字典序）、projectTrust 载荷字段级（name/scope/fingerprint 前 16 位/options，应答走 `AskAnswer::Approval` 不新增协议形态）。

**票内定案**：

1. **模块落位**：新建 `knowledge/` 域（§2.2 域清单既有）——`skills.rs`（发现/覆盖/指纹/注册表单例/skill 工具规格/run 快照）、`trust.rs`（`trust/project.json` 存储）、`commands.rs`（#48–51 + run 装配面信任核验）。tools→knowledge 反查（`skill_spec` 经 `SkillRegistry::global()` 进程级单例现读启用清单生成动态 description，与 08 号票 tools→mcp 反查同精神）；build_registry 每次现算，toggle/reload 后下一 run 生效。
2. **run 内版本固定（Q20）**：start_chat 装配时拍 `SkillSnapshot`（启用+已信任技能的正文全文进内存），`skill` 工具只读快照不触盘；reload/toggle 只影响管理页与下一 run。「旧版本不可得」分支不触发（快照持有全文恒可得）——若未来改按需读盘须重引入该分支（§9.8 已记录）。
3. **信任接缝最小版（24 号票收敛点）**：`projectTrust` 询问在 run 开始装配时同步等待（用户主动发消息触发，仿 08 号票 MCP TOFU）；`remember` 落 `trust/project.json`（decision 值域当前仅 `"remember"`，revoke/撤销 UI 随 24 号票）；`allow_once` 记进程内存会话信任集（同会话下一 run 免问，重启失效）；`deny` 不产生记录（TOFU 同 MCP 口径，下一 run 重新询问，拒绝提示引导去设置页禁用）。**应用级询问桶泛化**：08 号票的 `AppState.mcp_asks`/`PendingTrustAsk` 泛化为 `app_asks`/`AppAskSlot`（backend/ask.rs 承载），resolve_ask 的 run 槽未命中兜底收敛为 `AppState::resolve_app_ask`；mcp 侧保留 `resolve_trust_ask` 薄包装与 `PendingTrustAsk`/`TrustAskMap` 类型别名，08 号票测试面零破坏。
4. **项目作用域锚定**：设置页与 run 装配均无会话参数——项目技能根锚定「当前活跃会话的项目」（复用 08 号票 `mcp::commands::project_context_of` 同一解析面）；每次 run 装配与命令调用都全量重扫三来源（技能目录小，成本可忽略）。
5. **skill 工具执行面**：Meta 能力全档放行（§8.5 既有矩阵，tests/tools_registry_test 同步钉死 9 工具）；结果头部标注「以下是技能 X 的正文」，超长截断复用 `tools::truncate_middle`（§8.2 同口径头 4KB+尾 8KB）；边车 data={name, body} 供卡片正文预览；未信任调用回 `E_PROJECT_UNTRUSTED`（错误码常量随本票首次实装进 util/errors.rs）、未知技能回 `E_INVALID_ARG` 带可用清单。
6. **前端 `/` 菜单插入形态**（票面定案授权）：选中展开为「请用 skill 工具加载技能 X 并按其指引行动 」+ 保留用户后续文本（剥首段空白防双空格）；`detectSlashQuery` 与 detectAtQuery 同构且首字符互斥（光标前最近触发符归主，`/` 须行首或空白后——排除路径文本误触），键位优先级并入 §10.6 既有链（菜单 > ghost hint）；技能清单挂载时拉一次，不轮询。
7. **内置示例技能**：`src-tauri/resources/skills/hello-craftwave/`（SKILL.md + 附加资源 greeting.txt）随票入库，兼作 Claude 格式兼容样例与集成测试资源；`tauri.conf.json` bundle.resources 增 `resources/skills/**/*`。
8. **已知无关抖动**：`ui/src/chat/thread-rendering.test.tsx`「建议填入实际 composer」用例偶发失败，复跑即绿（按任务说明未追修），最终全套复跑 230/230 通过。

**检查链（2026-10-30 全部亲自重跑通过）**：cargo fmt --all -- --check ✓；cargo build ✓；cargo clippy --all-targets 0 警告 ✓；cargo test 全目标 8 个 ok（lib 378 + 集成 backend_e2e 30 + tools_registry 5 等）✓；pnpm typecheck ✓；pnpm test 27 文件 230 用例 ✓；pnpm lint（Biome）✓。
