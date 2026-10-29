# 实现：web_search 工具（本地优先混合，三通道）

Type: task
Status: open

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

- [ ] 启用 SearXNG 后搜索走本地（请求不经应用代理），停用即时回落抓取
- [ ] 未启用 SearXNG 时默认走直接抓取（走代理），解析结果含标题/链接/摘要
- [ ] 抓取失败且 Tavily Key 已配时自动落 Tavily，工具卡标注来源通道
- [ ] 三层全失败返回结构化错误（逐层原因 + 配置引导），不崩不挂
- [ ] SearXNG 开关/地址、Tavily Key 增删即时生效；连通测试按钮可用
- [ ] §8.3 web_search 行按定稿回写设计文档；fixture/回放测试 + 检查链全绿
