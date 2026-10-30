# 实现：web_search 工具（本地优先混合，三通道）

Type: task
Status: resolved

Blocked by: 11（已解决：C-12 定案见票 Answer 与 Comments）

## 要做什么

按 C-12 定案（用户决策 2026-10-29：本地优先混合）落地 web_search，把 §8.3 的 SearchProvider trait 做实为三个实现。对 AI 始终是同一个 `web_search` 工具，按优先级自动选通道，用户按需配置、零配置也可用。

## 具体做什么

### 后端（src-tauri/）

- **通道 1：SearXNG（用户显式启用才用）**——`web_search` 执行器内 provider 链第一环：
  - 配置：config 增 searxng 域（`enabled: bool`、`base_url` 默认 `http://127.0.0.1:8888`、`max_results`）；
  - 启用且探活成功才使用：调用 `GET {base_url}/search?q=…&format=json`（SearXNG settings.yml 需开 `format=json`——配置文档与错误提示都要引导这一点）；请求走本地回环，不经应用代理；
  - 未启用或探活/请求失败 → 落通道 2，不自动启用。
- **通道 2：内置直接抓取（默认兜底）**——DuckDuckGo HTML 版（`html.duckduckgo.com/html`）GET + 解析结果列表（标题/URL/摘要）；必须走应用代理（19 号票 reqwest client）；解析器宽松 + 空结果自检告警；引擎改版风险为已声明取舍（个人自用口径）。
- **通道 3：Tavily（BYOK，高级区，Key 已配才参与）**——`POST https://api.tavily.com/search`，Bearer Key，JSON 响应；`search_depth=basic`（1 credit/次）默认；Key 经 M1 钥匙串协议存储；免费 1000 credits/月用尽即当月停用（结构化错误，不重试）；429 带 Retry-After 单次退避；401/403/配额尽各给明确错误分类；大陆直连可用，仍统一走应用代理设置。
- **优先级（钉死）**：SearXNG 启用且探活成功 → 本地；否则内置抓取；抓取失败/不可用且 Tavily Key 已配 → Tavily；全部失败 → 结构化错误（含逐层失败原因与配置引导），不崩不挂。
- 注册表接线：工具名/参数 schema（`{ query, limit? }`）/权限档（Network）/annotations 按设计文档 §8.3 web_search 行。

### 前端（ui/）

- 设置页：SearXNG 独立配置组（启用开关/地址/连通测试按钮，交互对齐 19 号票连接测试）+ 高级区 Tavily Key 输入框、注册指引文案（tavily.com 邮箱注册、免绑卡、免费 1000 credits/月）、连通测试；
- 工具卡片：结果列表 + 来源链接，标注来源通道（本地/抓取/云）。

### 测试

- SearXNG：本地假服务 fixture（探活、正常、超时三态）；
- 抓取：录制 HTML 快照回放（真实站点改版不挂 CI）；
- Tavily：录制 JSON 回放（正常/429/401/配额尽）；
- e2e：优先级切换（启用/停用 SearXNG 后同查询走不同通道）。

## 边界（不在本票）

- Brave 不实现（trait 位预留）；MCP 搜索服务通道随 08 号票共存，不占内置位；浏览器工具 M3。

## 验收

- [x] 启用 SearXNG 后搜索走本地（请求不经应用代理），停用即时回落抓取
- [x] 未启用 SearXNG 时默认走直接抓取（走代理），解析结果含标题/链接/摘要
- [x] 抓取失败且 Tavily Key 已配时自动落 Tavily，工具卡标注来源通道
- [x] 三层全失败返回结构化错误（逐层原因 + 配置引导），不崩不挂
- [x] SearXNG 开关/地址、Tavily Key 增删即时生效；连通测试按钮可用
- [x] §8.3 web_search 行按定稿回写设计文档；fixture/回放测试 + 检查链全绿

## 决议记录

- **实现落点（2026-10-30）**：`src-tauri/src/tools/web_search.rs` 单文件承载 `SearchProvider` trait（§8.3 抽象面，Brave 留位）+ SearXNG/Ddg/Tavily 三实现 + 优先级编排（`run_chain` 线性回落，成功即返、逐层失败记入 attempts）；执行环境 `SearchEnv` 挂在 12 号票 `NetworkEnv.search` 下（`NetworkEnv::from_config` 一次配齐，save_config 后下一次 start_chat 生效）。注册表静态段 12→13（web_search 尾部追加，Network 能力：plan 档过滤）。
- **Tavily Key 存储协议按现行协议落 config.json（票面"钥匙串协议"的口径修正）**：§9.4 v2.5 用户决策已废除钥匙串，API Key 统一明文存 `~/.craftwave/config.json`（文件 0600）——Tavily Key 沿用同协议（`search.tavilyApiKey`），spec.md「凭据沿用现有存储协议」即此意；不回退到已废除的钥匙串方案。
- **SearXNG 不经 SSRF 管线（§9.2 例外，已回写设计文档）**：用户显式启用即信任该地址，而其默认地址 `127.0.0.1:8888` 天然命中 §9.2 回环拒绝判定，走管线必然被拦；SearXNG 通道用独立 client（`no_proxy` + 3s 连接/15s 总短超时——本地服务挂掉快速落通道 2）。通道 2/3（DDG 抓取、Tavily）仍完整走 `safety/ssrf.rs` 管线（钉址/逐跳重校验/50MiB/每 host 1s 节流，复用 12 号票原语）。
- **DDG 结果解析用 dom_query（dom_smoothie 0.19 的底层 HTML5 解析器，锁面同版本不新增下载）**：CSS 选择器定位 `.result__a` 锚点与 `.result__snippet` 摘要块（属性顺序/引号形式无关），比正则宽松；链接解 `uddg` 跳转包装（url crate `query_pairs`）。**空结果自检三态**：解析出结果 / 页面含无结果标志（"did not match any documents" 等）→ 真空结果（成功空列表，不落 Tavily）/ 0 条且无标志 → 疑似改版或反爬页（通道失败告警，让优先级链继续）——绝不把异常页当真空结果。文本清洗只压空白：dom_query 的 `.text()` 已解码实体且不含标签，再做标签吞除会误吞字面尖括号文本（快照测试钉住）。
- **Tavily 错误分类**：`tavily_classify`（401 Key 无效 / 403 无权限 / 429 限流（单次退避重试一次，Retry-After 优先、缺省 5s 封顶 30s）/ 432 或 429+quota 字样配额尽（当月停用不重试）/ 其余请求失败）；各分类带中文标签进逐层失败原因。
- **新增错误码 `E_SEARCH_UNAVAILABLE`（§3.6 已回写）**：三通道全败的结构化错误——error.message 带逐层失败原因与配置引导（模型通道可读），`detail.attempts` 带结构化 `{ channel, label, tried, error? }[]`（UI 通道，工具卡逐层展示）；执行器对失败结果照常 persist 边车（与 web_fetch 失败不写边车的 12 号票约定不同，本票结构化错误面走 detail）。
- **连通测试命令 `search_test`（§3.3 #59f，已回写设计文档）**：草稿试连不经保存（对齐 08 号票 mcp_test 模式），SearXNG/Tavily 复用工具通道的同一请求路径（测通即用通）；失败不 reject（ok=false + error 带引导），仅未知 target 拒 E_INVALID_ARG；总预算 30s。Tavily 测试消耗 1 credit（设置页指引文案注明）。
- **`limit` 缺省语义（票面 max_results 字段位置的口径澄清）**：工具参数 `limit` 缺省取 `search.searxng.maxResults`（1–30，缺省 10）——该配置作为各通道共用的结果条数缺省（生成为 `SearchEnv.max_results_default` 透出），显式参数覆盖并 clamp 1..=30。票面把 max_results 放在 searxng 域保持不动（字段级设计文档为准）。
- **reqwest `.query()` 未用**：reqwest 0.13 的 query 构造需要未启用的 feature（本仓 features 仅 rustls/gzip），改用 url crate 的 `query_pairs_mut` 拼查询串（`q` 正确 percent 编码）。
- **前端**：设置页网络页增「本地搜索（SearXNG）」独立配置组（启用开关/地址/连通测试）与「云搜索（Tavily，高级）」区（Key 密文输入/注册指引/连通测试）；连通测试走 `search_test` 草稿值不经保存（对齐 MCP 管理页测试连接的草稿态定案），不入 settings 草稿。`WebSearchCard`：查询词摘要 + 来源通道徽标（本地=绿/抓取=蓝/云=紫）+ 结果列表（标题外链/URL/摘要）+ 全败时「通道尝试记录」折叠块。SearXNG 启用后设置页显示 settings.yml `format=json` 引导。
- **测试资产**：`fixtures/duckduckgo_{results,empty,redesign}.html` 三份仿真快照（真实站点结构手工构造，改版不挂 CI）；SearXNG/Tavily 用本地 axum 假服务回放（SearXNG 三态：正常/403/超时（provider 超时可注入毫秒级）+不可达；Tavily 五态：正常/429+Retry-After 退避重试/401/432 配额尽/500）；优先级 e2e 四例（启用 SearXNG 走本地且后续通道不被调/停用回落抓取/抓取疑似改版自动落 Tavily/三层全败结构化错误 + Tavily 未配 Key 跳过 + 真空结果不落 Tavily）。Rust 23 例 + 前端 8 例（工具卡 4 + 设置页 2 + 契约 2 处更新）。
- **既有测试维护（压缩 e2e 窗口上调）**：`tests/backend_e2e.rs` 的 `small_window_config` 窗口 40000→44000——工具注册表每增一工具都抬高请求固定估算面（估算器把工具表计入 §6.2 预算），40000 的硬预算线（×0.9=36000）已被 web_search 挤破（固定面 36058），40000 失去「第一次发送达阈值但硬预算内可容纳」的设计前提；44000（硬预算 39600）恢复该区间且 32768 回复预留 > 0.6×44000 的跨阈值前提不变（三用例在 main 基线复跑通过，为本票回归非既有抖动）。
