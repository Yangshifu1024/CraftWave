# 实现：会话日志与日志查看器

Type: task
Status: resolved

## 要做什么

FR-15-01 + FR-14-04 诊断组 + FR-14-07（PRD 行 568/557/560，出处 §3.3 #54–56/D12/§9.4）：

- 每个会话的 run/LLM/工具/审批/压缩/子代理活动都有可查日志。
- verbose 模式记完整请求与响应原文——**落盘前经密钥脱敏**（D12 复刻点：rig `redact::json_text` 同款，§13 D12 表「复刻清单增补：落盘前 JSON redact 脱敏」原文；NFR-09「日志全链路脱敏」）。
- 设置页可切日志级别（`config.log.level` 字段已存在）；日志查看器按会话过滤查看 + 一键打开日志目录。

出处缺口声明：`~/.craftwave/logs/` 在 §7.1 布局中仅一行占位，文件布局/轮转/级别语义无字段级定义——本票票面定义，**动工前先回写 §7.1（布局）与 §3.3（#54 tail 参数语义）**。

## 具体做什么

### 后端（src-tauri/）

- **脱敏模块** `util/redact.rs`（当前代码库 grep redact 零命中，从零落，D12 复刻）：
  - `fn redact_json(value: &mut Value)` 递归遍历；
  - 对象 key 命中敏感名单（`apiKey`/`api_key`/`authorization`/`token`/`secret`/`password`/`credential`，大小写不敏感、含 key/token 结尾类子串）→ 值替换 `"***"`；
  - 字符串值兜底扫 `Bearer <…>`/`sk-…` 头模式；数组逐元素递归；其余字段零破坏；
  - 名单硬编码不做配置（攻击面最小化）；rig 依赖内不复刻，仅本项目自有实现。
- **图片日志（2026-10-29 Q16 用户补充）**：详细日志中的图片仅记实际可用路径、哈希、格式和大小，不写 base64/二进制本体。read 读图记录工作区源文件路径；附件/粘贴记录会话内 blob 存储路径；只有实际获得的原始路径才记录，不为无来源路径的图片虚构路径。文字请求响应仍按详细模式记录并脱敏。
- **会话删除（2026-10-29 用户确认 Q17）**：同步关闭句柄并删除会话专属诊断日志及轮转副本；应用级日志只保留运行元数据，不记录会话正文或图片，避免会话删除后仍留内容副本。删除失败必须报告，不能声称全部删除成功。
- **删除范围补充（Q43）**：删除当前会话正文、图片、专属日志及关联 run 用量明细；20 号票保留无正文/会话关联的日模型消耗数字，33 号票保留完整历史升级备份。删除确认明确说明旧备份可能仍含内容，并提供备份目录入口供用户自行删除；不能以保留备份为由跳过当前会话日志清理。
- 删除实现须盘点子代理历史、工具边车、命令/HTTP spill 等会话专属内容副本；共享资源只清本会话引用，不删除用户源文件或项目成果。未回收 worktree 继续按 Q11/ADR-0001 保留，处理其索引归属。由本票协调既有 delete_session 路径，测试需覆盖副本清理和工作成果保留，见 ADR-0003。
- **日志布局**（票面定义，回写 §7.1）：
  - `~/.craftwave/logs/app.log`：应用级（启动序/命令报错/panic 捕获/§7.3 ⑤ 落盘失败记账——把现有散落的 `log`/`eprintln` 诊断输出归拢到此通道）；>5MB 轮转 `.log.1`（保留 1 份，再满覆盖）。
  - `~/.craftwave/logs/sessions/<sessionId>.jsonl`：每会话一文件，每行 `{ ts, level, kind, msg, data? }`，可选锚 `runId?/callId?/subId?`；`kind ∈ run|llm|tool|approval|compact|subagent`；>10MB 轮转 `.jsonl.1`（保留 1 份）。
  - 保留期自动清理（FR-14-04 保留期项）随 M3，本票只做轮转。
- **写入通道** `host/session_log.rs`：轻量自写 logger，**不引入 tracing 全家桶**（per-session 分文件 + JSON 行需自定义 subscriber layer，成本高于收益，取舍记 Answer）：
  - `SessionLogger`——`Mutex<HashMap<sessionId, Writer>>`；append 打开、**不 fsync**（日志可丢、性能优先，与 NFR-06 会话数据通道严格区分）；写前级别过滤 = `config.log.level`（LogConfig Debug/Info/Warn/Error 已有；**verbose = level 设为 Debug**）；会话删除时关闭句柄并删除本会话日志及轮转副本。
  - 埋点（Info 级元数据、Debug 级全文）：
    - run：起止（runId/模型/耗时/tokens/终态 reason）——chat.rs run 收尾处；
    - llm：挂 `backend/rig_bridge.rs` 请求出口/响应收口——Info 记 host/model/耗时/usage，**Debug 记请求/响应 JSON（图片替换为路径/哈希/格式/大小），过 `redact_json` 后落盘**；
    - tool：起止与错误（callId/name/ok/耗时/E_* 码）——工具执行器完成点；
    - approval：ask 开关（askId/answered）——ask 协议两端口；
    - compact：压缩生命周期三事件（04 号票事件点复用）；subagent：子代理生命周期（10 号票事件点复用）。
  - 级别热生效：save_config 缓存热替换后，下次写入即按新级别过滤，无需重启。
- **命令三条**（§3.3 #54–56）：
  - `read_session_log` `{ sessionId, tail? }` → `{ lines: string[] }`（tail=尾行数，缺省 500、上限 5000——**语义回写 §3.3 注记**；文件不存在 → 空 lines 不报错）；
  - `list_log_files` `{}` → `string[]`（logs/ 下全部文件相对名：app.log* + sessions/*）；
  - `open_logs_dir` `{}` → `void`（系统打开 logs 目录——复用 03 号票 `open_dir` 的系统打开通道与数据目录白名单口径；03 未落地则随本票带最小实现并记 Answer）。

### 前端（ui/）

- **日志页增强**（`ui/src/settings/` logs 页 PageKey 已在 registry，现仅级别项骨架）：
  - 级别下拉（写 `config.log.level`，旁注「Debug=详细模式，记录请求响应文字及图片路径/摘要信息（已脱敏，不含图片本体）」）；
  - 「打开日志目录」按钮 → `open_logs_dir`；
  - 查看器：会话选择器（`list_log_files` 过滤 sessions/ 前缀，叠加活跃会话列表做标题映射显示）→ `read_session_log` 拉尾部渲染只读 `<pre>`（等宽滚动容器，kind/level 着色，长行折行）+「刷新」钮；
  - 拉取式，不做实时流（边界）。
- ipc/client.ts 增三命令出口 + COMMANDS 注册；类型随 02 号票生成物面。

- **命令审计接缝（36）**：记录审计起止、结论、简短理由、失败/超时原因与已返回用量；证据内容及命令参数遵循脱敏，不能通过应用级日志保留会话正文。

### 测试

- 图片日志：附图请求和 read 读图仅包含实际路径/哈希/格式/大小，不含图片 base64；粘贴图片记录 blob 路径，不虚构原始路径。删除会话后本会话日志及轮转副本不存在，应用日志不含会话正文或图片；删除失败可见。

- Rust：`redact_json` 表驱动（嵌套对象/数组/key 大小写与结尾模式/Bearer 与 sk- 兜底/其余字段零破坏）；级别过滤（Debug 全记/Info 不记 debug/Error 全记）；轮转触发（写超阈值出 .1 且续写新文件）；`read_session_log` tail 语义（缺省 500/上限 5000/空文件/坏行容错）。
- 集成（ScriptedBackend，无网络）：verbose 开 → 跑一轮带工具与审批的 run → 读回日志断言含 llm 请求/响应行、tool 行、approval 行，且全文 grep 无 `sk-`/apiKey 明文（**NFR-09「日志全链路脱敏」验收测试，常驻 CI**）；verbose 关 → 无消息体行。
- 前端 vitest：日志页渲染（级别写回/查看器拉取/空态/标题映射）；contract.test 扩域（#54/#55/#56 三命令）。

## 边界（不在本票）

- 日志实时流式查看（file watch）；日志导出/分享；按级别/关键字检索过滤 UI（先整段 tail）；保留期自动清理（M3 §7.7）；结构化日志检索/FTS（M3 index.db）。
- toolres 边车既有工具输出恢复通道不动（日志是运维诊断面，非会话数据面）；run 帧流与 canonical 落盘（NFR-06 通道）零改动。

## 验收

- [x] 图片仅记实际可用路径/哈希/格式/大小，不记录图片本体；删除会话同步删除其专属日志与轮转副本；应用级日志不保留会话正文或图片。

- [x] 日志级别切 Debug（verbose）后跑一轮真实/脚本 run：查看器可见请求与响应（文字脱敏、图片仅路径及摘要信息）及 run/tool/approval 行。
- [x] 同一会话日志文件与设置页拉取内容中 grep 不到任何 Key/Authorization 明文（脱敏集成测试常驻 CI，NFR-09 收口）。
- [x] 级别切回 Info：新日志不含请求/响应消息体；级别修改即时生效无需重启。
- [x] 单会话日志超 10MB 触发轮转（`.jsonl.1`），应用运行不中断。
- [x] 「打开日志目录」在系统文件管理器中打开 `~/.craftwave/logs/`；会话选择器能按会话过滤查看。
- [x] §7.1 日志布局、§3.3 tail 语义回写完成。

## 决议记录（2026-10-30，32 号票实施）

**实现面**：
- `util/redact.rs`（D12 复刻、本项目自有实现）：`redact_json` 递归脱敏（敏感键名单精确 + 结尾模式、大小写不敏感 → 值替换 `"***"`；字符串兜底 `Bearer <…>`（前缀保留）/`sk-<≥16 位>`；数组递归、其余零破坏）+ `summarize_images`（超长 base64 段按魔数判定为图片 → 替换 `[image <格式> sha256:<哈希> size=<字节>B 已省略本体]`，哈希与 imgblob 内容寻址同算法可对上 blob 文件名；解码失败防漏兜底 unknown 格式仍替换）。
- `host/session_log.rs`：轻量自写 logger（不引 tracing，票面定案记 Answer）——`SessionLogger` per-session 句柄表 + app 单文件，级别过滤每次写入读配置缓存（save_config 热替换下一行即生效，verbose=`verbose()` 供调用方预判跳过大载荷序列化）；轮转 10MB/5MB 各保留 1 份（`.1` 覆盖，打开前也探现有大小）；`close_and_purge` 关句柄 + 删两文件 + 失败清单上报 + 已清除名单防迟到写入重建文件；`read_tail`/`list_files` 命令读取面。
- llm 埋点：`TurnDriver` 挂 `LlmLogSink` trait（backend 层只递引用，序列化/脱敏/落盘全在宿主；不进 `drive_run` 签名、既有测试直驱路径零漂移）——请求出口/成功收口/取消/失败四相位；Info 元数据行（host/model/耗时/usage，失败升 Warn 且错误消息过字符串兜底）+ Debug 请求/响应原文两行（verbose 才序列化）。
- tool 行：EventfulSink 以 ToolStarted→ToolResult 配对算耗时（恢复重放无起点则耗时省略）；approval 行：`wait_approval_with`（审批卡，主/子代理共用）两端口 + `exec.rs` 问卷 ask 经 `AskLogSink` trait 接缝（backend 不反向依赖 host）；compact 行：`execute_compaction` 包装三相位（run 前自动/ask 关闭后/手动 compact_session 三调用点全覆盖）；subagent 行：spawn/done（锚 subId）；run 行：开始（spawn 内）+ 收尾（终态/耗时/tokens/reason，失败同时记 app 通道）。
- app.log：启动序（lib.rs setup 完成行）、run 失败记账、canonical/index 落盘失败记账（RunRecorder 增 app_log 参数，§7.3 ⑤）。散落的 debug eprintln（dev 终端诊断）**未全量迁移**——app 通道已建 + 关键记账点已挂，全量归拢降级为渐进（诚实降级，理由：几十处 cfg(debug_assertions) 诊断输出迁移属大面积触碰，价值/风险比低）。
- 配置句柄统一：`AppState::rebind_config`（lib.rs setup 用外部 config 实例重绑 config+session_log 同源——否则通知门与日志器各持一份缓存实例，save_config 热替换到不了日志级别）。
- 三命令：`read_session_log`（tail 缺省 500/上限 5000/0 视为缺省/文件不存在空清单/坏行原样）、`list_log_files`（app.log* + sessions/* 相对名）、`open_logs_dir`（创建 + 脱离进程打开，headless 可测 inner）；manifest 宏同源注册，ts-rs 生成物两件进 git。
- delete_session 联动（Q43 盘点）：会话日志与轮转副本（失败清单非空 → E_IO 报告残留，**不声称全部删除成功**）、`<sid>.queue.json`（followUp 队列文件，**此前 delete_session 遗漏，随本票补删**）、子代理历史（`histories/subs/<parentRunId>/` 按 meta.json sessionId 归属删父目录，meta 缺失/损坏保守保留不误删）、会话级内存桶（approval_memory 既有 + session_trust 新增 forget + plan_scopes forget）。
- 前端：日志页 = 级别下拉（旁注 verbose 说明）+「打开日志目录」（改走 open_logs_dir）+ 会话日志查看器（清单过滤 sessions/ 前缀 + 活跃 Tab 标题映射选择器 → 拉尾部 500 行只读渲染，level 着色 error/warn/debug、kind 徽标、长行折行、坏行灰显原样、刷新钮；拉取式无实时流）；i18n 双语字典补键。

**测试**（全部通过）：Rust 侧 `redact` 11 例（嵌套/大小写/结尾模式/Bearer 与 sk-/零破坏/图片摘要/blob 路径实证补全/无来源不虚构路径/短串与非图片零误伤/防漏兜底）+ `session_log` 9 例（级别过滤三态/热生效/轮转原语与写路径/tail 语义与坏行/删除联动与失败报告/list_files 与 app 行）+ `logs` 命令 2 例 + `session_delete_test` 4 例（日志与轮转副本随删/删除失败报告/子历史按 meta 归属与保守保留/工作成果保留）+ e2e `session_logs_e2e` 2 例（verbose 全链：mock 两拍 ask 问卷回合，断言 run/llm/tool/approval 行齐备 + 请求原文含 history 且全文无 `sk-` 明文 + 探针被替换占位 + 删除后日志不存在；Info 级：无消息体行、正文不入日志、run/tool 行照记——**NFR-09 常驻 CI**）；前端 SettingsPage 日志页 4 例（级别写回+旁注/查看器清单过滤+标题映射+坏行渲染+刷新/空态/open_logs_dir 接线）+ contract.test #54/#55/#56 锚点与生成物字段级断言。

**Q16 图片日志口径落点**：请求面图片本体（rig Image 的 base64 字段与 read 读图工具结果 compact 文本内的 base64 段）统一在脱敏闸替换为摘要；read 读图的工作区源路径与 mediaType 字段在 base64 段之外、原样保留即「实际源路径」；附件/粘贴图按哈希推 imgblob 路径且**文件实际存在才补** `path=`——无来源不虚构（单测钉住两个方向）。

**诚实边界与降级**：
1. **spill 无会话标识不可归属**：`logs/spill/<callId>.txt` 文件名 = 模型 call id，与会话无映射，删除会话无法可靠清理——依赖 §8.8 既有 24h 保留期自动过期（最长 24h 残留）；已回写 §7.7 Q43 实装注记与 §7.1 布局注释，删除确认不宣称已清 spill。属票面「盘点」要求的诚实结论，非遗漏。
2. **散落 eprintln 未全量归拢 app.log**：app 通道 + 启动序/run 失败/落盘失败三类关键记账已挂，dev 终端诊断串（cfg(debug_assertions)）渐进迁移。
3. **查看器 GUI 真实运行面留用户验收**：自动化覆盖渲染/拉取/空态/标题映射/级别写回；「在系统文件管理器中打开」的 OS 侧行为（open::that_detached）headless 无法验证，单测仅钉目录创建面。
4. **子代理 run 的工具行未逐条落日志**：tool 行挂主 run 的 EventfulSink；子代理执行过程由 kind=subagent 生命周期行（spawn/done 带终态与错误码）+ 子历史回看通道覆盖，子代理逐工具行归 Inspector 子代理面板职责（票面埋点清单以 run/tool/approval/compact/subagent 五类齐备为准，验收不细化到子代理逐工具行）。
5. **轮转「应用运行不中断」的实测口径**：轮转单测验证写路径在轮转后继续写入（写手重开），运行中应用不中断由「日志写入失败静默、不反压业务」的容错设计保证（全部写路径吞错）。

检查链七项全绿：cargo fmt --check / build / clippy --all-targets（0 警告）/ test（lib 840 + e2e 46 + protocol 6 + session_logs 2 + tools 3 + 其余小套件全过）、pnpm typecheck / test（523，首跑遇票面预告的 thread-rendering 已知抖动复跑绿）/ lint。设计文档回写 v2.39（§7.1/§3.3/§7.7/§10.7）。
