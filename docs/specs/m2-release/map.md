# CraftWave M2 正式版 1.0 全程地图

> Label: `wayfinder:map`。本地 markdown tracker，操作约定见 `docs/agents/issue-tracker.md` 的 "Wayfinding operations"。M1 图（`docs/specs/m1-mvp/`）已于 2026-10-29 由用户强制决策收官（见其 map.md Decisions 末行），本图承接全部后续增量。

## Destination

CraftWave M2（正式版 1.0）出口：**用户的日常刚需全部可用**——长会话聊不失忆（上下文压缩）；窗口重载/崩溃后不丢运行内容（attach_run 热重连 + 快照对账）；运行中可排队续发（followUp 后端队列）；AI 可同时干多件事（并行工具执行）；可接 MCP 服务器、可装技能、可派子代理；可上网搜索（SearXNG 本地 → 直接抓取 → Tavily 三通道，见 C-12）；可读 Office/PDF 文档、可发图读图；Thinking 档位/连接测试/网络代理/本地模型齐备；花销可查（用量统计面板 + Inspector 配额卡）；安全纵深到位（AST 命令拦截、内部状态防篡改、项目资源 TOFU 信任、外部目录授权、计划门基线/范围门）；git 状态与变更可视（AI 写操作默认禁）；会话自动起名、可导出 Markdown/JSONL、产物面板可回看；桌面补全（窗口几何记忆、Toast、全局热键、首启引导、中英双语）；macOS 签名公证分发 + 应用内更新（Windows/Linux 维持未签名）。**全 NFR 达标、事件契约测试与回归体系上线。终局关 = 用户日常真实使用稳定顺畅（用户自验，代理不代跑）**，不是"最后一个功能写完"。

## Notes

- **领域**：同 M1——本地优先跨平台桌面 AI Agent；Tauri 2 + rig 0.45（锁版）+ assistant-ui 0.15 + React 19 + Tailwind v4。上游依据：`docs/功能需求文档.md` v1.0（P1 行）、`docs/技术设计文档.md` v2.9（M2 章节字段级：§5.5/§5.7–5.10、§6、§7.7–7.8、§8.3、§8.9–8.10、§9.7–9.9、§10.7、§11.2/11.5/11.6、§12.4–12.5、D16–D19）。
- **票的详细度基准**（2026-10-29 用户要求并已重写）：每张实现票为施工级——`要做什么`（用户价值+切片范围）/`具体做什么`（后端/前端/测试逐条：真实命令名、帧名、事件键、错误码、存储路径、组件与接线点，出处为设计文档章节或代码现状）/`边界`（不在本票）/`验收`（可勾选）。字段级定义缺失的，票内注明"先回写设计文档再实现"。
- **分组执行图景**（七组，票号即组内建议序；组间无硬依赖即可并行领票）：
  - **A 基建欠账（01–03）**：TS 契约自动生成收漂移、错误码补行、M1 设置页烂尾清偿——其余票的地基；
  - **B 会话核心（04–07）**：压缩 / attach_run 热重连 / followUp 队列 / 并行工具——日常使用质感；
  - **C Agent 能力（08–17）**：MCP 客户端 / 技能系统 / 子代理 / web_search 三通道与 web 抓取 / 小工具包 / Office-PDF 读取 / 图片输入——能力面大头；
  - **D Provider 与计量（18–21）**：Thinking 档位与会话偏好 / 连接测试·代理·本地模型 / 用量面板 / 配额卡；
  - **E 安全纵深与 git（22–28）**：AST 围栏与写后检查 / 内部状态保护与审批记忆 / TOFU 信任与外部目录授权 / 计划门 P1 / git 集成 / 标题与导出 / 产物面板与 @ 引用；
  - **F 桌面与收尾（29–34）**：桌面杂项（窗口几何/Toast/热键）/ 首启引导 / i18n（收尾期执行）/ 会话日志 / macOS 签名公证与更新器 / 回归体系与 NFR 收口；
  - **终局关（35）**：用户真实使用稳定验收（blocked by 33+34）。
- **本图承载执行**（沿用 M1 定制，覆盖 wayfinder 默认"计划不执行"）：实现切片以 task 票进图，一票"解决"= 该切片实现完成且验证通过。
- **结论回写**：spike/grilling/research 结论涉及设计文档的，回写《技术设计文档.md》对应章节并勾销附录 C 对应项（已勾：C-2、C-12 均已定）。
- **技能路由**：grilling 票先调 Skill("grilling") 与 Skill("domain-modeling")；research 票调 Skill("research")；prototype 票调 Skill("prototype")。
- **站定偏好**（M1 沿袭 + AGENTS.local.md）：中文思考/回答/提交注释；git 只在用户明确要求时提交、按路径精确 add、Conventional Commits 中文主题；research 结论直接写回票文件，不建 research 分支；并行 worktree 允许多票并跑，合并/push 需用户点头（可整批授权）；pnpm 检查从仓库根跑；MACOSX_DEPLOYMENT_TARGET=13.0；网络问题用 127.0.0.1:7890 代理。
- **会话纪律**：一个会话只解决一张票（research 票除外）；动工前先把票置 `Status: claimed`。
- **M2 用户决策**（2026-10-29）：① 多 Key 池轮换砍单（Out of scope）；② Apple Developer 账号已注册，macOS 签名+公证进范围（HITL 清单在 33 号票），Windows 维持未签名；③ 其余 P1 照单全收，i18n 排收尾期（31 号票）；④ web_search 定案本地优先混合三通道：SearXNG（独立配置组：启用/地址/连通测试）→ 内置直接抓取（默认）→ Tavily BYOK（随 13 号票实现，免费 1000 credits/月），Brave 留位不实现。
- **图表期定稿**：35 票按建议执行序编号——编号即 frontier 默认顺序，并行会话可领任意未阻塞未领票。

## Decisions so far

<!-- 索引：一行一张已关闭票，足够判断相关性，详情点进票看 -->

- [调研：tauri-specta M2 复评（C-2 收口）](tasks/01-research-tauri-specta-recheck.md)：维持"ts-rs 12 生成 payload + 手写键面 manifest"不变，M2 不引入 tauri-specta——2.0 线 RC 已 25 个月未 stable（2.0.0-rc.26/27，specta 同为 RC），是整套契约体系的替代而非补充，不在契约面扩张最大破坏面上动刀；再评触发 = specta 2.0 stable 发布或 M3+ 命令面再翻倍。02 号票按 ts-rs 12.0.1 stable 直接落地，payload 不加 specta 双 derive。
- [调研：Office/PDF 解析 crate 选型](tasks/15-research-doc-crates.md)：三格式首选全纯 Rust、MIT 兼容、dmg 增量约 1–2MB——xlsx→calamine 0.37（回退 umya-spreadsheet）、docx→docx-rs read_docx（回退 docx-rust，兜底 zip+quick-xml）、pdf→pdf-extract 0.12（回退 pdfium-render，其按平台 3.4–3.7MB 动态库仅作质量逃生口）。16 号票三大风险：docx-rs 读侧开放缺陷 #873/#933 须真实样本回归+降级路径；pdf-extract 对不可信输入可能 panic（工具边界 catch_unwind）且非恒等 CMap 中文 PDF 可能乱码（#103）；纯 Rust 无 PDF 表格重建，v1 按线性文本交付。
- [调研：web_search SearchProvider 选型（C-12）](tasks/11-research-web-search-provider.md)：**用户定案（2026-10-29，两轮决策）：本地优先混合三通道**——① 设置页独立 SearXNG 配置组（启用/地址/连通测试），启用且探活成功走本地 JSON API；② 未启用或失败默认内置 DuckDuckGo HTML 直接抓取（走代理）；③ Tavily BYOK 随 13 号票一并实现（免费 1000 credits/月不绑卡、大陆可直连，Key 已配才参与）。优先级钉死 ①→②→③；Brave 留位不实现；MCP 搜索通道随 08 号票共存。

## Not yet specified

- **rig 0.46 stable 换边**：C-6 换边条件（0.46.0 stable 发布且 #2907/#2921/#2910 修复落地）属上游事件，发生时才需决策（预计仅改 Cargo.toml 一行 + 回归）；不立票，出现即处理。
- **触发符联动细节**：FR-01-08 的 @/$// 三触发符共用光标锚定——$ 随子代理票（10）、/ 随技能票（09）落地后，若三符共存有交互余量（菜单优先级、引号路径、与 ghost hint 的 Tab 优先级）另立小票。
- **NFR-03 压测不达标的后果**：34 号票压测若不达标，虚拟滚动本体（C-5 原定 M3）是否提前进 M2 属条件触发决策，届时 grilling。
- **manifest 治理增量**：命令面 31→72+、事件 18→31 键的扩张随 02 号票（ts-rs）定形；若契约测试维护成本超预期，可能有后续治理票。

## Out of scope

- **FR-02-12 多 Key 池轮换**：用户砍单（2026-10-29），不排期；M3 复议。
- **虚拟滚动本体**（FR-01-14）：C-5 定案 M3 引入，M2 只做 NFR-03 压测前置（34 号票）。
- **C-11 浏览器工具**：M3（snapshot 格式选型随 M3 图）。
- **Brave 云搜索**：留 trait 位不实现（C-12 定案），证书/需求到位再议。
- **PRD P2 全量**：成本统计、自定义请求头、斜杠命令面板、会话搜索 FTS5、会话保留期清理、运行总览 Activity、定时任务、后台服务管理、跨进程写协调、OAuth、本地 RAG、任务分级/质量门、会话导出 HTML 分享页等。
- **Windows 签名**：无证书，维持未签名引导口径（FR-16-02 的 Windows 半随证书到位另行决策）；macOS 签名公证范围不打折。
