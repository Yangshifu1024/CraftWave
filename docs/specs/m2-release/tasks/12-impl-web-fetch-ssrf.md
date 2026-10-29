# 实现：web_fetch / http_request 与 SSRF 防护

Type: task
Status: open

## 要做什么

模型能抓网页正文（web_fetch）与发通用 HTTP 请求（http_request），并把 SSRF 防线一次做实：内网/回环/链路本地/CGNAT 地址与重定向逃逸全部拒掉，为 13 号票 web_search 提供抓取底座。覆盖 FR-05-07 + §8.3 两工具行 + §9.2 SSRF 管道（P1 规格先行，本票实装）。

## 具体做什么

### 后端（src-tauri/）

- 新依赖 `reqwest`（rustls 特性，免系统 OpenSSL；Cargo.lock 依赖树核查后定版本）；HTTP 客户端按 `AppConfig.proxy`（none/system/manual）构建——**接 19 号票 client 重建路径，19 未落地先按 config.proxy 直建并在票面记录收敛点**；每 host 1s 节流（Mutex<HashMap<host, Instant>> 起步，够用不引库）。
- **SSRF 共享层 `safety/ssrf.rs`**（§9.2 五步逐条）：
  1. URL 预校验：仅 http/https；
  2. DNS 解析（`tokio::net::lookup_host`）→ **全地址校验**：拒私网（RFC1918）、回环、链路本地（169.254/16、fe80::/10）、CGNAT（100.64/10）、ULA（fc00::/7）、未指定/组播/保留段；`allow_private_network` 设置例外——承载于 AppConfig 增 `network: { allowPrivateNetwork?: boolean }` 缺省 false（serde default 兼容旧 config）——**字段级定义缺失，先回写 §3.2/§9.2 再实现**；
  3. **连接前再校验**（防 DNS rebinding）：reqwest `resolve()` 把域名钉到已校验 IP 再连，杜绝二次解析换址；
  4. 重定向：`reqwest::redirect::Policy::custom` ≤10 跳、每跳重跑 1–3；HTTPS 不降级（https→http 拒）；跨源剥自定义头（Authorization/Cookie 等敏感头不跨源携带）；
  5. 正文 ≤50MiB（Content-Length 预检 + 流式计数双保险，§8.3 web_fetch 行）。
- **`tools/web_fetch.rs`**（Network，§8.3 行）：参数 `{ url, format?: "readability"|"raw" }`；readability 正文提取 crate **票内定案**——候选 `dom_smoothie`（纯 Rust Readability 移植、MIT、活跃）优先，备选 `readability` crate（久未维护）；定案标准=纯 Rust、MIT、中文正文实测可用，**定案后回写 §8.3 web_fetch 行**；结果 data=`{ url, title, content(markdown), length }`；raw 模式返回清洗 HTML（compact 头 4KB+尾 8KB，§8.2）。
- **`tools/http_request.rs`**（Network，§8.3 行）：参数 `{ method, url, headers?, body? }`（method 限 GET/POST/PUT/PATCH/DELETE/HEAD）；响应截断**同命令输出治理**（§8.8：>96KB 全量 spill `logs/spill/<callId>.txt` + 尾部 16KB + spill 路径，24h lazy 清理——复用 tools/command 既有 spill 原语）；结果 data=`{ status, headers(截断), body }`。
- 错误码：SSRF 拒绝统一 `E_SSRF_BLOCKED`（**新增并回写 §3.6**，message 带命中类别：私网/回环/重定向降级/跨源剥头等）。
- 审批面：两工具 Network 能力走既有矩阵（§8.5——plan 档过滤、confirm_each/auto_edit 审批、full_access 放行），gate.rs 零改动；审批载荷构造（`host/commands/chat.rs`）增 web 分支——command 字段承载 `GET https://…` 摘要行。
- 注册表：`TOOL_NAMES` 8→10（tools/mod.rs），注册表清单钉死测试同步。

### 前端（ui/）

- 两张工具卡（`chat/ToolCards.tsx` + `chat/toolkit.ts`）：web_fetch=URL+标题+正文 Markdown 折叠预览（默认收起，展开前 50 行）；http_request=method 徽标+URL+状态码+响应头折叠+body 折叠+spill 链接（对齐 CommandCard 形态）。
- `CW_TOOL_PARAMETERS` 增两工具 schema，与 manifest toolSpecs 双向锁定（contract.test）。

### 测试

- **SSRF 向量测试集**（本地 fixture HTTP server 驱动，无需外网）：直连私网 IP、域名解析到私网、重定向到内网、https→http 降级、跨源自带 Authorization 被剥、CGNAT/链路本地/IPv6 ULA 各一例、50MiB 超限截断——全拒且错误类别可读。
- readability：入库两份真实网页 HTML 快照（含中文正文页），断言标题/正文提取与噪声剔除。
- 节流：同 host 连发第 2 次间隔 ≥1s（mock clock）；异 host 不受影响。
- e2e：confirm_each 档 web_fetch 挂起→allow 放行全链（复用 M1 14 号票审批 e2e 模式）。

## 边界（不在本票）

- web_search 三通道（13 号票，本票仅交付其可复用的抓取底座）；浏览器工具（M3 §8.10）；代理三模式本体（19 号票）；文档格式解析（16 号票）；JS 渲染页面（无 headless，raw HTML 为限——票面已知取舍）；重试策略复用 §11.3 不单独做。

## 验收

- [ ] 真实网页 web_fetch readability/raw 两模式端到端可用（含中文页）
- [ ] SSRF 向量测试集全拒，E_SSRF_BLOCKED 错误类别可读
- [ ] http_request 重定向逐跳校验、大响应 spill 与 §8.8 一致
- [ ] plan 档不见两工具，confirm_each 档调用弹审批
- [ ] §8.3/§3.2/§3.6/§9.2 回写完成，检查链全绿
