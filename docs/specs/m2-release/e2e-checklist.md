# M2 发版前 e2e 人工走查清单

> Label: `m2:e2e-checklist`。34 号票交付物（§12.5 把 GUI 人工清单扩页排在 M3，M2 发版需要，本票提前承载）。
> 用法：发版前三平台（macOS / Windows / Linux）各过一遍；35 号票「用户真实使用验收」以此为前置。每条 = 链路步骤 + 预期现象 + 勾选格。
> 约定：〔自动〕标注的行有对应自动化测试（可点验，见括号内测试文件），人工走查仍建议过一遍关键现象；〔人工〕标注的行无自动化覆盖，是本清单的核心增量。
> 平台列：mac = macOS 13+（arm64）、win = Windows 10+、linux = 主流发行版。某平台不适用的条目在格内标 `N/A` 并注原因。

## 0. 环境准备（每平台开始前）

- [ ] 构建产物安装包为当前 tag 构建（非 dev 调试包）；应用数据目录为全新环境或按各条目要求预制
- [ ] 准备：至少两家真实 Provider（其中一家 Anthropic Messages、一家 OpenAI 系）、一个本地 MCP stdio echo server、（可选）SearXNG 实例与 Tavily Key
- [ ] macOS 首装按未签名引导口径放行（33 号票后 macOS 无签名公证——右键打开绕过 Gatekeeper 提示）

## 1. 压缩（04 号票；§6.1/§6.2）

| # | 链路步骤 | 预期现象 | 勾选（mac / win / linux） |
|---|---|---|---|
| 1.1 | 新会话用小上下文窗口模型连续对话至窗口占比过 0.6 阈值 | 下一请求前自动触发压缩：时间线出现「正在压缩」指示条，完成后出现「已压缩早期上下文」分隔提示；Composer 正常可继续发送 | 〔自动〕`auto_compaction_across_threshold_frame_event_wire_and_metering`（tests/backend_e2e.rs）；人工复核指示条与分隔提示可见性：mac☐ win☐ linux☐ |
| 1.2 | 点击 Inspector 压缩按钮（手动压缩） | 立即执行一次压缩并展示 tokensBefore/After；运行中点按被拒（Toast 提示会话运行中） | 〔自动〕`manual_compact_session_snapshot_and_running_rejected`；人工：mac☐ win☐ linux☐ |
| 1.3 | 把压缩专用模型配置为无效端点后跨阈值 | 压缩失败：Toast 报错；**原上下文可容纳则继续对话不中断**；不可容纳则 run 暂停并保留输入框内容（E_CONTEXT_OVERFLOW 文案），修复配置后可继续 | 〔自动〕`compaction_failure_keeps_history_and_continues_when_fits` / `context_overflow_after_compaction_pauses_with_input_kept`；人工复核输入保留：mac☐ win☐ linux☐ |
| 1.4 | 制造一次超大已完成工具批次（长输出工具多轮）后跨阈值压缩 | 摘要输入中该批次转为带工具名/callId/来源标记的分块文本；时间线中原始工具卡与结果**配对完整**（点开折叠分组核对） | 〔自动〕`entry_block_marks_tools_with_sources`（src/backend/compaction_test.rs）；人工复核卡片完整：mac☐ win☐ linux☐ |
| 1.5 | 压缩后重启应用，继续同一会话 | 历史完整：压缩边界提示仍在，压缩点前后消息均可回看；发送正常 | 〔人工〕mac☐ win☐ linux☐ |

## 2. 重连（05 号票；§5.8）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 2.1 | 发起长回复（流式中），Cmd+R / WebView 崩溃恢复（开发菜单 Reload） | 出现重连覆盖层 → 自动 attach_run 续流：**不丢帧、不重复**（文本连续无断字、无重放整段）；覆盖层消失后交互正常 | 〔自动〕`attach_mid_run_replays_and_hot_switches_channel` / `attach_replay_window_exhausted_sends_snapshot_frame`；人工真机重载：mac☐ win☐ linux☐ |
| 2.2 | 流式中强杀 Rust 进程后重启应用 | 会话标记 interrupted（「已中断」标注）；已提交历史完整；无悬挂「运行中」状态 | 〔人工〕mac☐ win☐ linux☐ |
| 2.3 | 审批卡挂起时重载 WebView | AskPanel 恢复显示**原始操作及参数**；批准/拒绝照常生效；重载期间不自动决定任何审批 | 〔自动〕`snapshot_matches_frames_and_attach_during_approval_pending`；人工：mac☐ win☐ linux☐ |

## 3. 队列（06 号票；§5.5）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 3.1 | 运行中输入消息 Alt+Enter 排队 ×2 | 队列条显示 2 条待发；run 结束后自动投递第一条、完成后第二条（两个 run 背靠背） | 〔自动〕`followup_auto_delivery_after_done_two_runs_back_to_back`；人工：mac☐ win☐ linux☐ |
| 3.2 | 运行中 Enter 发送（steer） | 消息入当前 run，turn 边界注入；时间线出现「插话」徽标节点；下一拍模型可见插话内容 | 〔自动〕`steer_injected_midrun_consumed_at_turn_boundary`；人工：mac☐ win☐ linux☐ |
| 3.3 | 队列投递失败（如把模型配成无效端点后投递） | 队列失败暂停：横幅提示；**跨重启保留**（重启后队列条仍在、保持暂停）；点「继续」明确恢复后才投递 | 〔自动〕`failure_pauses_queue_until_explicit_resume`；人工跨重启：mac☐ win☐ linux☐ |
| 3.4 | 队列有条目时取消当前 run | 队列清空、条目文本交还输入框草稿、queue 文件删除 | 〔自动〕`cancel_clears_queue_and_deletes_persisted_file`；人工：mac☐ win☐ linux☐ |
| 3.5 | 队列条 × 删除 / 「编辑取出」 | 删除后队列长度减一；取出后文本回填输入框、队列移除 | 〔自动〕`remove_followup_entry_and_miss_reports_not_found`；人工：mac☐ win☐ linux☐ |

## 4. MCP（08 号票；§9.7）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 4.1 | 设置页添加本地 stdio echo server 并 connect | 首连触发 TOFU 信任询问（展示启动命令与指纹）；允许后状态徽标 Ready、工具清单出现 | 〔自动〕`connect_discover_call_and_status_events`（src/mcp/pool_test.rs）+ fixture echo server（src/mcp/fixture.rs）；人工真机：mac☐ win☐ linux☐ |
| 4.2 | kill echo server 进程后等自动重连 | 状态 Error → 自动重连 → Ready；重连后工具可调用 | 〔自动〕`auto_reconnect_after_server_kill`；人工：mac☐ win☐ linux☐ |
| 4.3 | 调用 MCP 工具（会话内） | 镜像工具卡正常渲染参数与结果；未声明 read_only 的工具首次调用弹审批 | 〔自动〕`mcp_snapshot` 面 + tools_test；人工：mac☐ win☐ linux☐ |
| 4.4 | 调用中 kill server（结果未知场景） | 报「结果未知（可能已执行）」明确提示，**不自动重放**；由模型/用户核查后决定重试 | 〔自动〕`result_unknown_never_replayed`；人工：mac☐ win☐ linux☐ |
| 4.5 | echo server 新增一个工具（改 fixture 脚本重启） | 工具清单热同步；**新增工具首次调用须重新确认**（不继承旧免审批许可）；远端 annotations 声明 read_only 不提权 | 〔自动〕`list_changed_hot_sync` / `always_allow_auto_grant_respects_schema_changes`；人工：mac☐ win☐ linux☐ |

## 5. 子代理（10 号票；§5.9/ADR-0001）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 5.1 | 会话内用 `$` 触发符委派子代理（如 explore） | subStarted 帧 → 工具卡出现子代理运行态；subFrame 实时跟随（抽屉可看子代理流式过程）；完成后 subReport 汇总 | 〔自动〕sub_registry/exec 全链（src/host/commands/subagent_test.rs 四象限）；人工真机：mac☐ win☐ linux☐ |
| 5.2 | 子代理运行中 stop_subagent（卡片停止钮） | 干净终止：subDone(cancelled)、无残留进程、无悬挂状态；主代理继续可用 | 〔人工〕mac☐ win☐ linux☐ |
| 5.3 | Git 项目内委派写任务（implementer） | 子代理在独立 worktree 工作；完成后主代理回收：无冲突合入、有冲突保留双方副本（`*.cw-conflict-*`）且不覆盖用户修改；不自动 commit/push | 〔自动〕worktree_test（dispatch/recover/conflict/abandoned 全套）；人工真机：mac☐ win☐ linux☐ |
| 5.4 | 并发委派超过设置上限（默认 4） | 满额排队不拒绝；降低上限不取消已有任务，低于新上限后补入 | 〔自动〕`gate_queues_when_full_and_admits_after_release` / `lower_limit_waits_until_active_below_new_limit` / `raise_limit_admits_immediately`；人工：mac☐ win☐ linux☐ |
| 5.5 | 子代理预算耗尽（小步数上限） | partial 终态 + E_SUB_BUDGET 错误面回喂（模型可读「再次委派」提示）；子面板显示子代理用量 | 〔自动〕subagent_test partial 象限；人工：mac☐ win☐ linux☐ |

## 6. 图片（17 号票；§6.2/§7.7）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 6.1 | vision 模型会话：附件按钮 / 粘贴截图 / 拖拽图片各发一张 | 三入口均入附件条（≤4 张、5MiB 上限）；发送后气泡缩略图可见；模型回复内容证明「看得到图」（multimodal 块下发） | 〔自动〕`vision_model_consumes_image_as_multimodal_block`；人工三入口：mac☐ win☐ linux☐ |
| 6.2 | 切到非 vision 模型继续同一会话 | 历史图片以占位提示（「当前模型无法查看历史图片」）继续纯文字对话；**新附图直接拒绝**（明确报错） | 〔自动〕`non_vision_rejects_new_image_and_degrades_history_to_placeholder`；人工：mac☐ win☐ linux☐ |
| 6.3 | 运行中带图消息发送 | 带图不入 steer，自动转 followUp 队列，投递时展开 image part | 〔自动〕`followup_with_image_delivers_image_part`；人工：mac☐ win☐ linux☐ |

## 7. 审批与安全门（07/22/23/25 号票；§8.5–8.7）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 7.1 | 诱导会话执行写文件（默认权限档） | 审批卡展示 diff 与目标路径；allow once / 本会话 / 拒绝三选项行为正确 | 〔自动〕`approval_confirm_each_suspend_allow_resumes_done` / `approval_deny_ends_cancelled`；人工：mac☐ win☐ linux☐ |
| 7.2 | 同批多工具含 ask（混批） | **只执行源序首个 ask**，其余调用明确返回「未执行，等待回答后重新决定」；回答后模型重新决定 | 〔自动〕`ask_mixed_batch_executes_only_ask_others_reported_skipped` / `multiple_asks_execute_first_only_others_skipped`；人工：mac☐ win☐ linux☐ |
| 7.3 | 诱导写宿主内部状态（如 `~/.craftwave/config.json`、项目 `.craftwave/`） | 硬拒（E_INTERNAL_STATE）不挂起询问；任何权限档与白名单不豁免 | 〔自动〕internal_state_test 全套（src/safety/internal_state_test.rs）；人工抽验一条：mac☐ win☐ linux☐ |
| 7.4 | 计划门：无计划写 / 批准计划后改写范围外文件 | 默认有计划即守门：范围外写弹范围卡（文件/目录范围明确展示）；批准基线冻结后越界再问 | 〔自动〕`plan_tier_sees_and_executes_all_four_small_tools` / `e2e_plan_scope_suspend_resume_allow_once`；人工：mac☐ win☐ linux☐ |
| 7.5 | 审批挂起时强杀进程 → 重启恢复 | 恢复后待审批卡重现（原始参数完整）；不自动执行；失效审批 resolve 得到明确错误并关闭弹窗 | 〔自动〕`snapshot_matches_frames_and_attach_during_approval_pending`；人工：mac☐ win☐ linux☐ |
| 7.6 | 灰区命令（不在黑名单/白名单、有写信号）触发审计 | 审计状态徽标（审计中→结论）；allow 放行 / block 拒绝（E_AUDIT_BLOCKED，run error 终态）/ ask·失败转人工审批卡带审计注记 | 〔自动〕audit 六例 e2e（tests/backend_e2e.rs `audit_*`）；人工：mac☐ win☐ linux☐ |

## 8. web_search 三通道（13 号票；C-12 优先级 ①→②→③）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 8.1 | 仅配 SearXNG（启用+探活通过）触发搜索 | 走 SearXNG 本地通道；工具卡来源徽标「本地」；连通测试按钮 ok | 〔自动〕web_search_test 优先级切换（src/tools/web_search_test.rs）；人工：mac☐ win☐ linux☐ |
| 8.2 | 关闭 SearXNG（或探活失败）触发搜索 | 自动降级内置直接抓取（DuckDuckGo HTML）；徽标「抓取」；走应用代理设置 | 〔自动〕同上；人工：mac☐ win☐ linux☐ |
| 8.3 | 仅配 Tavily Key（SearXNG 关闭、直抓故障模拟） | 降级到 Tavily；徽标「云」；Key 无效/配额尽给明确分类不重试 | 〔自动〕同上；人工：mac☐ win☐ linux☐ |
| 8.4 | 三通道全部不可用 | E_SEARCH_UNAVAILABLE：报错带逐层失败原因与配置引导（模型可读） | 〔自动〕同上；人工：mac☐ win☐ linux☐ |
| 8.5 | web_fetch 抓取公网页面 + http_request 打 API | 卡片渲染正常；SSRF 向量（内网地址）被拦并给类别提示 | 〔自动〕ssrf_test / web_fetch_test；人工：mac☐ win☐ linux☐ |

## 9. Provider 三协议（18/38 号票；§11）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 9.1 |三家协议（openai_chat / openai_responses / anthropic_messages）各配真实 Key，设置页连接测试 | 三家测试均 ok 且延迟合理；401 Key 给七类错误分类（脱敏不回显 Key） | 〔自动〕probe_test 13 例 + protocol_dialects_e2e 6 例（tests/protocol_dialects_e2e.rs，fixture 假服务）；**人工真实端到端（不以设置项存在替代）**：mac☐ win☐ linux☐ |
| 9.2 | 三协议各跑实际对话：文本 / 工具调用 / reasoning / usage 计量 / 中途取消 | 流式正常、工具卡配对、reasoning 折叠展示、四口径计量入账、取消立即终止且保留已输出 | 〔自动〕protocol_dialects_e2e 全链（anthropic/openai_responses 文本+工具+reasoning+usage、慢流取消）；**人工真实端到端**：mac☐ win☐ linux☐ |
| 9.3 | 同一 run 中途切模型 / 切 thinking 档 | 下一请求生效（不中断当前流）；预算与图片能力按新模型重新校验（切非 vision 后带图请求给明确错误） | 〔自动〕`e2e_mid_run_switch_applies_on_next_request` + 38 号跨方言热切换；人工：mac☐ win☐ linux☐ |
| 9.4 | 网络代理三种模式（none/system/manual socks5）各跑一轮对话（有代理环境时） | none 显式直连（不读环境变量代理）；manual 经指定代理可达；设置保存后下一次对话生效 | 〔自动〕19 号票 socks5 转发器 e2e；人工真机代理环境：mac☐ win☐ linux☐ |

## 10. 用量与配额（20/21/27 号票；§7.7/§11.5）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 10.1 | 跑若干轮对话（含子代理与 suggest/压缩辅助调用）后重启应用 | run/会话四口径重启恢复一致；Composer 本轮只含主代理自身用量；子代理面板单独显示子量；主任务次数只计主 run | 〔自动〕`usage_stats_ledger_e2e_record_recover_delete_degrade` + `suggest_background_call_events_and_session_usage`；人工：mac☐ win☐ linux☐ |
| 10.2 | 改名/删除一个已用过的模型后回看历史 | 历史名称仍可读（调用时快照 modelName）；日统计过期（>90 天）不抹掉、仍保留会话明细 | 〔自动〕20 号票 storage 单测；人工：mac☐ win☐ linux☐ |
| 10.3 | 跨日时分界点附近跑对话（改系统日期模拟） | 按 usage 返回时的本地日期固定落账；之后改时区不重排历史；无重复计入（主/子/辅助汇总核对） | 〔自动〕`record_on_day` 注入单测（src/host/usage_test.rs）；人工：mac☐ win☐ linux☐ |
| 10.4 | Inspector 统计区块四范围切换（今天/7d/30d/全部） | 柱状图/占比条与明细一致；legacy 标量旧会话回落显示 | 〔自动〕usage-charts/usage-stats 单测；人工：mac☐ win☐ linux☐ |
| 10.5 | 配置 DeepSeek/智谱等真实 Key 查配额卡 | 六态之一正确展示；离线兜底显示最近快照；Key 不回显 | 〔自动〕quota_test 六态（mock）；**人工真实 Key 比对**：mac☐ win☐ linux☐ |

## 11. 存储与删除（20/32/33 号票；§7.1/ADR-0003）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 11.1 | 删除一个有正文/图片/日志/队列/run 明细的会话 | 确认弹窗说明备份残留并提供备份目录入口；确认后正文/图片 blob/专属日志/`.queue.json`/run 明细全清；**无会话关联的日模型数字保留** | 〔自动〕`usage_stats_ledger_e2e_…delete` + session_delete_test；人工核对数据目录：mac☐ win☐ linux☐ |
| 11.2 | 触发格式升级备份（旧版本数据目录 + 新版本首启） | 备份目录独立生成（含恢复说明.txt）；写入失败时暂停升级并告知；关闭应用手动恢复到备份时点可行 | 〔自动〕backup/恢复序列测试（33 号票 +16 例）；人工真实旧数据目录：mac☐ win☐ linux☐ |
| 11.3 | 恢复旧备份后首次启动 | 信任（projectTrust/MCP TOFU）/外部目录授权/审批记忆全部要求重新确认；队列保持暂停；旧待审批请求不直接执行；报告落 app.log | 〔自动〕33 号票保护序列全链测试；人工：mac☐ win☐ linux☐ |
| 11.4 | 正常 WebView 重连（无恢复备份场景）恢复有效审批 | 审批卡照常恢复可用（不因备份保护误伤正常恢复路径） | 〔自动〕2.3 同源；人工：mac☐ win☐ linux☐ |

## 12. 桌面面（29/30/31/33 号票；§10.7）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 12.1 | 拖动窗口到新位置/尺寸后退出重启 | 几何还原到上次位置（越界回默认）；最大化只置位 | 〔自动〕window_state_test；人工拖拽：mac☐ win☐ linux☐ |
| 12.2 | 托盘图标点击/菜单直达会话；开启 close_to_tray 后点关闭 | 通知点击回跳对应会话锚点；关闭隐藏到托盘（开关关=走退出确认协议） | 〔人工〕mac☐ win☐ linux☐ |
| 12.3 | 设置全局热键（默认 alt+space）实按 | 主窗口呼出/隐藏；冲突时红字+Toast；录制态交互正常 | 〔人工〕mac☐ win☐ linux☐ |
| 12.4 | 删除配置文件首启 | 四步向导自动弹出（语言→供应商→项目→权限档）；中途跳过/续走/整包关闭均正常；完成后不再自动弹 | 〔自动〕onboarding 27 例逻辑面；人工真实首启：mac☐ win☐ linux☐ |
| 12.5 | 设置页切换语言（zh↔en） | 全界面即时切换含设置页/工具卡/Inspector/Toast/错误码文案；托盘菜单与系统通知跟随；无裸 key、无遗漏硬编码 | 〔自动〕i18n 门禁 9 例；人工全界面目检：mac☐ win☐ linux☐ |
| 12.6 | 更新器：配置真实 pubkey 后连打两 tag 端到端 | 关于页三态（检查中/已最新/可更新）；下载安装前有活跃 run 或审批挂起时被阻止并列原因；队列未明确暂停时被阻止 | 〔人工 HITL——33 号票留验，需 tauri 密钥配齐后实测〕mac☐ win☐ linux☐ |
| 12.7 | 更新前把队列暂停（pause_followup_queue）后走更新重启 | 更新后队列仍保持暂停、等待明确继续 | 〔自动〕pause 持久化测试；人工：mac☐ win☐ linux☐ |
| 12.8 | 会话运行中强杀进程重启（NFR-06 演练） | 无半写段、无数据丢失；中断 run 标 interrupted | 〔自动〕run_persistence_test；人工 SIGKILL：mac☐ win☐ linux☐ |

## 13. 键盘可达抽查（NFR-20）

| # | 链路步骤 | 预期现象 | 勾选 |
|---|---|---|---|
| 13.1 | 纯键盘走一遍核心流：Tab 切焦点 → 发送 → 审批卡四选项 → Inspector 六页 → 设置页保存 | 全功能可达；焦点顺序合理；弹窗/抽屉焦点管理正确（Esc 关闭回焦） | 〔人工〕mac☐ win☐ linux☐ |
| 13.2 | ghost hint（Tab 接受）、Ctrl+L 模型切换、@/$/ 触发符菜单纯键盘操作 | 均可完成无鼠标死角 | 〔人工〕mac☐ win☐ linux☐ |

## 14. 已知边界（不作为缺陷）

- macOS 无签名公证（用户砍单）：首装绕过 Gatekeeper 提示属预期；应用内更新不依赖 OS 代码签名（minisign 正交）。
- Windows/Linux 未签名分发：SmartScreen/未知发布者提示属预期，引导文案见首装说明。
- 中文 PDF 部首变体映射（pdf-extract #103）与 docx 无分页参数：16 号票已记录的已知限制。
- 压测结论：2000 条会话稳态滚动达标；**发送插入点约 1.1s 冻结**为已知热点（39 号决策票处理中）。
