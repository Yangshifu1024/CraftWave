# 实现：连接测试、网络代理与本地模型（FR-02-09 + FR-02-10 + FR-02-04）

Type: task
Status: resolved

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

- web_fetch/web_search 的 SSRF 防护与代理细节（12/13 号票）；per-provider headers 的请求侧接线（FR-02-11，字段 M1 已就位、未排票，如需 M2 落地另行立票）；OAuth（§11.6 P3）；anthropic_messages / openai_responses 协议连通测试与实际对话接入由 38 号票接通本票探测入口，不再指向无承载的“§11.1 票”。

## 验收

- [x] manual socks5 模式经本地代理实测对话成功；切 none 直连恢复；保存后下一次对话即生效（热生效实测）——自动化等价面：probe_test 的 socks5 e2e（手写最小 SOCKS5 转发器 + axum mock，计数器证明请求经代理、切 none 后不再经代理）+ http_test 工厂 reload 换代断言；真实人工对话流由用户终局关验收
- [x] test_provider：错误 Key 返回 auth 分类、不可达地址返回 network 分类、正常端点返回模型列表 + 延迟（probe_test 七例 + 402/429/400/500 分类表）
- [x] Ollama 预设免 Key 添加后端到端对话成功——预设免 Key 实例化与归一化放行有测试；「本机 `ollama serve` 真实对话」与「断网下本地模型对话不受影响」为用户真机验收面（本代理环境无 ollama 服务，不假绿）；断网影响面自动等价断言 = 免 Key 本地端点不经代理工厂的 system 探测（none 直连语义）
- [x] LM Studio 经 OpenAI 兼容端点接入可用——预设（`http://127.0.0.1:1234/v1` + openai_chat 兼容方言）就位，与 Ollama 共用同一注入面与探测路径
- [x] 契约测试 #32；§3.2 ProviderConfig.local、§11.1 注入口径回写完成（manifest 锚点双侧 + ipc_manifest_test 契约类型锚；文档回写 v2.24）

## 决议记录

- **代理三模式语义定稿**：`none` 与 `proxy` 字段未配置一律 `no_proxy()` 显式直连（防环境变量泄漏，票面口径）；`system` = reqwest 内建探测，本票给 reqwest 补开 `system-proxy` feature（0.13 该 feature = hyper-util 的 macOS SystemConfiguration / Windows 系统设置探测）——12 号票遗留收敛点（"system 只有 reqwest 默认行为"）由此落地：`safety/ssrf.rs::build_client` 的代理段改调 `providers/http.rs::apply_proxy`，两侧语义完全一致。**推论**：UI 缺省高亮从 M1 的"系统代理"改为"不使用代理"（`config.proxy?.mode ?? "none"`），对齐后端 None=直连语义，否则未配置代理的用户看到"系统代理"高亮而实际直连。
- **热生效边界**：`save_config` 成功 → `AppState.apply_proxy_config()` 重建工厂缓存（generation+1）；进行中 run 不切换（客户端按 run 构建，§11.1）；运行中切模型（18 号票 refresher）重建客户端时按当时代工厂快照取用——是"换模型"路径的固有行为，不是代理切换路径。
- **草稿试连的边界**：#32 契约 `{ providerId, baseUrlOverride? }` 要求 providerId 已存在——「未保存草稿直接测试」实现为**编辑弹框**内的草稿地址覆写探测（baseUrlOverride 传表单当前值，不落库）；新增态的全新供应商没有配置主体（providerId 后端查不到），保存后从卡片测试。卡片与编辑弹框各一份测试状态（探测动作不落盘，不入 settings 草稿，对齐 13 号票搜索连通测试定案）。
- **保存不强制阻塞**：FR-02-09 的"保存前"实现为引导而非强制——添加弹框文案"建议保存前测试"，无校验拦截（本地/离线场景必须可保存，票面取舍原文）。
- **免 Key 判定**：仅按 `ProviderConfig.local` 显式判定（serde default false，schemaVersion 维持 2）；不做"baseUrl 是 localhost 即本地"的隐式推断（远端自建网关同样可能是回环地址）。rig 侧必填 Key 以占位值 `"local"` 过面（本地端点忽略 Authorization 头）。
- **本地预设取舍**：三家全走 OpenAI 兼容端点（Ollama `/v1`、llama.cpp server `/v1`、LM Studio `/v1`），零新方言代码；llama.cpp server 不校验 model 值（占位名 `local-model` 即可用），LM Studio 需按实际加载模型名修改（预设仅开箱建议值）。UI 预设入口 = 添加弹框顶部预设 chips（M1 的 `newProviderFromPreset` 有实例化辅助但无 UI 入口，本票补上）。
- **协议边界**：`test_provider` 仅 openai_chat；anthropic_messages/openai_responses 的按钮禁用 + 提示，38 号票三协议接入接通本探测入口（票面边界原文）。
- **reqwest feature 扩张**：`socks`（socks5/socks5h 代理支持）与 `system-proxy`（系统代理探测）为本票功能面必需；`arc-swap`（1.9，锁面已有）与 `rig-reqwest`（0.45.0，锁面已有）为票面点名载体，均不新增下载。
- **模型请求不设整体超时**：代理工厂 client 仅设 connect_timeout(10s)——流式回复可达分钟级；探测请求经 `RequestBuilder::timeout` 按请求覆盖 30s。
