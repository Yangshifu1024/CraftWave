# CraftWave M2 正式版 1.0 全程地图

> Label: `wayfinder:map`。本地 markdown tracker，操作约定见 `docs/agents/issue-tracker.md` 的 "Wayfinding operations"。M1 图（`docs/specs/m1-mvp/`）已于 2026-10-29 由用户强制决策收官（见其 map.md Decisions 末行），本图承接全部后续增量。

## Destination

CraftWave M2（正式版 1.0）出口：**用户的日常刚需全部可用**——长会话聊不失忆（上下文压缩）；WebView 重载/崩溃且 Rust run 仍存活时恢复运行内容（attach_run 热重连 + 快照对账）；运行中可排队续发（followUp 后端队列）；AI 可同时干多件事（并行工具执行）；可接 MCP 服务器、可装技能、可派子代理；可上网搜索（SearXNG 本地 → 直接抓取 → Tavily 三通道，见 C-12）；可读 Office/PDF 文档、可发图读图；Thinking 档位/连接测试/网络代理/本地模型齐备；花销可查（用量统计面板 + Inspector 配额卡）；安全纵深到位（AST 命令拦截、灰区命令审计、可检查目标的内部状态防篡改、项目资源 TOFU 信任、外部目录授权、计划门基线/范围门）；git 状态与变更可视（AI 写操作默认禁）；会话自动起名、可导出 Markdown/JSONL、产物面板可回看；桌面补全（窗口几何记忆、Toast、全局热键、首启引导、中英双语）；macOS 签名公证分发 + 应用内更新（Windows/Linux 维持未签名）。**全 NFR 达标、事件契约测试与回归体系上线。终局关 = 用户日常真实使用稳定顺畅（用户自验，代理不代跑）**，不是"最后一个功能写完"。

## Notes

- **领域**：同 M1——本地优先跨平台桌面 AI Agent；Tauri 2 + rig 0.45（锁版）+ assistant-ui 0.15 + React 19 + Tailwind v4。上游依据：`docs/功能需求文档.md` v1.0（P1 行）、`docs/技术设计文档.md` v2.9（M2 章节字段级：§5.5/§5.7–5.10、§6、§7.7–7.8、§8.3、§8.9–8.10、§9.7–9.9、§10.7、§11.2/11.5/11.6、§12.4–12.5、D16–D19）。
- **票的详细度基准**（2026-10-29 用户要求并已重写）：每张实现票为施工级——`要做什么`（用户价值+切片范围）/`具体做什么`（后端/前端/测试逐条：真实命令名、帧名、事件键、错误码、存储路径、组件与接线点，出处为设计文档章节或代码现状）/`边界`（不在本票）/`验收`（可勾选）。字段级定义缺失的，票内注明"先回写设计文档再实现"。
- **分组执行图景**（七组，票号即组内建议序；组间无硬依赖即可并行领票）：
  - **A 基建欠账（01–03）**：TS 契约自动生成收漂移、错误码补行、M1 设置页烂尾清偿——其余票的地基；
  - **B 会话核心（04–07、37）**：压缩 / attach_run 热重连 / followUp 队列 / 并行工具 / 监督看门狗（37）——日常使用质感；
  - **C Agent 能力（08–17）**：MCP 客户端 / 技能系统 / 子代理 / web_search 三通道与 web 抓取 / 小工具包 / Office-PDF 读取 / 图片输入——能力面大头；
  - **D Provider 与计量（18–21、38）**：Thinking 档位与会话偏好 / 连接测试·代理·本地模型 / 用量面板 / 配额卡 / 三协议接入欠账（38）；
  - **E 安全纵深与 git（22–28、36）**：AST 围栏与写后检查 / 灰区命令审计 Agent（36，依赖 22+23）/ 内部状态保护与审批记忆 / TOFU 信任与外部目录授权 / 计划门 P1 / git 集成 / 标题与导出 / 产物面板与 @ 引用；
  - **F 桌面与收尾（29–34）**：桌面杂项（窗口几何/Toast/热键）/ 首启引导 / i18n（收尾期执行）/ 会话日志 / macOS 签名公证与更新器 / 回归体系与 NFR 收口；
  - **终局关（35）**：用户真实使用稳定验收（blocked by 33+34）。
- **本图承载执行**（沿用 M1 定制，覆盖 wayfinder 默认"计划不执行"）：实现切片以 task 票进图，一票"解决"= 该切片实现完成且验证通过。
- **结论回写**：spike/grilling/research 结论涉及设计文档的，回写《技术设计文档.md》对应章节并勾销附录 C 对应项（已勾：C-2、C-12 均已定）。
- **技能路由**：grilling 票先调 Skill("grilling") 与 Skill("domain-modeling")；research 票调 Skill("research")；prototype 票调 Skill("prototype")。
- **站定偏好**（M1 沿袭 + AGENTS.local.md）：中文思考/回答/提交注释；git 只在用户明确要求时提交、按路径精确 add、Conventional Commits 中文主题；research 结论直接写回票文件，不建 research 分支；并行 worktree 允许多票并跑，合并/push 需用户点头（可整批授权）；pnpm 检查从仓库根跑；MACOSX_DEPLOYMENT_TARGET=13.0；网络问题用 127.0.0.1:7890 代理。
- **会话纪律**：一个会话只解决一张票（research 票除外）；动工前先把票置 `Status: claimed`。
- **M2 用户决策**（2026-10-29）：① 多 Key 池轮换砍单（Out of scope）；② Apple Developer 账号已注册，macOS 签名+公证进范围（HITL 清单在 33 号票），Windows 维持未签名；③ 其余 P1 照单全收，i18n 排收尾期（31 号票）；④ web_search 定案本地优先混合三通道：SearXNG（独立配置组：启用/地址/连通测试）→ 内置直接抓取（默认）→ Tavily BYOK（随 13 号票实现，免费 1000 credits/月），Brave 留位不实现。
- **图表期定稿**：原 35 票保持编号，新增 36 号灰区命令审计票插入 E 组、37 号监督看门狗插入 B 组、38 号三协议接入插入 D 组，三票在 34 号收口前完成；原票按建议执行序编号——编号即 frontier 默认顺序，并行会话可领任意未阻塞未领票。

- **本轮设计收敛（2026-10-29，Q1–Q44 用户逐项确认）**：最终口径统一见 [spec 的 Implementation Decisions](spec.md#implementation-decisions)，实施票同步补齐测试与验收。重点变化：
  - 长上下文每请求检查预算、失败不静默裁历史；恢复待审批，followUp 失败暂停。
  - 子代理仅主代理派发，全局并发默认 4、设置页可调；独立 worktree 按派发起点回收，失败保留，公共操作由主代理协调。见 [ADR-0001](../../adr/0001-subagent-workspaces.md)。
  - 资源信任覆盖技能附属内容，任务内固定版本；计划批准时明确文件/目录范围。
  - 新增 [36 号命令审计](tasks/36-impl-command-auditor.md)：灰区可模型判低风险放行，硬规则与明确人批优先，失败转人工，最多 15 秒。见 [ADR-0002](../../adr/0002-command-auditor.md)。
  - MCP 结果未知不重放；非 vision 可继续含历史图片的纯文字会话；图片日志仅路径及摘要，删除会话同步删专属日志；更新安装保护运行与队列状态。

- **Q31–Q34 补充收敛（2026-10-29）**：增量摘要超窗分块合并、共用总超时，失败不提交边界（04）；同文件非重叠 edit 合并原子提交，重叠或混入 write 拒后续冲突（07）；补 [37 号看门狗](tasks/37-impl-run-watchdog.md)，停滞计时仅模型请求阶段，34 依赖 36+37；新格式首次升级前可恢复备份、失败暂停，更新说明提示旧版不可读，不做自动降级转换（04/33）。

- **Q35–Q38 补充收敛（2026-10-29）**：ask 混批仅执行 ask、其余明确未执行并待回答后重发（07）；备份迁移权威数据及关联文件，独立目录保存、凭据权限不放宽、关闭应用手动恢复到备份时点（33）；run/会话四口径持久化及恢复、历史模型名快照、日/模型保留 90 天，不新增复杂 run 列表（20）；超大已完成工具批次仅在摘要输入中带来源转普通文本分块，正式配对完整（04）。

- **Q39–Q42 补充收敛（2026-10-29）**：usage 返回时按当时本地日期固定记账、跨日可拆分，失败仍计已知用量（20/27）；会话含主/子/辅助，本轮只主自身、子量单独展示，runs 只主 run（10/20）；恢复旧备份须重核信任/外部授权/审批记忆，队列暂停、旧审批不自动执行（33/23/24/06）；同批多个 ask 只执行源序首个（07）。另按既有 FR-02-07 修正 18 号票：运行中模型/thinking 切换在同一 run 下一次模型请求生效，不能仅在 start_chat 读取偏好。

- **既有需求漏票补齐**：新增 [38 号三协议接入](tasks/38-impl-provider-protocols.md) 归 D 组，承载 FR-02-01 的实际对话与连接测试，接通 18/19；总计 38 票，34 收口依赖 36+37+38。

## Decisions so far

- **Q43–Q44 补充收敛（2026-10-29）**：删除当前会话与专属副本、关联明细，保留无会话关联的日模型数字及完整升级备份；删除确认说明备份残留并给目录入口（20/32/33，[ADR-0003](../../adr/0003-session-deletion-retention.md)）。MCP 新增工具/输入 schema 变化须重新确认，远端 annotations 不提权（08）。最终核对未发现需新增 Q45 的独立产品决策；下述条件触发项保持原有处理时机，功能实现与验收仍按票执行。

<!-- 索引：一行一张已关闭票，足够判断相关性，详情点进票看 -->

- [实现：子代理（10）](tasks/10-impl-subagents.md)：FR-07/§5.9/ADR-0001 落地——7 内置角色（explore/planner/implementer/reviewer/tester/frontend-dev/backend-dev = 系统提示+工具白名单+预算，白名单经 filter_for_tier 同源裁剪、allowed_tools 执行入口过滤，公共操作限制 = 写边界指向 worktree + MCP/委派面不装配）；`subagent` 工具进注册表静态段第 10 位（Meta、description 内嵌角色清单）；`agent/roles.rs` + `agent/sub_registry.rs`（全局并发闸=动态信号量，maxConcurrentSubagents 缺省 4/1–16/save_config 热应用，满额排队不拒绝、热降欠账湮灭）+ `agent/worktree.rs`（HEAD 基线 worktree + 起点差异复制含未提交修改与未忽略新文件，回收按派发时内容指纹判冲突、保留双方写 `*.cw-conflict-<subId8>` 副本、不 commit 不改暂存、abandoned 跨重启保留）；子 run 驱动复用 drive_run（审批挂起经 wait_approval_with 泛化 + resolve_ask 路由链扩展），取消级联 = child_token 树；预算耗尽 rig MaxTurns → partial + E_SUB_BUDGET；帧 +4（subStarted/subFrame/subReport/subDone，父枢纽入环 attach 可重放）+ 事件 +6（sub:* 六键）+ 命令 #58/#59（stop_subagent/load_subagent_history）+ 子历史 meta.json+history.jsonl（histories/subs/）+ worktrees/ lease 索引（§7.7 回写）；Q40 用量= tokens:update 单一出口只计一次、sub:usage 仅子面板、子 run 不占会话互斥；前端 $ 触发符角色菜单（三符互斥）+ subagents store 双通道幂等 + Inspector 子代理 tab（抽屉回看）+ SubagentCard 工具卡 + 设置页并发输入。git2 0.21（default-features=false）引入。检查链七项全绿（Rust 454、前端 252；§5.9/§7.7/§10.6 回写 v2.18）。
- [实现：技能系统（09）](tasks/09-impl-skills.md)：FR-09-02/§8.3 skill 行/§7.7 存储行/§9.8 落地——新建 `knowledge/` 域三模块（skills 发现三来源合并覆盖 内置<全局<项目 + Claude 格式 frontmatter 兼容额外字段忽略 + Q21 整目录版本指纹 sha256 over 路径字典序清单 + symlink 不追随 512 文件/64MiB 上限；trust `trust/project.json` 项目信任存储 remember 值域；commands #48–51 + run 装配信任核验）；skill 工具进注册表静态段第 9 位（Meta 全档放行、description 动态列启用清单随 toggle/reload 下一 run 生效）；Q20 run 快照正文全文固定（活跃 run 不混版本）；projectTrust 第四询问形态（AskPayload 扩展 + run 开始同步询问，allow_once 内存会话集/remember 落盘/deny 不记录，24 号票收敛 revoke）；`AppState.mcp_asks` 泛化 `app_asks`/`AppAskSlot`（mcp 侧薄包装零破坏）；渐进披露注入文本断言不含正文；skills-state.json 启停持久化（§7.7 回写 v2.17）；内置示例技能 resources/skills/hello-craftwave 入库（tauri resources + dev 回退）；前端 SkillsPage（列表/来源徽标/启停/重扫/Markdown 预览）+ `/` 触发符菜单（detectSlashQuery 首字符互斥 + 展开明确指令文本定案）+ SkillCard 正文折叠预览 + AskPanel projectTrust 表单。检查链七项全绿（Rust 378 lib + 39 集成；前端 230）。

- [实现：MCP 客户端接入（08）](tasks/08-impl-mcp-client.md)：FR-08-01/02/04/05/06/08 + §9.7 落地——`src-tauri/src/mcp/` 五模块（config 双作用域同名覆盖 + flatten 未知字段保序 round-trip；pool LRU≤16 连接池 + Starting→Ready→Stopped/Error 状态机 + mcp:status 四态 + generation 防旧握手复活 + 指数退避重连 + stdio 进程组/HTTP reqwest-rustls 双传输 + Q14 结果未知 E_MCP_RESULT_UNKNOWN 不重放；trust TOFU 指纹落 trust/mcp.json + Q44 工具级许可三元组 trust/mcp-tools.json；tools 归一化/碰撞 __2 后缀/反向还原/glob 过滤/进程级注册表单例；commands 八命令 #40–47 + TOFU 询问槽落 AppState.mcp_asks）；注册表动态化（ToolSpec.name → String + origin 段 + lookup_spec 静态+动态合一 + plan 档 MCP 全过滤）；审批门 MCP 分支（未声明 read_only 无许可即便 full_access 挂起；「始终允许」三元组热生效与服务器级 always_allow 解耦——always_allow=true 语义 = 同步自动授权、schema 未确认变更不自动放行）；前端 McpPage（双作用域 JSON 编辑/启停/mcp:status 实时徽标/信任列/测试连接）+ AskPanel mcpTrust 第三形态（sessionId="" 应用级桶）+ McpFallbackCard 兜底卡（part.toolUI===null 判定）+ contract.test 双向断言；§3.3#46/§3.6/§8.1/§8.2/§9.7 回写（keyring 字样按 v2.5 决策改 trust 文件存储）。检查链七项全绿（cargo 362 lib + 前端 210）。

- [实现：监督看门狗与模型停滞检测（37）](tasks/37-impl-run-watchdog.md)：FR-03-07/Q33 落地——`backend/watchdog.rs` 双接缝：工具窗口（`RunWatchdog`，容量 32 签名=工具名+DefaultHasher 参数哈希，挂 `DriveState` 跨挂起延续）turn 边界评估四信号（同签名失败 3 纠偏/5 终止带锚重开复位、无进展 8/14、只读成功重复每满 4 纠偏、同文件分段读每满 10 纠偏）+ 停滞计时（`StallGuard` 随单次模型请求生灭、`select!` biased 竞争、三类增量帧 textDelta/reasoningDelta/toolArgs 刷新、审批/工具/子代理等待结构性豁免、UI 帧/重连不可续命）；纠偏 hidden system 插入请求面 history 不入树、`RunExit::Stalled` → runEnded(error, E_RUN_STALLED) 唯一终态 + run:error 事件 code 带出 + 队列失败暂停（`followup_disposition` 纯函数钉住）；§3.2 增 watchdog 组（stallTimeoutSecs 缺省 300/5–3600 校验/旧配置 serde default 回填/按 run 装配生效）+ 设置页"运行监督"区接线；TurnDriver 泛型化（Model<Chat> → Model<W,T>，方法体零改动）解锁 rig test-utils 剧本假模型 + 自定义悬空/慢速 transport 直驱 drive_run 全链（21 例独立测试：阈值上下界/水位/重置/滑动/停滞触发/增量刷新/工具等待不误杀/终态唯一/副作用不重放/纠偏注入请求断言）；§3.2/§5.7 回写（v2.16）。检查链七项全绿（Rust 375 + 前端 193）。

- [实现：并行工具执行与批次调度（07）](tasks/07-impl-parallel-tools.md)：FR-03-11/FR-04-04 落地——`CancellableExecutor::execute_batch` 批次契约（默认串行实现保测试假执行器零改动，ToolsetExecutor 覆写受控并发）+ `run_batch_tasks` 纯调度核心（JoinSet + 信号量 `tools.maxParallel` 缺省 4/1–16 + 进程级全局写互斥跨会话串行 + 取消停止派发/在飞 join/未派发补 `Cancelled{executed:false}`）；策略门先筛选（Q35 批内含 ask 只执行源序首个、Q42 多 ask 同规、被跳过项不申请审批，E_TOOL_SKIPPED 新错误码回喂"未执行，等待回答后重新决定"）再既有门控预判（Suspend 挂起零帧零执行、恢复重放判定一致）；Q32 同文件写冲突检测——全 edit 组基于同一原内容预演（L1/L2/L3 统一归算归一化 LF 域字节区间），非重叠合并原子提交（备份→降序应用→EOL 还原→单次 rename，失败回滚整组），任一成员匹配失败整组降级源序独立执行，混 write/重叠拒后续（E_INVALID_ARG）；结果统一收集后按源序发帧与 `answer_all`（完成顺序不改序）；§3.2 增 tools 组（RIG tool_concurrency 澄清为自研信号量等价面）/§5.4 ⑤ 受控并发五步/Q35·Q42 契约/§8.4 步骤 4/5/§3.6 回写（v2.15）；rig part 序号 = 响应内流水号实证（e2e 以递增序验源序）。检查链七项全绿（前端含 FrameBuffer 并发交错不串卡回归 2 例）。
- [实现：followUp 后端队列与 steer 插话（06）](tasks/06-impl-followup-backend-queue.md)：C-3/FR-01-07 落地——`inject_run_message`（#21 改写：mode 分叉 steer/followUp + 附着 runConfig 快照，返回 `{queued, mode?, entryId?}`）+ 会话级 followUp 队列（`host/followup.rs`，`sessions/<sid>.queue.json` 原子写 + 惰性恢复，§7.7 增行）；steer 挂 RunHandle 插话队列、drive_run turn 边界（turn>1）经 `SteerSource` 宿主实现消费（canonical 落盘 id=`{runId}:steer:{n}` + run:inject 回执，注入下一拍请求 history）；投递时机 = runEnded 帧发出 + remove 之后（`deliver_followup_head` 复查无活跃 run、撞 E_SESSION_RUNNING 回队重试不丢消息），失败暂停跨重启保留（`resume_followup_queue` #59d 明确继续）、取消清空+文件删除+`run:queue.returned` 交还草稿（含 steer 残余）；新增事件键 `run:queue`（{size, paused, returned?}，32 键触顶）与 run:start/run:inject 实装；前端三态路由（空闲 Enter=发送/运行中 Enter=steer/Alt+Enter=排队，M1 `unstable_enableMessageQueue` 撤除）、自绘队列条（排队徽标/删除/编辑取出/暂停横幅+继续）、steer 时间线「插话」徽标节点（树收养按 entryId 剔除）、自动投递新 run 帧面 = DetachedSink 入环 + run:start → attach 观察态续流（05 号票机制复用）；AppState 全字段共享句柄化 + `share_handles()`；§3.2/§3.3#21/#59d/#59e/§5.5/§7.3/§7.7 回写（v2.14）。检查链七项全绿。
- [实现：attach_run 帧重放与热重连（05）](tasks/05-impl-attach-run.md)：C-1/D16 落地——FrameHub 帧枢纽（`agent/run_manager.rs`，512 帧 `VecDeque<SequencedFrame>` 环形缓冲 + dropped 计数，seq 递增→入环→下发收进枢纽，新增 `SequencedSink` 下发端 trait 使 agent 不依赖 tauri）；sink 装饰链 `EventfulSink（+RunState 同步）→ PersistingSink → HubSink`，压缩帧同枢纽出槽随重放恢复；attach_run（#57）重放与热切换锁内完成保证帧序（sinceSeq=0=全量重放的语义钉死回写 §5.8）、runSnapshot 摘要帧（§3.4 定名，占新 seq 号不进环）兜底窗口耗尽；session_run_snapshot（#20，RunState 增 usage、lastSeq 以枢纽为准）与 session_pending_ask（#59c 回写，AskSlot 留存原始载荷）两个对账命令；前端观察态（`chat/run-reconnect.ts` + `ReconnectOverlay.tsx`：快照 running → attach 重放续流喂独立 FrameBuffer，runEpoch 会话级记账、5s 无帧快照 idle 兜底、runEnded 收口 reloadTick 驱动树重载、覆盖层隔离输入、不触 noteRuntimeNode）+ 待审批恢复（查询命中恢复 AskPanel 原始载荷、resolve E_NOT_FOUND 关闭失效弹窗、重载不自动决定）；§3.3/§3.4/§5.6/§5.8 回写（v2.13）。检查链七项全绿。
- [实现：上下文压缩 compaction（04）](tasks/04-impl-compaction.md)：FR-03-05 落地——StoredCompactionEntry 条目（id=`{owner}:compact:{n}`，Q34 备份守卫 `storage/backup.rs` 首次写入前可恢复备份、失败暂停升级）+ AppConfig compaction 组（0.6 阈值/180s 总超时/专用小模型）；压缩编排（`backend/compaction.rs` 增量+分块摘要共用总超时、`backend/estimator.rs` 宁高勿低估算、每请求预算含回复预留）接两触发点（run 开始前完整编排立即生效 / ask 关闭后落盘面压缩——rig AgentRun 不可回溯的定稿口径）；wire 按最近 cutEntryId 裁剪 + summary 注入（customPrompt → summary 层叠，锚点缺失显式拒绝不静默展开）；失败兜底（可容纳继续/超 E_CONTEXT_OVERFLOW 暂停且输入先落盘保留）；手动 compact_session（#59b，运行中拒绝）；compactDone 帧 + 压缩三事件 + Composer 指示条/Inspector 压缩按钮/时间线分隔提示；compaction 不投影树节点（§6.3 定稿，推翻票面 runMarker 式建议并记录分支语义原因）；§3.2/§3.3/§3.6/§6.1/§6.2/§6.3/§7.3 回写（v2.12）。检查链七项全绿。
- [实现：M1 欠账清偿与设置页打磨（03）](tasks/03-impl-m1-debts-settings-polish.md)：预设清单 ts-rs 生成物单源落地（Rust 权威 + cargo test 导出类型声明与 BuiltinPresets.ts 常量，前端副本与"slug 双侧测试钉住"防线撤除，zhipu 漂移取并集回填）；open_dir 命令实装（open crate 5.4.5，白名单 = 数据/日志/项目数据目录，`~` 后端展开后照走 PathGuard 校验，越界 E_PATH_OUTSIDE_ROOTS）；customPrompt 注入层（`agent.customPrompt` + wire 出口 `with_system_prefix`，§6.2 次序首位：customPrompt → 压缩 summary → 计划，空/未配置零注入）；设置页三处目录入口（关于/日志/工作区与智能体页）与 agent 页 customPrompt 文本框；§3.2/§3.3/§6.2 回写完成。检查链七项全绿。
- [实现：ts-rs 12 契约生成落地与错误码补行（02）](tasks/02-impl-ts-contract-generation.md)：payload 类型单一来源切换 ts-rs 12 生成物（`ui/src/ipc/generated/` 78 文件进 git，cargo test 经清单宏统一导出，types.ts 只再导出）；线帧类型前端对齐 SequencedFrame（生成物如实拆 seq/ts 基座）；镜像别名 ToolOutcomeJson/ThemeName/WorkspaceReadRange 收编为 Rust 同名；漂移演练实证单源（Rust 改字段 → 重新生成 → typecheck 红）；§3.6 补 E_IO 行、变更记录 v2.10；事件载荷族 5 类型仍手写（无 Rust 结构体来源，随对应功能票收编）。检查链七项全绿。
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
