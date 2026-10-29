# 实现：会话日志与日志查看器

Type: task
Status: open

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

- [ ] 图片仅记实际可用路径/哈希/格式/大小，不记录图片本体；删除会话同步删除其专属日志与轮转副本；应用级日志不保留会话正文或图片。

- [ ] 日志级别切 Debug（verbose）后跑一轮真实/脚本 run：查看器可见请求与响应（文字脱敏、图片仅路径及摘要信息）及 run/tool/approval 行。
- [ ] 同一会话日志文件与设置页拉取内容中 grep 不到任何 Key/Authorization 明文（脱敏集成测试常驻 CI，NFR-09 收口）。
- [ ] 级别切回 Info：新日志不含请求/响应消息体；级别修改即时生效无需重启。
- [ ] 单会话日志超 10MB 触发轮转（`.jsonl.1`），应用运行不中断。
- [ ] 「打开日志目录」在系统文件管理器中打开 `~/.craftwave/logs/`；会话选择器能按会话过滤查看。
- [ ] §7.1 日志布局、§3.3 tail 语义回写完成。
