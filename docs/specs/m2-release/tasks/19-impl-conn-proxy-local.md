# 实现：连接测试、网络代理与本地模型（FR-02-09 + FR-02-10 + FR-02-04）

Type: task
Status: open

## 要做什么

保存供应商前能一键验证连通性并拿到**分类**错误；代理三模式真正作用于模型请求（config.proxy 的设置 UI M1 已有，"代理生效链路随后续票"正是本票）；开箱接入本地模型（Ollama / llama.cpp / LM Studio），离线可用。

FR 依据（原文摘引）：
- FR-02-09：保存前连通性验证（models 列表拉取或最小补全），明确错误分类提示
- FR-02-10：none/system/manual 三模式；系统代理探测；socks5/socks5h；保存热生效
- FR-02-04：Ollama、llama.cpp（LM Studio 走兼容端点）接入预设；离线可用
- 附录 B：代理经 rig-reqwest builder；§3.3 #32：`test_provider { providerId, baseUrlOverride? } → { ok, latencyMs?, models?: string[], error? }`

## 具体做什么

### 后端（src-tauri/）

1. `test_provider` 命令（§3.3 #32），落 `providers/probe.rs`（不依赖 Tauri 可单测）：
   - 探测策略：OpenAI 方言先 `GET {base}/models`（带 Key）；404/不支持时回退**最小补全**（max_tokens=1 的 chat 请求——FR 文"models 列表拉取或最小补全"两策略皆合法）；
   - 返回 latencyMs（首响应耗时）、models（拉取成功时）、error（`{ code, message }` 脱敏，分类复用 §11.3/§3.6 七类：auth/billing/rate_limited/bad_request/server/protocol/network）；
   - 探测请求走代理工厂（见 2）。
2. 代理 client 工厂：新增 `providers/http.rs`——进程级 `reqwest::Client` 缓存（ArcSwap），按 `AppConfig.proxy`（`providers/types.rs:127` 既有 ProxyConfig mode/url）构建：
   - none → `ClientBuilder::no_proxy()` 显式禁用（防环境变量泄漏，语义=直连）；
   - system → reqwest 默认行为（env `http_proxy/https_proxy` + macOS SystemConfiguration / Windows 系统设置——"系统代理探测"即此内建探测，不重复造轮子）；
   - manual → `Proxy::all(url)`；url 支持 `http://`/`https://`/`socks5://`/`socks5h://`（reqwest socks feature；socks5h = 远端 DNS 解析）；
   - proxy.url 校验进 `AppConfig::normalize`（非法 scheme/地址 → E_INVALID_ARG）。
3. rig 注入（附录 B"代理经 rig-reqwest builder"），注入点已勘定：
   - rig-http 0.45：`ReqwestClient::from(client)` 包装配置好的 reqwest::Client、`DynHttpClient::new(client)` 擦除、provider 配置以 `.connect(client)` 接收（rig-http 0.45.0 `src/http_client/mod.rs` 弃用注记原文）；
   - `resolve_model` 改经工厂取 client 注入；具体签名以锁版 rig-http 0.45.0 源码为准，出入记 Answer。
4. 保存热生效（FR-02-10）：config 协调器保存成功 → 工厂重建缓存 client（generation 计数）；进行中 run 不切换（客户端按 run 构建，§11.1），下一次 start_chat 用新代理。
5. 本地模型预设（FR-02-04），`providers/presets.rs` 增三家，均 openai_chat 方言 + 无 Key：
   - Ollama `http://127.0.0.1:11434/v1`；llama.cpp server `http://127.0.0.1:8080/v1`；LM Studio `http://127.0.0.1:1234/v1`（LM Studio 走 OpenAI 兼容端点，PRD 括号原文口径）；
   - **取舍**：走各家 OpenAI 兼容端点而非 rig 原生 ollama 方言（零新方言代码、行为一致）；若评审要求原生 `/api/chat` 先回写 §11.1；
   - 预设结构加 `local: bool`（"离线可用"标注 + 免 Key 依据）；前端 `settings/presets.ts` 同步副本，漂移测试照旧。
6. 免 Key 校验：`resolve_model` 现 apiKey 缺失即报错（chat.rs:156）——`ProviderConfig` 增 `local?: bool`（serde default，schemaVersion 维持 2，**回写 §3.2**），local=true 允许无 Key；远端仍强制；不做"baseUrl 是 localhost 即本地"的隐式推断。

### 前端（ui/）

1. ProvidersPage：供应商卡片增"测试连接"按钮 + 结果行（延迟 / 模型数 / 错误分类文案——七类→人话文案映射表）；支持对**未保存草稿**直接测试（baseUrlOverride 传草稿地址，不落库）；
2. 保存流程不强制阻塞（本地/离线场景必须可保存），按钮旁文案"建议保存前测试"——FR-02-09"保存前"的实现取舍记 Answer；
3. NetworkPage：三选 + URL 输入 M1 已有（SettingsPage.tsx NetworkPage），增：manual 模式 scheme 即时校验与错误就地展示；说明文案标注"保存后下一次对话生效"；
4. 本地预设卡：预设选择器标注"本地 / 离线可用"，免 Key（apiKey 输入隐藏）。

### 测试

- Rust：probe mock HTTP server（models 成功 / 404 回退最小补全 / 401→auth / 超时→network / 延迟计量）；代理工厂三模式构建（manual http/socks5/socks5h URL 解析、非法 URL 拒绝、none 显式 no_proxy）；本地预设免 Key 解析 roundtrip；normalize 对 proxy.url 校验。
- 前端：测试按钮状态机（idle/testing/ok/error）、分类文案、本地预设表单、契约测试 #32。

## 边界（不在本票）

- web_fetch/web_search 的 SSRF 防护与代理细节（12/13 号票）；per-provider headers 的请求侧接线（FR-02-11，字段 M1 已就位、未排票，如需 M2 落地另行立票）；OAuth（§11.6 P3）；anthropic_messages 方言连通测试（方言接入本身未落地，随 §11.1 票）。

## 验收

- [ ] manual socks5 模式经本地代理实测对话成功；切 none 直连恢复；保存后下一次对话即生效（热生效实测）
- [ ] test_provider：错误 Key 返回 auth 分类、不可达地址返回 network 分类、正常端点返回模型列表 + 延迟
- [ ] Ollama 预设免 Key 添加后端到端对话成功（本机 `ollama serve`）；断网下本地模型对话不受影响
- [ ] LM Studio 经 OpenAI 兼容端点接入可用
- [ ] 契约测试 #32；§3.2 ProviderConfig.local、§11.1 注入口径回写完成
