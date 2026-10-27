# CraftWave M1 MVP 全程地图

> Label: `wayfinder:map`。本地 markdown tracker，操作约定见 `docs/agents/issue-tracker.md` 的 "Wayfinding operations"。

## Destination

CraftWave M1 MVP 出口准则达成：P0 需求全集实现，NFR-01/02/06/09/12 达标，三平台 CI 打包通过（FR-16-01），并在 macOS 上以真实项目连续使用 1 周无数据丢失。地图最后一站是"1 周真实使用验收"这一关，不是"最后一个功能写完"。

## Notes

- **领域**：本地优先跨平台桌面 AI Agent；Tauri 2 + rig（进程内嵌）+ assistant-ui + React 19 + Tailwind v4。上游依据：`docs/功能需求文档.md` v1.0（PRD）、`docs/技术设计文档.md` v2.0（P0 字段级，开发的直接依据）、`docs/数据存储调研.md`。
- **本图承载执行**（用户确认，覆盖 wayfinder 默认"计划不执行"）：实现切片以 task 票进图，一票"解决"= 该切片实现完成且验证通过。
- **Spike 票结论必须回写**《技术设计文档.md》对应章节，并勾销其附录 A（S 验证点）与附录 C（未决问题）对应项。
- **技能路由**：grilling 票先调 Skill("grilling") 与 Skill("domain-modeling")；research 票调 Skill("research")；prototype 票调 Skill("prototype")。
- **站定偏好**：中文交流；AGENTS.md 优先（仅用户明确要求时 git 提交、默认分支不直提、按路径精确 add、Conventional Commits 中文主题）；research 结论直接写回票文件，不建 research 分支。
- **会话纪律**：一个会话只解决一张票（research 票除外）；动工前先把票置 `Status: claimed`。
- **图表期定稿**（grilling 决策，已固化进本图结构）：终点口径 = M1 出口准则；地图承载执行；M0 Spike 并入本图为第一批票；实现切片按关键路径阶段纵切（非 FR 域横切）。

## Decisions so far

<!-- 索引：一行一张已关闭票，足够判断相关性，详情点进票看 -->

- [调研：TS 契约生成方案（C-2 前置）](tasks/02-research-ts-contract-generation.md)：键面保留 manifest 契约测试，payload 类型 M1 即引入 ts-rs 12（tauri-specta 维持观望）；NFR-12 双向扫描任何路线都不可删。
- [调研：rig 0.45 稳定版 vs 0.46-rc.1 锁版底料（C-6 前置）](tasks/03-research-rig-version-choice.md)：锁 0.45.0 为主底料，0.46.0-rc.1 仅平行验证；两版 API 面签名级一致，大断层在 0.44→0.45，从 0.45 入场即绕过。
- [S6 最小骨架与三平台构建验证（含 C-7）](tasks/04-spike-s6-scaffold-platform.md)：骨架落地（tauri 2.12.5 × React 19 × assistant-ui 0.15.31 × Rust 1.97.0），三平台 CI 绿；C-7 兼容无需退化；CSP 收紧版实测落地；工具链组件须在 rust-toolchain.toml 显式声明。
- [修正 AGENTS.md 构建检查段（Go 残留 → Tauri 工具链）](tasks/01-fix-agents-md-buildchain.md)：检查链换为 cargo 三件套（src-tauri/）+ 前端三项（ui/），scope 改 §2.2 九模块，CGO 与旧模板目录清空；骨架落地时若布局有出入需回头同步。
- [S2+S3 rig 流事件保真与手动工具循环（含 C-6 收口）](tasks/05-spike-s2s3-rig-stream-loop.md)：两者均可行——载体 `rig::run::AgentRun`（sans-IO）+ CallTools 整只 run 序列化审批挂起（含跨重启）；锁 0.45.0（双版本零改动实证）；NFR-02 余量 >35ms；帧协议增 `ts?`/callId 同源/节流语义钉死；**附带发现 rig MSRV=1.97.1，工具链已抬升**。
- [S1 assistant-ui 镜像工具桥接](tasks/06-spike-s1-mirror-tools.md)：可行，不退 D11；唯一修正：local runtime 无 execute 管线，改为 adapter 直写 result；`makeAssistantToolUI` 已废弃、`Tools` 是 resource 工厂、partial JSON 依赖 assistant-stream 显式声明（§10.3 已重写）。
- [S4 消息树持久化 roundtrip（含 C-4 钉死）](tasks/07-spike-s4-tree-roundtrip.md)：roundtrip 通过（编辑/重生成/分支切换逐节点一致，vitest 11 例）；C-4 定案（headId/messages 结构钉死，§6.1 增 `status?`）；D12 复核 rig-memory 直接承载不可行，决策维持并增补 redact 复刻点。
- [决策：C-9/C-10 默认方案确认](tasks/09-grilling-c9-c10-defaults.md)：C-9 确认默认——后台小调用计入会话累计、不计轮次、失败静默；C-10 推翻默认——M1 即 encoding_rs+chardetng 编码自动判断（与 agent read 共用解码器），512KB 分页读取，write/edit 一律 UTF-8，GBK 不再缓 P1。
- [S5 取消传播全链路](tasks/08-spike-s5-cancel-propagation.md)：可行——select! 流停 0.03–0.06ms、含进程组收割 6–8ms，全链远低于 1s；标记语义钉死（被收割/未执行补 E_CANCELLED、已完成保留不回滚、runEnded(cancelled) 必为收齐后最后一帧，帧协议零修订）；Windows Job Object 方案 MSVC target 可编译性已验。
- [实现：storage 读写器与会话树命令](tasks/10-impl-storage-session-tree.md)：分段 JSONL 读写器（封口/指纹/尾段容错）+ 原子写 + save/load_session_tree 命令与 manifest 契约测试落地，Rust/前端 roundtrip 全绿，§6/§7 无出入。
- [实现：工具系统只读/写切片](tasks/11-impl-tools-file-slice.md)：util 共用解码器（encoding_rs 0.8.42+chardetng 1.0.0，BOM→严格 UTF-8→探测→最佳猜测）+ safety（safe_join/围栏 P0 黑白名单/权限四档矩阵）+ tools 五工具（read/write/edit/list_files/grep）与注册表钉死，90 例测试，与 §8 无出入（细化记录见票 Answer）。

## Not yet specified

- **工程骨架随开发推进的增量维护**——最小骨架与三平台 CI 已随「S6 最小骨架与三平台构建验证（含 C-7）」落地，无独立决策点；后续仅随票增量维护（依赖升级、CI 注解治理等顺手做，不单独立票）。
- **单会话核心链路**——start_chat → rig 流 → Channel 帧 → assistant-ui 渲染、打字机、思考流、运行过程实时展示、取消 UI。backend 形态（AgentRun 步进 + 序列化审批挂起）、帧协议修订（ts/callId 同源/节流语义）已随「S2+S3」钉死、镜像工具渲染路线已随「S1」钉死；剩余未知仅「S5 取消传播全链路」（08 号票，已解锁），其结论落地后即可按关键路径切成实现票。
- **工具系统与安全审批**——9 个 P0 工具（read/write/edit/list_files/grep/command/plan/ask/suggest）、权限四档、审批门、围栏 P0（高危黑名单+只读白名单）、safe_join 路径边界、结果双通道、工具卡片 UI。工具卡片渲染路线已随「S1」钉死（§10.3），审批挂起/恢复机制已随「S2+S3」钉死。
- **持久化与会话树**——append-only 分段 JSONL、原子写、树 roundtrip 工程化、分支/编辑/重生成的完整落地、崩溃恢复。roundtrip 与 C-4 结构已随「S4」钉死（§6.1/§6.3/§7.2 定稿），**已无不依赖未知项**，实现切片（storage 读写器 + save/load_session_tree 命令）随时可立票。
- **多会话与项目**——多 Tab 并行、临时/项目会话双形态、项目注册表、workspace 读写与路径搜索命令。
- **Provider 管理与设置**——CW schema v2、keyring 提交协议、内置供应商预设、自定义 OpenAI 兼容端点、config 协调器、设置页结构。
- **桌面体验**——三栏布局、Composer、自绘标题栏、托盘+后台运行+系统通知直达会话、快捷键体系、主题、单实例、滚动跟随。
- **三平台打包与 1 周真实使用验收**——打包矩阵（dmg/nsis/AppImage+deb）、验收剧本（真实项目、记录方式）、未决项 C-8（临时会话 roots 边界）安全复判在此毕业成票。

## Out of scope

- **M2（P1）全部增量**：MCP、技能系统、子代理、上下文压缩、看门狗、followUp 后端队列、多 Key 池轮换、AST 围栏、SSRF 防护、项目信任（TOFU）、MCP 启动信任、外部目录授权、Office/PDF、全局热键、i18n、更新器与签名、用量面板、Inspector 配额卡、会话标题自动生成等。M2 临近时另立图。
- **M3（P2）/ P3**：本地 RAG、任务分级/质量门、浏览器工具、成本统计、文件树面板、定时任务、多窗口、OAuth 登录、自学习闭环、语音/视频等。
- **未决项 C-11**（浏览器工具 snapshot 格式，M3）、**C-12**（web_search SearchProvider 选型，M2）。
- 注：M1 范围内的 steer 前半（P0）、围栏 P0 黑白名单在本图内；它们各自的 M2 后半增量（followUp 完整版、AST 围栏）不在。
