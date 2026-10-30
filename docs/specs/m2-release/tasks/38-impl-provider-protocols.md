# 实现：补齐三协议 Provider 对话链路

Type: task
Status: resolved

## 要做什么

补齐 FR-02-01 已要求的 openai_chat / anthropic_messages / openai_responses 三协议实际对话链路，作为 M1 欠账纳入 M2。当前 chat.rs 的 resolve_model 只接受 openai_chat，18/19 号票指向的协议接入此前无票承载；本票补齐执行归属，不新增协议范围。

## 具体做什么

- 检查已安装 rig 的三协议客户端、请求选项及流事件接口，优先复用现有依赖和防腐层，不手写重复协议客户端；必要的依赖调研结论记本票。
- 按 ProviderConfig.api_format 路由模型构造及请求装配，接通流式文本、工具调用及结果回喂、reasoning、图片、usage 与取消；不支持某项能力的模型给明确反馈，不静默丢弃输入或伪装成功。
- 与 18 号票统一模型/thinking 映射及同一 run 下一请求生效；每次请求走 04/17 号票预算与图片能力校验。与 19 号票统一连接测试、错误分类和代理路径。
- 保持现有 ChatFrame、门控、取消收齐、工具 callId 和存储契约；协议差异在 providers/backend 现有边界收敛。三协议的 usage 按 Q37/Q39/Q40 统一四口径及实际模型归属，不重复累计。
- 实现前补齐技术设计 §11.1/§11.4 中协议适配、选项和能力差异，并回写 18/19 号票接缝。

## 测试

- 独立 _test.rs 文件使用本地 mock SSE/响应 fixture，三协议分别覆盖文本、工具调用往返、reasoning/图片能力及四口径 usage、错误分类、流中取消；不依赖真实 Key 或公网 CI。
- 同一 run 请求间切模型/协议后保持规范化历史、工具配对、预算检查和实际模型用量归属；正在进行的请求不被切换打断。
- 19 号连接测试与实际对话走相同协议选择，不能出现测试成功却因固定 openai_chat 限制无法聊天。

## 边界

- 不新增 OAuth、Key 池、自定义请求头或 M3 协议能力；不把三协议开工前的现状限制写成正式版支持范围。

## 验收

- [x] 三协议配置均可实际对话，受支持的文本/工具/reasoning/图片/usage 链路可验证，不支持能力有明确反馈。
- [x] 取消、审批、模型切换、历史与用量归属的既有契约不回归。
- [x] 本地 fixture 回归及连接测试覆盖三协议；§11.1/§11.4 与 18/19 号票接缝一致，34 号收口引用本票。

## 决议记录

- **依赖调研结论（rig 0.45 三方言可用面）**：rig 0.45 已原生承载三方言——`providers::openai::wire` 的 `OpenAiWire` 枚举统一 Chat（`/chat/completions`）与 Responses（`/responses`）两路线（按 `Route` 配置分派；**OpenAI 官方 dialect 的缺省路线是 Responses**，openai_chat 必须显式 `with_route(Route::Chat)` 强制，否则打错端点）；`providers::anthropic::wire::Messages` 承载 Messages 方言（`/v1/messages`，`x-api-key` + `anthropic-version` 鉴权，baseUrl 自动剥 `/v1`、`/messages`、`/v1/messages` 尾缀防双路径）。三 wire 的 Payload=`Encoded`、Frame=`WireFrame` 同形——代理工厂 HTTP client 经 rig 的 blanket `Transport` 实现直接共用，**零手写协议客户端**；新增代码仅一层枚举防腐（`providers/wire.rs::DialectWire`，照 rig `route.rs` 的 `OpenAiWire` 先例逐变体委托 Event/Decoder/Reassembler），使 `Model<DialectWire>` 单类型贯穿 TurnDriver/drive_run 全链（37 号票 `Model<W: Wire, T: Transport>` 泛型化的生产收口）。
- **Anthropic 请求级 max_tokens 恒注入**：实现中发现 rig 对 Messages wire 有硬校验——目录外模型（第三方 Anthropic 兼容网关）无按模型缺省可兜，请求缺 max_tokens 即 `EncodeError: "max_tokens must be set for Anthropic"` 拒发。因此 `RunSpec.max_tokens` 按方言注入：Anthropic 恒带（有 thinking 上调规则时用上调值 `min(contextWindow, budget+原maxTokens)`，否则用 `ProviderModel.maxTokens` 兜底）；**OpenAI 系维持不传**（openai_chat 现状零回归）。运行中切换经 18 号票 `HostRequestRefresher` 同源重算。
- **thinking 的 Anthropic 消费面**：18 号票 `ReasoningDirective.budget_tokens`（providers 纯函数层已定表）由本票消费——`backend::turn::reasoning_directive_to_options` 按指令自身分派（`budget_tokens` 有值 → rig `Reasoning::Budget`，其 anthropic options 层原生产出 `{"thinking":{"type":"enabled","budget_tokens":N}}` 并校验 budget≥1024、budget<max_tokens；`OnUnsupported::Ignore` 语义不变）；会话 off 档在 providers 层已折算为空指令，**不显式发 disabled**（§11.4「缺省行为」口径，与 rig `Reasoning::Off` 的显式 disabled 区分）。
- **连接测试三方言**：`probe::probe` 按 `ApiFormat` 分发（票面测试节「连接测试与实际对话走相同协议选择」）——openai_chat 与 openai_responses 共用 `GET {base}/models` + Bearer，回退段各自打 `/chat/completions`（max_tokens=1）或 `/responses`（max_output_tokens=16，Responses API 最小合法输出上限）；anthropic_messages 用 `x-api-key` + `anthropic-version` 鉴权（与对话请求同款头），`GET {base}/v1/models` → 回退 `POST {base}/v1/messages`，baseUrl 归一与 rig wire 同口径。七类错误分类与 30s 预算不变。前端卡片「测试连接」的方言禁用解除（ProvidersPage）。
- **能力与契约保持**：流式文本/reasoning/工具调用往返/取消/审批/图片降级/usage 四口径在帧面（ChatFrame）与存储契约零改动——协议差异全部收敛在 providers（wire 装配 + probe 端点）与 backend（reasoning→options/max_tokens 翻译）既有边界；跨方言切换后规范化历史与工具配对经 rig `AgentRun` 内部历史天然保持（e2e 实证：openai_chat 第 1 拍 → anthropic 第 2 拍，tool_result 回喂与 thinking 预算同拍生效）。usage 按 Q37/Q39/Q40 既有落账链不变（`current_model` 槽按拍归属）。
- **明确反馈面**：不支持能力不伪装——401 等供应商错误经 rig `PromptError` → runEnded(error) 带错误消息（e2e 实证）；§11.4「不支持者忽略」（`OnUnsupported::Ignore`）为设计定稿的静默缺省，不属伪装成功。
- **自定义请求头**：`ProviderConfig.headers` 三方言均不注入（现状 openai_chat 也未注入；票面边界「不新增自定义请求头」）。Key 池/OAuth 不涉及。

## 测试落点（全本地 fixture，无真实 Key / 公网）

- `src-tauri/src/providers/wire_test.rs`（5 例）：三方言变体构造与 describe 元数据、encode 端点路径分派（/chat/completions、/responses、/v1/messages）、Anthropic 地址尾缀归一四形态、解码器/重建器变体配对。
- `src-tauri/src/providers/probe_test.rs`（+4 例 = 13 例）：responses 方言回退段与最小补全体形状、anthropic 鉴权头断言（x-api-key + anthropic-version、无 Bearer）、anthropic baseUrl 五形态归一、分发入口按 ApiFormat 路由互斥。
- `src-tauri/tests/common/mock_anthropic.rs` / `mock_responses.rs`（新增 fixture）：Anthropic Messages 流协议（message_start/thinking_delta/text_delta/input_json_delta 分段半截 JSON/message_delta 计数）与 OpenAI Responses 事件流（reasoning summary delta/output_text delta/function_call_arguments delta/response.completed 计数）双轮脚本 + 非流式答复。
- `src-tauri/tests/protocol_dialects_e2e.rs`（6 例，新增集成测试）：anthropic 全链（thinking=medium 预算 76800 与 max_tokens 上调 109568 断言、tool_result 回喂 id 同源、usage 累计四口径）、无 thinking 的 max_tokens 兜底、401 明确反馈、openai_responses 全链（reasoning.effort=high、function_call_output 回喂、usage）、Anthropic 慢流中取消（runEnded(cancelled) 末帧、无 usage）、同 run 跨方言热切换（openai_chat → anthropic_messages，thinking/max_tokens/tool_result 跨方言保持）。
