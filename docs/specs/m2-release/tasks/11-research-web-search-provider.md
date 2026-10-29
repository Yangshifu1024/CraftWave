# 调研：web_search SearchProvider 选型（C-12）

Type: research
Status: resolved

## Question

C-12 待定项（设计文档建议默认：内置 Brave/DuckDuckGo API（用户自填 Key，BYOK）+ 可经 MCP 搜索服务扩展，不做内置代理）。2026 年现状核实：

1. Brave Search API、DuckDuckGo（注意其官方 API 现状）、及候选（Serper、Tavily、Exa、SearXNG 自托管等）的定价/限额/条款/区域可用性；
2. rig 生态或社区是否已有现成 SearchProvider 实现可借鉴；
3. BYOK 体验与代理环境下的可达性（用户本机 127.0.0.1:7890 代理为常态）。

结论给出首选/次选排序与理由，回写附录 C-12 建议默认方案（13 号票按此落地）。

## Answer（2026-10-29 调研定稿）

**一句话结论：首选 Tavily、次选 Brave，弃用 DuckDuckGo，保留 MCP 搜索服务双通道。**

### 1. 各候选定价 / 额度 / 条款（官方一手来源，2026-10 现状）

| Provider | 免费额度 | 付费定价 | 条款/许可要点 |
|---|---|---|---|
| **Tavily** | Researcher 档 1,000 credits/月，**无需绑卡**，每月 1 日重置 | Basic search 1 credit/次、Advanced 2 credits/次；PAYG $0.008/credit；Project $30/月 4,000 credits；更大档 Bootstrap $100/15k → Growth $500/100k | 面向 AI agent 设计（search/extract/crawl，raw_content 直出）；学生免费 |
| **Brave Search API** | Search 档含 $5 免费额度/月（≈1,000 次查询） | Search $5/1k 请求（≤50 QPS）；Answers $4/1k + $5/M tokens；预付费 credits、无订阅 | **要求绑卡**（反欺诈，不扣费）；**未购 storage rights 计划禁止存储 API 结果**——本地优先应用把结果写进聊天历史属灰区，需文档标注 |
| **Serper** | 注册一次性送 2,500 次（非每月） | 阶梯：$1.00/1k → $0.75/1k（50 万）→ $0.50/1k（250 万）→ $0.30/1k（1,250 万，300 QPS） | Google SERP 转售；直连被 Cloudflare 403（见 §2） |
| **Exa** | 约 $10/月免费额度（官网称"$120 credits/年"） | Search $4/1k 请求；Contents $1/1k 页；News $15/1k | 语义/神经搜索定位；提供官方 MCP server |
| **SearXNG** | —（自托管，无 Key） | — | **AGPL-3.0**，39k stars；元搜索聚合上游引擎，上游（Google 等）在大陆的可达性问题转嫁给部署者；可作为高级用户选项而非内置默认 |
| **DuckDuckGo** | — | — | 官方 Instant Answer API（api.duckduckgo.com）实测仍返回 202/JSON，但**只含摘要类即时答案、无网页结果列表**，文档多年未维护，实质废弃；非官方抓取库 ddgs（原 duckduckgo_search）README 自认 "**educational purposes only**"，生产使用条款风险高，且 DDG 域名被墙 |

### 2. 大陆直连实测（2026-10-29，用户本机，不走代理）

| 端点 | 结果 | 判定 |
|---|---|---|
| api.tavily.com | GET 200；POST /search 无 Key 返回 401（0.8s） | **大陆直连可用**（唯一一家） |
| api.exa.ai | POST /search 无 Key 返回 402（1.8s） | 直连可用 |
| google.serper.dev | GET/POST 均 403（Cloudflare 拦截） | 需代理 |
| api.search.brave.com | 10s 超时 | **必须代理** |
| api.duckduckgo.com | 超时 | 必须代理 |

用户常态走 127.0.0.1:7890，Brave/Serper 走代理均可用；但"直连可达"是代理故障时的韧性冗余，目前只有 Tavily/Exa 具备。

### 3. rig 生态现状

- rig-core 上游 max stable **0.45.0**（与本仓库 Cargo.lock 一致），newest 0.46.0-rc.1（2026-10-23）。
- rig-core **无内置 web search 工具**：`crates/rig-core/src/tool/builtin/` 仅 `think`。0.46-rc 新增的 `rig-tools` crate 是编码工具（read/write/edit/**本地文件搜索**/shell），并非 web search。
- 官方 examples 无任何搜索 provider 示例；但有 `rmcp` 示例与 `rig-rmcp` crate——**MCP 通道有官方路径可借鉴**。
- 社区 crate 均不成熟：`websearch` 0.1.1（16.7k 下载，2025-09 后未更新，含 DDG 抓取）、`agent-search` 0.9.0（仅 602 下载）、`riglr-web-tools`（Solana 生态、依赖重）。**结论：SearchProvider trait + 每个 provider 一个薄 HTTP 封装自研**，集成成本低（各家都是单端点 JSON API），无需引入外部 crate。

### 4. 排序与理由

1. **首选 Tavily**：① BYOK 体验最好——免费档无需绑卡，填 Key 即用，1,000 credits/月（=1,000 次 basic 搜索）对本地桌面 agent 足够；② 为 AI agent 设计（basic/advanced 深度档、raw_content 省一次抓取）；③ **大陆直连可用**（候选中唯一），代理故障不影响核心搜索；④ 条款面向 agent 用例，无 Brave 式存储限制。
2. **次选 Brave**：① 独立索引（非 Google/Bing 转售），结果质量与独立性最好，$5/月免费额度（≈1,000 次）；② 扣分三项：直连被墙必须代理（用户常态可用但无冗余）、要求绑卡（BYOK 摩擦）、未购 storage rights 禁止存储结果（本地优先应用聊天历史持久化有合规灰区，需在设置页文档标注）。
3. **备选 Exa**：语义搜索定位不同（更擅长"找概念/找论文"），大陆直连可用、有官方 MCP server；定价略高（Search $4/1k + Contents $1/1k），建议作为第三引擎留待后续按需接入，不进 M2 内置默认。
4. **不内置 DuckDuckGo**：官方 API 实质废弃（只剩 Instant Answer），非官方抓取条款风险 + 被墙，与"BYOK 正规渠道"的产品定位直接冲突。
5. **Serper 不内置**：免费额度一次性、直连被 Cloudflare 拦、Google SERP 转售条款稳定性弱于 Brave 自有索引；有需求的用户可经 MCP 通道自接。

**MCP 搜索服务双通道：保留。** rig 侧有 `rig-rmcp` 官方支持；Tavily/Exa 均有官方 MCP server，自托管 SearXNG 也可经 MCP 或自定义 base URL 接入——正好落实"不做内置代理、高级用户自担"的边界。

### 5. 建议回写附录 C-12（13 号票落地）

将原"内置 Brave/DuckDuckGo API（用户自填 Key，BYOK）+ 可经 MCP 搜索服务扩展；不做内置代理 | 待定"修订为：

> 内置 **Tavily（默认引擎）/ Brave（备选引擎）** API（用户自填 Key，BYOK 一致）+ 可经 MCP 搜索服务扩展；不做内置代理；DuckDuckGo 因官方 API 实质废弃且非官方抓取有条款风险而移除。§8.3 工具表 `web_search` 行同步改为 Tavily/Brave。

### 证据链接

- Brave 官方定价（Search/Answers 档、$5 免费额度、绑卡与存储条款）：https://brave.com/search/api/
- Tavily 官方定价（1,000 免费 credits/月、无需绑卡、各档价格）：https://www.tavily.com/pricing ；credit 计费规则（Basic 1 / Advanced 2）：https://docs.tavily.com/documentation/api-credits
- Serper 官网（2,500 次免费、阶梯 $1.00→$0.30/1k）：https://serper.dev/
- Exa 官方定价（$120/年免费额度、Search $4/1k）：https://exa.ai/pricing
- SearXNG 许可（AGPL-3.0）：https://github.com/searxng/searxng
- ddgs 非官方抓取免责声明（"educational purposes only"）：https://github.com/deedy5/ddgs （README Disclaimer 节）
- rig 源码核查（builtin 仅 think、rig-tools 为编码工具、rmcp 示例）：https://github.com/0xPlaygrounds/rig （crates/rig-core/src/tool/builtin/、crates/rig-tools/Cargo.toml、examples/）；版本：https://crates.io/crates/rig-core ；生态索引：https://github.com/0xplaygrounds/awesome-rig
- 社区 crate 现状：https://crates.io/crates/websearch 、https://crates.io/crates/agent-search
- 大陆直连实测：本文件 §2 表格，2026-10-29 于用户本机（大陆网络、无代理）执行 curl 所得

## Comments

- **用户决策（2026-10-29）**：不满意云 API 路线，要求本地工具优先。定案方案三（本地优先混合）：设置页独立 SearXNG 配置组（启用开关/地址/连通测试等），启用且探活成功走本地 JSON API；未启用默认使用内置直接抓取；云 API（Tavily/Brave）降为高级区 BYOK 默认隐藏；MCP 搜索通道保留。本票调研事实（定价/可达性/条款）仍然有效，支撑该定案。已同步：附录 C-12 改「已定」、13 号票按定案重写。
- **用户决策二（2026-10-29，查最新定价后）**：确认免费档 1000 credits/月不绑卡、直连可用后，拍板 Tavily provider 随 13 号票一并实现（原"仅预留"升级为在票），三层优先级钉死 SearXNG → 内置抓取 → Tavily，"给用户多种选择"。Brave 维持留位不实现。13 号票已按此重写，C-12 改三通道定稿。
