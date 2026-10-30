# 实现：web_fetch / http_request 与 SSRF 防护

Type: task
Status: resolved

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

- [x] 真实网页 web_fetch readability/raw 两模式端到端可用（含中文页）。（readability=dom_smoothie 提取，中文/英文仿真快照入库断言标题、正文四段全提取与导航/相关推荐/评论/页脚/脚本噪声剔除——web_fetch_test：readability_extracts_chinese_article / readability_extracts_english_article；raw 模式清洗控制字符 + compact 头 4KB/尾 8KB 截断；端到端经本地 fixture HTTP 服务全链驱动（票面测试条款与 map.md 站定偏好一致：测试不连真实外网），e2e confirm_each_suspends_then_allow_completes_real_fetch 真实走完 SSRF 校验→抓取→结果回喂）
- [x] SSRF 向量测试集全拒，E_SSRF_BLOCKED 错误类别可读。（ssrf_test 判定核向量：IPv4/IPv6 拒绝段全集含私网/回环/链路本地/CGNAT/ULA/未指定/组播/IANA 保留（ipv4_deny_vectors/ipv6_deny_vectors）、部分解析私网整体拒绝（partial_private_resolution_rejects_whole）、scheme 白名单与 IP 字面量请求前拦截（url_scheme_and_ip_literal_vectors）；集成面：IP 直连拒（fetch_rejects_loopback_ip_by_default）、解析到回环拒（fetch_rejects_domain_resolving_to_loopback）、跨源剥 Authorization（fetch_strips_authorization_across_origins）、重定向超限拒（fetch_rejects_redirect_loop_over_limit）、50MiB 双保险拒（fetch_rejects_oversized_content_length/oversized_stream）；message 一律带中文类别名（「私网地址」「回环地址」「重定向超限」等，§3.6）
- [x] http_request 重定向逐跳校验、大响应 spill 与 §8.8 一致。（重定向=关闭 reqwest 自动重定向后的 fetch 手动循环，每跳 check_url + 节流 + 降级拒 + 跨源剥头，ssrf_test next_hop_downgrade_rejected/next_hop_cross_origin_strips_sensitive_headers/fetch_follows_redirect_chain_when_permitted；spill=复用 tools/command 的 spill_full 原语，>96KB 全量落 logs/spill/<callId>.txt + 尾 16KB + spillPath，http_request_test oversized_body_spills_with_tail）
- [x] plan 档不见两工具，confirm_each 档调用弹审批。（web_gate_e2e_test：plan_tier_filters_web_tools_from_gate 钉 plan 档两工具不可见；confirm_each_suspends_then_allow_completes_real_fetch 全链：Suspend 挂起 → resolve_ask allow → 放行执行真实抓取；tools_registry_test 钉 Network 能力走既有矩阵，gate.rs 零改动）
- [x] §8.3/§3.2/§3.6/§9.2 回写完成，检查链全绿。（技术设计文档 v2.19：§3.2 AppConfig 增 network 组、§3.6 增 E_SSRF_BLOCKED 行、§8.3 web_fetch 行回写 dom_smoothie 定案与两模式结果形状、§9.2 扩写五步防线字段级；检查链七项：cargo fmt --check / cargo build / cargo clippy --all-targets（0 警告）/ cargo test（Rust 489：lib 447 + backend_e2e 30 + tools_registry 5 + spike 7）/ pnpm typecheck / pnpm test（前端 256）/ pnpm lint（0 错误 0 警告）全过）

## 决议记录（2026-10-30 实现，续作收尾）

1. **Readability 选型定案 dom_smoothie 0.19**（票面候选定案，已回写 §8.3）：纯 Rust（dom_query HTML5 解析）、MIT、活跃维护（0.19.0 于 2026-10 发布）、内置 Markdown 输出（TextMode::Markdown）；备选 `readability` crate 久未维护不取。中文/英文仿真快照实测正文提取与噪声剔除可用。实现注记：Readability 的 Article 非 Send，DOM 解析放 `spawn_blocking` 线程池执行。
2. **DNS 钉址实现 = reqwest `dns_resolver` 自定义解析器**（票面所述「resolve() 钉址」的等价落地）：`SsrfResolver` 替换 reqwest 默认解析器，每次连接前解析并对**全部**解析结果逐一校验、只把已校验地址交给连接层——校验与连接同源，无二次解析窗口（防 DNS rebinding），且每跳重定向的域名自动重走同一 resolver。
3. **重定向策略实现 = 关闭自动重定向 + fetch 手动循环**（票面所述「Policy::custom ≤10 跳」的等价强化落地）：`Policy::none()` 后由 `fetch` 逐跳处理——每跳重跑 URL 预校验（新 host 触发 resolver 重校验）、HTTPS 不降级、跨源剥敏感头、≤10 跳、浏览器语义（301/302/303 对非 GET/HEAD 转 GET 剥 body，307/308 保持方法与 body，票面未明确处按浏览器行为）。
4. **测试不连外网的向量覆盖方式（诚实记录）**：fixture 服务驻留 127.0.0.1 与默认拒绝策略互斥（须 `allow_private_network=true` 才能到达 fixture），故集成向量以「判定核纯函数（check_url/check_ipv4/check_ipv6/check_all_addresses/next_hop_allowed）+ resolver 注入 + 例外策略集成」组合等价覆盖；「重定向到内网被拒」无法在集成面构造（起点即回环、策略全局一致），其拒绝路径与直连共享同一 check_url 入口，由私网 IP 字面量向量（url_scheme_and_ip_literal_vectors）覆盖判定核；「https→http 降级」同理以纯函数 next_hop_downgrade_rejected 覆盖（本地 fixture 无 https 终点）。
5. **50MiB 超限错误码用 E_INVALID_ARG 而非 E_SSRF_BLOCKED**：资源上限是配额保护非安全拒绝，§3.6 的 E_SSRF_BLOCKED 语义限定为「命中保留段/重定向逃逸」；message 均带「50MiB 上限」字样可读（票面未指定该向量错误码，此处定案）。
6. **readability 正文本地 1MiB 截断（READABILITY_CONTENT_LIMIT）**：正常文章几 KB～几十 KB，异常超长先本地截断再走 §8.2 compact 口径（头 4KB + 尾 8KB），避免 50MiB 级正文整个序列化进 data；`length` 字段保留全量字符数供模型判断。web_fetch 结果不 spill（走 compact 截断）；http_request 响应体走 §8.8 spill（96KB/16KB）——两工具口径不同系结果面差异（正文 vs 原始响应体）。
7. **抓取面防御性超时兜底**：连接超时 10s、单请求总超时 60s（含 body 下载）——非票面字段，防止 run 被慢速对端无限挂死；每 host 1s 节流在 fetch 循环内执行，IP 直连同覆盖，节流等待与下载均响应取消。
8. **网络工具执行环境 = `ExecutorEnv.network`（Arc\<NetworkEnv\>）装配注入**：SSRF 策略 + 代理 + 进程级节流器由宿主按 AppConfig 现读装配（`NetworkEnv::from_config`，save_config 后下一次调用生效）；未装配时按工具级失败回喂（纯文件工具测试场景）。子代理环境照常装配（执行入口白名单先拦，策略与主代理同源；配置读取失败按缺省配置 fail-closed）。
9. **代理接线收敛点（§9.2 注记兑现）**：HTTP 客户端按 `AppConfig.proxy` 三模式构建（none/未配置=显式禁用代理不探测环境变量；system=reqwest 默认探测，19 号票收敛点完善；manual=Proxy::all(url)，manual 无 url 时参数级拒绝）；每工具调用现建客户端，无全局缓存，19 号票 client 重建路径落地后收编。
10. **WIP 续作修正清单**（续作代理收尾）：tests/tools_registry_test.rs 钉死清单与全保留数量同步 10→12（WIP 遗漏，此前红）；ui/src/ipc/types.ts 增 NetworkConfig 再导出（cargo test 导出生成物后 contract 反向扫描需要）；ui 三处 AppConfig 字面量（DEFAULT_CONFIG 与两测试 fixture）补 network 字段（typecheck 抓缺字段）；toolcards.test 两用例修正断言时序（详情区收起不渲染，须先展开再断言、边车数据 waitFor 等待）；clippy 4 警告清零（derivable_impls/question_mark/manual_range_patterns/let_and_return）。
