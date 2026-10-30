# 实现：Inspector 配额卡（FR-02-15，§11.5 配额适配器）

Type: task
Status: resolved

## 要做什么

右栏 Inspector 常驻配额卡：当前活跃供应商的账户余额 / 订阅额度剩余与总量 / 有效期，随手可见，不用切浏览器查。移植 CodeWave 已验证的 7 适配器（6 家，MiniMax 国际/中国双域名各一）；六态展示 + 快照离线兜底，失败静默降级绝不影响聊天。

FR-02-15 原文要点：Inspector 内置配额卡片，多供应商按当前会话所用 provider 展示；适配器按 Base URL 主机匹配，首批 DeepSeek、Kimi、MiniMax（国际/中国）、智谱、Z.ai、OpenCode（参照 CodeWave 7 家适配器）；未适配显示"暂不支持查询"占位，不报错；六态 ok/error/invalid/rejected/no_key/unsupported，错误态可展开原因；刷新=5 分钟周期 + 手动 + 切 provider 即时；快照持久化 quota/，离线显示最近快照及获取时间；查询失败静默降级，不阻塞任何功能。
§11.5：`trait QuotaAdapter { fn matches(&self, baseUrl) -> bool; async fn fetch(&self, keys) -> Result<QuotaData> }`；按 Base URL 主机路由；快照原子写 quota.json；失败静默降级（占位"暂不能查询"），永不阻塞聊天。

## 具体做什么

### 后端（src-tauri/）

1. 新增 `providers/quota/`（mod + 每家一文件），照 CodeWave `src-tauri/src/core/quota/providers/` 结构移植：
   - 每适配器拆「取数 fetch + 纯解析 parse_*」两半；解析收 `serde_json::Value` + 注入 now——单测无网络无时钟；
   - 失败统一 `FetchFailure { status: Option<u16>, message }`：带 HTTP 状态与无状态失败分开，上层据此判 rejected/error；
   - 密钥源 = config.json `ProviderConfig.apiKey`（v2.5 口径；CodeWave 走 CredentialStore/keyring，移植时去除）。
2. 主机匹配（CodeWave 口径：host_of 去协议/端口/路径/@ 取小写主机；host_matches = 相等或 `.{c}` 子域后缀命中），白名单与端点快照（移植时以 CodeWave 源码 const 核对）：
   - DeepSeek：`api.deepseek.com`/`deepseek.com` → GET `https://api.deepseek.com/user/balance`（Bearer）；响应 `is_available` + `balance_infos[]`（currency/total_balance/granted_balance/topped_up_balance）；
   - Kimi：`api.kimi.com`/`kimi.com`/`api.moonshot.cn`/`moonshot.cn` → GET `https://api.kimi.com/coding/v1/usages`（Bearer）；
   - MiniMax 国际：`api.minimax.io`/`minimax.io` → GET `https://api.minimax.io/v1/api/openplatform/coding_plan/remains`（Bearer）；
   - MiniMax 中国：`api.minimax.cn`/`minimax.cn`/`www.minimax.cn` → GET `https://www.minimax.cn/v1/token_plan/remains`（Bearer）；
   - 智谱：`bigmodel.cn` → GET `https://bigmodel.cn/api/monitor/usage/quota/limit`（**裸 key，无 Bearer 前缀**，与上游一致）；
   - Z.ai：`api.z.ai`/`z.ai` → GET `https://api.z.ai/api/monitor/usage/quota/limit`（裸 key）；
   - OpenCode：`opencode.ai` → GET `https://opencode.ai/zen/go/v1/usage`（Bearer）。
3. 六态分类（CodeWave 口径移植 + CraftWave 改写）：
   - ok；error = 超时/连接失败/5xx/429/解析失败，或曾成功过的 401/403/404；
   - rejected = 401/403/404 且该家从未成功过（密钥/套餐被拒）；
   - no_key = 命中适配器但 apiKey 缺失；
   - unsupported = 主机未命中白名单（reason: no_adapter）或 baseUrl 为空（reason: empty_base_url）；
   - invalid = CodeWave 定义为 keyring 回读失败，CraftWave 无 keyring——**重定义**为保留枚举位（Key 形态不可用，当前无稳定触发路径），随 Answer **回写 §11.5 注记**；
   - 百分比一律一位小数取整、收敛 0–100（CodeWave round1/clamp_percent 口径）。
4. `quota_snapshots` 命令（§3.3 #53）：`{} → { providerId, state, data?, fetchedAt }[]`；
   - 调用时并发实时拉取（每家独立超时，一家失败不影响其余）；
   - 排序 = 配置序、可查询类在前 unsupported 殿后（CodeWave 口径）；5min 周期刷新由前端 visible 门控驱动，不建后端定时器。
5. 快照持久化 `quota.json`（§7.7：`[{ providerId, state, data?, fetchedAt }]` 原子写）：
   - 每次拉取成功/失败更新对应行；
   - 增 `lastOkAt`（CodeWave last_ok_at 口径：上次成功取数时刻，区分"刚才还好好的"与"从来没成功过"；从未成功恒 None）——字段 §7.7 未列，**回写 §7.7**。
6. 失败静默（FR 原文"静默降级，不阻塞任何功能"）：拉取错误不 emit 事件、不弹 toast；日志 warn 级且脱敏。

### 前端（ui/）

1. Inspector 增"配额"页（§10.7 M2 五页之一）：
   - 首屏 = **当前会话所用 provider** 卡（解析自当前 runConfig 的 provider；无活跃会话用全局 activeModelId 归属）；
   - 其余供应商折叠列表。
2. 行态渲染（CodeWave QuotaSection 口径移植，antd → 现有 Tailwind 组件）：
   - ok = 名称 + 剩余% + 进度条，点整行展开窗口明细；
   - error / rejected = 警示/灰行，展开原因 + 重试按钮 + lastOkAt 时间行；
   - no_key = 标签行 + "去设置"（跳设置页对应供应商）；
   - unsupported = 灰行"暂不支持查询"占位（不报错，>3 家折叠为一行汇总）。
3. 刷新策略（FR 逐字）：配额页**可见时**每 5min 周期 + 手动刷新按钮 + 切换 provider 即时；不可见即停（visible 门控）；离线兜底 = 显示最近快照 + fetchedAt/lastOkAt 标注"数据获取时间"。

### 测试

- Rust：每家适配器 parse_* 纯解析单测（fixture：正常/缺字段/数字字符串/超期窗口）；host 匹配表驱动（子域/端口/@/大小写陷阱）；六态分类表驱动（mock HTTP server：200 / 401 首次 / 401 曾成功 / 超时 / 解析失败 / 无 Key / 未命中）；quota.json 原子写与行合并。
- 前端：六态行渲染、刷新周期与 visible 门控、切 provider 即时刷新、契约测试 #53。

## 边界（不在本票）

- 成本统计（FR-02-14，M3）；OAuth 订阅查询（§11.6 P3）；配额耗尽的主动通知/提醒（无此需求）；CI 跑真实供应商端点（只跑 mock，真实两家为人工验收）。

## 验收

- [ ] 至少两家真实供应商（建议 DeepSeek + 智谱，Key 自备）卡面余额/额度与官网一致（**留人工验收**：自动化只覆盖 mock 面（票面边界即如此定义），agent 无真实 Key 无法联网比对；见决议记录 7）
- [x] 未适配供应商显示占位不报错；命中但无 Key 显示 no_key 行（`unsupported_rows_never_send_requests_and_sort_last` / `missing_key_yields_no_key_row_without_request`；前端 `unsupported 态灰行占位` 用例）
- [x] 断网后打开配额页显示最近快照与获取时间；聊天与其他面板功能零影响（自动化等价面：失败行继承最近成功明细 + lastOkAt 标注（`auth_failure_after_success_is_error_with_last_ok_at_and_history_entries` / `commit_overwrites_ok_rows_and_failure_rows_inherit_history`），前端展开区「获取的最近数据」标注（QuotaPanel error 态用例）；静默降级无 toast/事件；真断网属人工面）
- [x] 错误 Key 呈 rejected 态且可展开原因；曾成功后失效呈 error 态并带 lastOkAt（`first_auth_failure_is_rejected_and_recorded_without_history` / `classify_failure_rejects_only_never_succeeded_auth_statuses` / 前端 rejected 态用例）
- [x] 切换 provider 配额卡即时刷新；契约测试 #53；§7.7/§11.5 回写完成（QuotaPanel「切换 provider 即时刷新」用例；#53 由 ipc_commands! 单一事实源 + manifest 双向扫描 + ts-rs 生成物钉住；§7.7/§11.5 已回写 v2.26）

## 决议记录

1. **quota.json 格式按 §7.7 字面实现顶层数组**（`[{ providerId, state, data?, fetchedAt, lastOkAt? }]`）：CodeWave 的 quota.json 是 `{version, verified[]}`（只记「曾成功过」），与 CraftWave §7.7 的「快照行数组」是两个东西——按字段级设计文档为准移植语义（曾成功判据 / 行合并）而非格式。快照是可重取缓存面，无 version 字段；损坏备份 `.corrupt` 后按空继续（宁缺勿炸，§7.7 stats 惯例）。
2. **invalid 态保留枚举位**：CodeWave 定义为 keyring 回读失败；CraftWave 无 keyring（v2.5 口径 Key 明文存 config.json），无稳定触发路径——六态 wire 面保留 `invalid`（前端标签「密钥不可用」），已回写 §11.5 注记。
3. **失败行继承最近成功明细（离线兜底的落地面）**：票面「断网后打开配额页显示最近快照与获取时间」落在 error/rejected 行——`quota.json` 行的 `data.entries` 恒为最近一次成功明细（失败行落盘时继承旧行），`lastOkAt` 标注时点；前端展开区显示明细并标「以上为 <时间> 获取的最近数据」。§7.7 回写已含此不变式。
4. **#53 契约无 active 参数 → 后端不做 active 置顶**：排序 = 配置序、可查询类在前 unsupported 殿后（CodeWave 的 active 置顶不移植）；「首屏 = 当前会话所用 provider」的编排由前端完成（会话覆盖模型 > 全局 activeModelId 归属，`providerOfModel` 纯函数 + 测试锚点，与后端 §11.1 优先级一致）。
5. **「去设置」最小实现 = 打开设置页供应商页**：ProvidersPage 无单供应商定位参数，未做卡级滚动/展开定位（M2 验收口径为「跳设置页对应供应商」的入口可达；卡级定位属增强，如需后续补 ProvidersPage 的 initialProvider prop）。
6. **适配器移植保留点（逐家核对 CodeWave const）**：智谱/Z.ai 裸 key（无 Bearer 前缀）；MiniMax 国际/中国 `current_*_usage_count` 计数语义相反（国际=剩余、中国=已用，勿按字段名直觉改）；OpenCode `status: rate-limited` 只作诊断不过滤（用满窗口最该被看到）；DeepSeek 只认 CNY/USD、total 缺失退化 granted+topped 之和；前端进度条低于 20% 转红（「余额不足前有感知」的呈现面）。
7. **真实两家验收留人工**：票面边界明言「只跑 mock，真实两家为人工验收」——自动化覆盖 = axum mock 的 fetch_json HTTP 面（200/401/500/不可达/非 JSON）+ FakeFetcher 六态分类 + 各家 parse_* 纯解析 fixture；DeepSeek + 智谱真实 Key 比对需用户执行。
8. **刷新编排全在前端**（票面「5min 周期刷新由前端 visible 门控驱动，不建后端定时器」的实现确认）：配额页 = Inspector 条件渲染的第三 tab（卸载即停 = visible 门控）；挂载/切 provider 同入口即时拉；手动刷新按钮。store.load 并发去重（loading 守卫）。
