# 实现：用量统计持久化与用量面板（FR-02-13 + FR-15-02）

Type: task
Status: open

## 要做什么

把散落在内存与 index.json 的用量数据变成可跨重启、可跨会话聚合的统计：用户在用量面板看到每天/每个模型花了多少 token、缓存命中率、速率。切片：stats/ 按日持久化 + index.db 查询缓存 + stats_token 命令 + Inspector 统计图表；C-9 口径（后台小调用计入会话累计、不计轮次）延伸到 stats 落账。

FR 依据（原文摘引）：
- FR-02-13：按 run/会话/日/模型：input/output/cache_read/cache_write token、缓存命中率、tok/s；持久化 stats/
- FR-15-02：token 明细（四口径）、缓存命中率、速率、按模型/按日图表；90 天保留
- §7.7：`stats/<YYYY-MM-DD>.json` = `{ day, byModel: { modelId: UsageTotal & { runs } }, total }`；每次 run 终态读当日→合并→原子重写；90 天保留
- §7.8：cache/index.db M2 建库；`stats_cache` 表为 stats_token 查询缓存，**真相在 stats/*.json**；降级=统计直读 stats/*.json
- §3.3 #52：`stats_token { range?: { from, to } } → { byDay[], byModel[], total }`

## 具体做什么

### 后端（src-tauri/）

1. 新增 `storage/stats.rs`，按 §7.7 结构读写 `stats/<YYYY-MM-DD>.json`（`util/atomic.rs` 原子写；文件名即日期）：
   - 落账点 ①：run 终态——RunRecorder 帧观察器收口 usage 帧的同一位置（`host/run_persistence.rs` 既有链路；usage 为 run 内累计值，终态一次合并）；
   - 落账点 ②：后台小调用收口（suggest/标题生成，C-9/§10.5：计入按日按模型 stats、`runs` 计数**不增**、不计轮次）；
   - 合并语义 = §7.7 原文"每次 run 终态读当日→合并→原子重写"（当日文件小，重写成本可忽略）。
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
6. 会话级跨重启恢复：维持 M1 口径——SessionMeta.totalTokens 标量回填 sessionUsage（§10.5 既有）。
   - **勘误**：票面"从 stats 汇总恢复"不可行——stats 无会话维度；会话级四口径明细恢复需扩 SessionMeta（§3.2 回写），本票不做（见边界）；只保证 runMetrics / index / stats 三处 totalTokens 折算同源。

### 前端（ui/）

1. Inspector 用量页（既有：实时速率/本轮/会话累计/上下文进度条）增"统计"区块：
   - 按日柱状图（近 30 天）+ 按模型排行（模型名/四口径/占比条）；
   - 缓存命中率 = cacheRead/(input+cacheRead)（§10.5 公式）；
   - 范围切换：今天 / 近 7 天 / 近 30 天 / 全部。
2. 图表选型：无图表依赖——建议手写 ≤100 行 SVG 柱状/条形组件（数据 ≤90 点、两种图），Answer 记录理由；若评审要求组件库，recharts 体积代价一并记录（AGENTS.md 调研纪律）。
3. 数据源 = `stats_token`；面板打开拉取 + `tokens:update` 事件到达时本地增量修正当日数字（不等轮询）。

### 测试

- Rust：stats 合并（同日多 run/多模型、跨日翻文件、重复落账幂等）；90 天清理；原子写崩溃恢复（残留 tmp）；stats_cache 回填/失效/降级直读；stats_token range 边界；后台小调用 runs 不增；index.db quick_check 重建路径。
- 前端：图表渲染（多日多模型 fixture）、范围切换、事件增量修正、契约测试 #52。

## 边界（不在本票）

- 成本统计（FR-02-14，M3，stats_cache 增 cost 列）；会话级四口径明细恢复（需 §3.2 SessionMeta 扩字段，另立小票）；Activity 运行总览（M3 #68）；run 级明细列表（stats 按日/模型聚合，无 run 级表）。

## 验收

- [ ] 多会话多模型跨多日使用后，stats_token 聚合数字与 Inspector 实时会话累计同源一致
- [ ] 重启后面板数字不丢（stats/*.json）；删除 index.db 后面板仍正确（降级直读）
- [ ] C-9：suggest 触发的后台小调用计入当日 stats 且 runs 计数不增
- [ ] 按日/按模型图表渲染正确，范围切换数字联动
- [ ] 91 天前的 stats 文件被清理；契约测试 #52；§3.3 #52 回写完成
