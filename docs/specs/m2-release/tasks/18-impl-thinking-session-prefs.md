# 实现：Thinking 档位与会话偏好（FR-02-08 + FR-14-05/07 + Ctrl+L）

Type: task
Status: open

## 要做什么

用户能按会话调整模型思考力度（off/minimal/low/medium/high），并让"这个会话用哪个模型、什么思考档"跨重启还原——长会话不必每次重新选模型。切片：① 会话偏好持久化（模型 + thinking；权限档明确不持久化）；② Composer thinking 快捷钮；③ Ctrl+L 模型快速切换（FR-13-05 P1 半）；④ reasoning 参数真正注入请求——`ProviderModel.reasoning_effort` 字段 M1 已就位但全仓无请求侧消费（grep 证实仅 types.rs 定义），本票接线。

FR 依据（原文摘引）：
- FR-02-08：Thinking 档位 off/minimal/low/medium/high 切换（Shift+Tab 或 Composer 快捷钮）；切换写入会话记录可还原
- FR-14-05：`<id>.prefs.json`：模型/thinking；权限档**不持久化**（重启回保守档）
- FR-02-07：全局 active model + 会话级 override（`<id>.prefs.json`）；运行中切换下一请求生效
- FR-13-05：Ctrl+L 模型切换（P1）
- §11.4：reasoningEffort default/low/medium/high/max → Anthropic thinking budget 占 contextWindow 20/40/60/80/80%（max 叠加 max_tokens 上调）；OpenAI 系直接映射 reasoning_effort 字段；不支持者忽略并在 uiSummary 标注

## 具体做什么

### 后端（src-tauri/）

1. 会话偏好存储：
   - 路径 `sessions/<sessionId>.prefs.json`（§7.1 既有；删除随会话清除已在 `storage/session_tree.rs:381`）；
   - 新增 `storage/prefs.rs`：`SessionPrefs { model_id: Option<String>, thinking: Option<ThinkingLevel> }` 读写，经 `util/atomic.rs` 原子写；
   - 损坏/缺失 → 空偏好兜底，不阻塞会话加载。
2. 命令实装 §3.3 #24/#25（**当前代码未实现**，全仓无 `get_session_prefs`/`set_session_prefs`，落 `host/commands/session.rs`）：
   - `get_session_prefs { sessionId } → { modelId?, thinking? }`；会话不存在 → E_NOT_FOUND；
   - `set_session_prefs { sessionId, prefs } → void`；merge 语义（只传的字段被改）；
   - **回写点**：§3.3 两行参数面只有 modelId，本票增 thinking 字段，实现前最小修订 §3.3 #24/#25。
3. RunConfig 扩展：`thinking?: ThinkingLevel`（serde default 向后兼容；§3.2 RunConfig 增字段回写；schemaVersion 维持 2，设计文档未定义 v3）。
4. reasoning 注入实装：
   - 现状：`resolve_model`（`host/commands/chat.rs:126`）仅构造 `OpenAIConfig::new(key).with_base_url(..).client().chat(model)`，未传任何请求参数；
   - 注入点 = 请求装配的 rig options 面：rig-core 0.45 openai options 已内建 `Reasoning::Off / Reasoning::Effort → 请求字段 reasoning_effort` 映射（`providers/openai/options.rs`，`Effort::as_str`）；
   - "下一请求生效"：客户端按 run 构建（§11.1），start_chat 每次读 prefs 快照，无额外语义。
5. 档位映射（纯函数 `thinking_to_request(level, api_format, context_window, max_tokens)`，落 `providers/reasoning.rs`）：
   - 优先级：会话 thinking 显式设置 > `ProviderModel.reasoning_effort` > 不传；
   - OpenAI 系：off → 不传（Reasoning::Off 语义）；minimal/low/medium/high → reasoning_effort 同名值；
   - Anthropic 口径（§11.4 比例轴）：minimal→20%、low→40%、medium→60%、high→80%（budget_tokens = contextWindow×比例；max_tokens 上调随 §11.4 max 行类推）；
   - **纪律回写点**：FR-02-08 五档与 §11.4 五档（default/low/medium/high/max）不对齐——实现前把上表回写 §11.4 定稿，再按表实现。

### 前端（ui/）

1. Composer thinking 快捷钮（FR-02-08 给"Shift+Tab 或按钮"二选一——Shift+Tab 已被权限档占用（§10.6），走按钮）：
   - Composer 工具条加档位钮（off→minimal→low→medium→high 循环或下拉）；
   - 新增 per-session 内存态 store（仿 `chat/permission-tier.ts` 的 bySession 模式）；
   - 变更时 `set_session_prefs` 落盘（防抖）。
2. Ctrl+L 模型切换（FR-13-05）：唤起模型选择下拉；选择即 `set_session_prefs` 持久。
   - 现状：M1 `chat/session-model.ts` 已有每会话模型覆盖但纯内存，文件头自述"持久化随 session_prefs 域后续票"——即本票。
3. 恢复链路：进入会话 `get_session_prefs` → 回填模型 store 与 thinking store；运行中切换下一请求生效（runConfig 惰性取值已在 `runtime.ts:222` 就位）。
4. 契约：`ipc/client.ts` COMMANDS 增 #24/#25、`ipc/types.ts` 镜像、contract.test 接线断言；§10.6 键位表增 Ctrl+L 行（回写）。

- **请求预算接缝（Q18 用户确认）**：附图、切模型及下一次模型请求均走 04 号票的实际请求预算检查并预留回复空间；没有历史 usage 也不能跳过，压缩后仍超限则暂停保留输入。

- **非视觉切换（Q30 用户确认）**：含历史图片的会话切换到非 vision 后允许纯文字继续，历史图片发给模型时占位并显示提示，界面原图保留；新附图和主动读图仍拒绝。与 17 号票统一校验。

### 测试

- Rust：prefs roundtrip / 损坏文件兜底 / 会话不存在 E_NOT_FOUND；`thinking_to_request` 表驱动（五档 × 两方言 × 窗口参数）；RunConfig 旧载荷（无 thinking）反序列化兼容；映射函数对 §11.4 回写表的锚定断言。
- 前端：thinking store 循环与恢复、Ctrl+L 打开选择、runtime 惰性 runConfig 携带 thinking、契约测试 #24/#25。

## 边界（不在本票）

- anthropic_messages / openai_responses 方言的请求接入（resolve_model 现仅 openai_chat，§11.1 相关票）；本票交付 Anthropic 预算映射纯函数 + 单测，供方言票直接消费。
- 多 Key 池（FR-02-12 已砍单，Out of scope）；权限档持久化（明确不做，FR-14-05）；reasoning 内容渲染（reasoningDelta 帧已有）；模型参数编辑 UI（maxTokens/contextWindow 表单，供应商编辑器既有范畴）。

## 验收

- [ ] Composer thinking 钮五档可切，下一请求生效（mock 端可断言 reasoning_effort 字段随档变化）
- [ ] 选模型 + 切档后重启应用，进入会话两者还原；权限档重启回默认档不还原
- [ ] Ctrl+L 唤起模型切换，选择即持久
- [ ] 会话未显式设置 thinking 时，per-model reasoningEffort 生效（含不传档）
- [ ] 契约测试含 #24/#25 接线；§3.3 #24/#25、§3.2 RunConfig、§11.4 映射表、§10.6 键位表回写完成
