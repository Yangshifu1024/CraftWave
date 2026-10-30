# 实现：用量统计持久化与用量面板（FR-02-13 + FR-15-02）

Type: task
Status: resolved（2026-10-31，分支 m2/ticket-20-usage-stats，检查链七项全绿）

## 要做什么

把散落在内存与 index.json 的用量数据变成可跨重启、可跨会话聚合的统计：用户在用量面板看到每天/每个模型花了多少 token、缓存命中率、速率。切片：stats/ 按日持久化 + index.db 查询缓存 + stats_token 命令 + Inspector 统计图表；C-9 口径（后台小调用计入会话累计、不计轮次）延伸到 stats 落账。

FR 依据（原文摘引）：
- FR-02-13：按 run/会话/日/模型：input/output/cache_read/cache_write token、缓存命中率、tok/s；持久化 stats/
- FR-15-02：token 明细（四口径）、缓存命中率、速率、按模型/按日图表；90 天保留
- §7.7（Q39 更新）：`stats/<YYYY-MM-DD>.json` 按日、按实际模型聚合；每次模型响应返回 usage 时，按当时本地日期固定记账，原子合并；90 天保留，不再按整个 run 结束日归属
- §7.8：cache/index.db M2 建库；`stats_cache` 表为 stats_token 查询缓存，**真相在 stats/*.json**；降级=统计直读 stats/*.json
- §3.3 #52：`stats_token { range?: { from, to } } → { byDay[], byModel[], total }`

## 具体做什么

### 后端（src-tauri/）

1. 新增 `storage/stats.rs`，按 §7.7 结构读写 `stats/<YYYY-MM-DD>.json`（`util/atomic.rs` 原子写；文件名即日期）：
   - **落账时点（Q39）**：每次模型响应返回 usage 时，按当时本地日期固定记账；跨午夜的 run 可分属两天，之后切换时区不重划历史。记录发生时间、当时的日期及必要时区信息，以便复核。
   - 主代理、子代理及辅助调用共用此日期规则；从既有 run 累计 usage 帧落账时须规范化为本次调用的贡献，不能把累计值重复相加。终态只做收口，不把已记入前一天的用量挪到结束日。
   - **汇总归属（Q40）**：会话总量包含主代理、子代理及辅助调用；Composer 本轮只显示主代理自身用量，子代理自身用量在子代理面板显示；日/模型统计按实际执行模型记录，父子汇总只计一次。
   - `runs` 只统计主 run（含队列自动发起），子 run 与辅助调用不增加次数。按主 run 身份去重，不能把子代理数或模型请求次数当主任务次数；具体计数字段先补齐 §7.7 与 #52，再实现。
   - 合并继续采用原子重写与既有存储锁纪律，调用身份幂等，重放/重连不重复落账。
   - Q39 按返回 usage 记账也覆盖后续失败/取消：已知实际用量不能因最终失败被丢弃，未知用量不伪造；suggest/标题等后台调用仍静默失败，不承诺供应商不收费。
2. 90 天保留：落账时顺带清理过期文件（文件名排序惰性删除，无后台任务）。
3. `cache/index.db` M2 建库（§7.8 DDL 逐字）：`sessions` 表 + `stats_cache` 表（`PRIMARY KEY (day, model_id)`，四口径列）；
   - 新依赖 rusqlite（bundled feature；注意 MACOSX_DEPLOYMENT_TARGET=13.0）；
   - 维护 = stats 落账成功后异步 upsert（失败仅记日志，§7.8）；
   - 启动校验 `PRAGMA quick_check` + `PRAGMA user_version` 不符 → 弃库重建；
   - 库缺失/损坏/重建中 → 统计直读 stats/*.json（§7.8 降级行）。
4. `stats_token` 命令（§3.3 #52）：优先查 stats_cache，miss 回源 stats/*.json 并回填缓存；range 缺省 = 全部保留期；
   - **回写点**：byDay/byModel 元素形态 §3.3 未字段级定义——按 `{ day, input, output, cacheRead, cacheWrite, total }` / `{ modelId, input, ..., total, runs? }` 定义并回写 §3.3 #52。
5. 口径对齐：
   - UsageTotal 四口径（§3.2）；totalTokens = 四口径之和（`run_persistence.rs:327` 既有折算，保持一致）；
   - rate（tok/s）= usage 帧 rate（§3.4 已有）——不入 stats 文件，面板实时读帧/事件。
6. **run/会话跨重启恢复（Q37）**：本票补齐 input/output/cacheRead/cacheWrite 四口径的权威持久记录与恢复，不能只留 SessionMeta.totalTokens 标量；会话累计与 run 用量分别恢复，后台小调用遵守计入会话、不计主轮次的既有规则。
   - 按实际调用保留模型身份及当时的模型显示名称快照；修改/删除当前模型配置不能改写或抹掉历史名称。run 内切模型须保留真实模型归属，不能全算给结束时模型。
   - 日/模型聚合文件不承担全部会话历史恢复；run/会话四口径记录及汇总的字段、落盘点、旧数据兼容与读取契约先回写 §3.2/§7.7/§10.5，再实现。旧历史仅总量可知时不能伪造四口径拆分。
   - 日/模型统计继续保留 90 天；runMetrics/index/stats 折算同源，缓存删掉后可从权威记录重建。
7. **会话删除（Q43）**：删除当前会话及关联 run 用量明细，保留不含正文及会话关联的日/模型消耗数字；不从保留的日聚合恢复已删除会话明细。聚合存储和查询不得继续暴露已删除会话身份，缓存清理与明细删除一致；具体匿名化字段先回写 §7.7 再实现。历史升级备份按 33 号票保留并向用户说明。

### 前端（ui/）

1. Inspector 用量页（既有：实时速率/本轮/会话累计/上下文进度条）增"统计"区块：
   - 按日柱状图（近 30 天）+ 按模型排行（模型名/四口径/占比条）；
   - 缓存命中率 = cacheRead/(input+cacheRead)（§10.5 公式）；
   - 范围切换：今天 / 近 7 天 / 近 30 天 / 全部。
2. 图表选型：无图表依赖——建议手写 ≤100 行 SVG 柱状/条形组件（数据 ≤90 点、两种图），Answer 记录理由；若评审要求组件库，recharts 体积代价一并记录（AGENTS.md 调研纪律）。
3. Q37：沿用 Composer/Inspector 的本轮及会话用量入口，重启后恢复四口径与缓存命中率；历史名称取调用快照；本期不新增复杂 run 明细列表。
   - Q40：Composer 本轮为主代理自身消耗，会话累计为主/子/辅助总和；子代理面板展示各自用量，文案明确范围，避免误以为会话与本轮数字不一致。
4. 数据源 = `stats_token`；面板打开拉取 + `tokens:update` 事件到达时本地增量修正当日数字（不等轮询）。

### 测试

- Q43：删除会话后其 run/会话明细不可查询，日/模型数字保持真实消耗但无正文或会话关联；删除缓存再重建不复活会话明细，统计数字不因删除或重建重复增减。
- Q39/Q40 回归：跨午夜分别落账，切时区不重划历史；主/子/辅助各按实际模型统计且不重复，队列主 run 计次数、子 run 与辅助不计；累计 usage 帧归一不重算，终态不搬移已记日期。
- Q37 回归：多 run/多模型会话重启后四口径一致，run 内切模型保留真实归属；模型改名或删除后历史名称可读；旧记录无明细时不伪造；日统计过期不抹掉仍保留会话的用量明细。
- Rust：stats 合并（同日多 run/多模型、跨日翻文件、重复落账幂等）；90 天清理；原子写崩溃恢复（残留 tmp）；stats_cache 回填/失效/降级直读；stats_token range 边界；后台小调用 runs 不增；index.db quick_check 重建路径。
- 前端：图表渲染（多日多模型 fixture）、范围切换、事件增量修正、契约测试 #52。

## 边界（不在本票）

- 成本统计（FR-02-14，M3，stats_cache 增 cost 列）；Activity 运行总览（M3 #68）；复杂 run 级明细列表（Q37 只补齐持久化/恢复并沿用现有展示入口）。

## 验收

- [x] 删除会话同步删关联用量明细，保留无正文/会话关联的日模型消耗数字，缓存重建不复活已删除明细。（e2e `usage_stats_ledger_e2e_record_recover_delete_degrade` ⑦⑧ + `session_usage_after_delete_is_zero`）
- [x] Q39 按每次响应 usage 的当时本地日期固定落账，跨日任务可拆日，切时区不重算历史。（`midnight_split_and_timezone_never_rewrites_history` + `record_splits_models_and_days`，记账日注入验证）
- [x] Q40 会话含主/子/辅助，本轮只含主代理自身，子用量可见；runs 仅主 run，实际模型归属正确且无重复累计。（`sub_and_aux_never_count_runs_but_count_usage` + e2e ②③；本轮/子面板口径为 10 号票既有帧面，本票落账同源）
- [x] run/会话四口径跨重启恢复，模型配置改删不影响历史名称；旧记录不可知的拆分不伪造。（e2e ⑥ + `session_usage_three_level_fallback`；modelName 快照随落账写入明细/聚合文件，不随配置改删变化——直接断言在 e2e ②与单测）
- [x] 多会话多模型跨多日使用后，stats_token 聚合数字与 Inspector 实时会话累计同源一致（同源 = 同一落账入口 `UsageLedger::record` 的三面写穿；e2e ④ 同源断言 + `stats_token_aggregates_by_day_and_model` 多日多模型聚合。诚实注记：e2e 为单会话，多会话聚合为单测面）
- [x] 重启后面板数字不丢（stats/*.json）；删除 index.db 后面板仍正确（降级直读）（e2e ⑥⑧：重启恢复 + 删库后回填/降级数字一致）
- [x] C-9：suggest 触发的后台小调用计入当日 stats 且 runs 计数不增（e2e ② + 压缩辅助调用同路径单测）
- [x] 按日/按模型图表渲染正确，范围切换数字联动（`usage-charts.test.tsx` fixture 渲染 + `usage-stats.test.ts` 范围换算与重拉）
- [x] 91 天前的 stats 文件被清理；契约测试 #52；§3.3 #52 回写完成（`prune_expired_removes_only_old_days` + contract.test #52/#59k 双侧断言 + 文档回写 v2.25）

## 决议记录

1. **图表选型（票面「图表选型」条）**：手写轻量组件定案——`ui/src/chat/usage-charts.tsx` 两个组件（按日柱状 SVG viewBox 等分矩形 + 按模型排行 CSS 占比条）约 120 行，零图表依赖，未引入 recharts（体积代价不换：数据 ≤90 点两种图）。
2. **新依赖（AGENTS.md 调研纪律说明）**：`rusqlite 0.41.1`（bundled feature，票面点名 §7.8 建库；自带 SQLite 源码经 cc 静态编译，免系统 libsqlite3 三平台一致）；`chrono 0.4.45`（Q39 本地日期面；已在依赖树内——tauri 等间接依赖，锁同版本不新增下载，仅开 clock+std）。
3. **§7.8 DDL 回写（字段级口径先行）**：`stats_cache` 表增 `runs INTEGER NOT NULL DEFAULT 0` 列——缓存须承载 #52 byModel.runs 的完整查询面，否则每次查询必回源、缓存失去意义；维护语义「异步 upsert」以「不阻断落账主链路、失败仅记日志」为准实现为进程内同步写穿（§7.8 维护行已回写）；库重建/清空后由 `stats_token` 查询路径做缺日检测 → 全量回填（回源 stats/*.json 真相面）。
4. **runs 计次口径（Q40 细化）**：主 run 的**首个有计量的 usage 帧**计当日该模型 runs 一次——次数与用量同源（有真实计量才计次），无 usage 的失败 run 不计次不伪造；子 run/辅助调用恒不计次。
5. **Q43 匿名化字段级定案（已回写 §7.7）**：`stats/<YYYY-MM-DD>.json` 从设计上不含任何会话标识（匿名聚合）——删除会话不改写聚合文件、聚合面永不暴露已删会话身份；`sessions/<sid>.usage.json`（含会话身份的明细）随 `delete_session` 清除；`stats_cache` 与聚合同构（day+model 维度）无会话身份不需清理。子代理历史/工具边车等会话专属副本的盘点清理随 32/33 号票（与 ADR-0003 的联动点，本票只删 usage 明细与内存累计条目）。
6. **tokens:update 载荷扩展（已回写 §3.5）**：增可选 `modelId`（本次增量贡献的实际执行模型）——前端 Inspector 统计区块据此对今日/该模型条目做本地增量修正（不等轮询），随后防抖 500ms 权威重拉校正；modelId 缺省（老载荷）回退整段重拉。
7. **Q37 恢复读面（新增 #59k，已回写 §3.3/§10.5）**：`session_usage` 三级回落——内存累计表（活跃实时面）> `sessions/<sid>.usage.json` sessionTotal（权威面）> index.json totalTokens 标量（`legacyTotalTokens` 回落显示，不伪造四口径拆分）；前端在 Inspector 挂载/会话切换时拉取，经 tokens:update 同款 replace 回填 runMetrics。turnUsage（本轮）重启归零不恢复（票面口径）。
8. **落账接线点**：主 run = EventfulSink 的 usage 帧差分增量（模型身份取 `current_model` 槽——HostRequestRefresher 每拍刷新，run 内切模型按拍真实归属）；子 run = SubRunSink（装配时解析的恒定模型）；辅助调用 = suggest（`{runId}:suggest`）与压缩（`{runId}:compact`，摘要模型归属）。attach 重放不经落账链，天然幂等。
9. **诚实注记（测试面）**：run 内切模型的统计归属由单测覆盖（`usage_detail_merges_by_model_and_recomputes_total`），未新增切模型 e2e（18 号票 e2e_mid_run_switch 已有同场景框架，用量面增量在单测钉住）；「多会话聚合一致」为单测面（e2e 单会话）；91 天清理按文件名日期惰性删除（落账顺带），无后台任务（票面口径）。
10. **损坏自愈语义**：日聚合文件损坏 → 先备份 `<name>.json.corrupt` 再按空重建（宁缺勿炸，查询路径同样生效——Mutex 内串行无并发副作用）；会话明细损坏 → 按空兜底不阻塞会话加载（prefs 同款容错）。
