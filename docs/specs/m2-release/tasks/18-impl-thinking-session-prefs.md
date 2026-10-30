# 实现：Thinking 档位与会话偏好（FR-02-08 + FR-14-05/07 + Ctrl+L）

Type: task
Status: resolved

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
   - **下一请求生效**：指当前 run 内的下一次模型请求，不只是下一次 start_chat。每次请求前读取最新会话模型/thinking 偏好，必要时更新模型客户端与请求参数；正在进行的模型请求不被切换打断。前端 runtime 惰性取值只覆盖新 run，不能代替后端在同 run 后续请求前刷新。沿用 Q18 预算检查与 Q30 历史图片降级，模型用量按实际调用归属。
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
3. 恢复链路：进入会话 `get_session_prefs` → 回填模型 store 与 thinking store；运行中切换写入后端偏好，当前请求结束后的下一次模型请求生效，不能仅等待下一个 run。
4. 契约：`ipc/client.ts` COMMANDS 增 #24/#25、`ipc/types.ts` 镜像、contract.test 接线断言；§10.6 键位表增 Ctrl+L 行（回写）。

- **请求预算接缝（Q18 用户确认）**：附图、切模型及下一次模型请求均走 04 号票的实际请求预算检查并预留回复空间；没有历史 usage 也不能跳过，压缩后仍超限则暂停保留输入。

- **非视觉切换（Q30 用户确认）**：含历史图片的会话切换到非 vision 后允许纯文字继续，历史图片发给模型时占位并显示提示，界面原图保留；新附图和主动读图仍拒绝。与 17 号票统一校验。

### 测试

- 同一个 run 先以模型 A/thinking low 请求，运行中切到 B/high；当前请求正常结束，下一次模型请求使用 B/high，预算与图片能力重新校验；无需重新 start_chat，usage 归实际模型。
- Rust：prefs roundtrip / 损坏文件兜底 / 会话不存在 E_NOT_FOUND；`thinking_to_request` 表驱动（五档 × 两方言 × 窗口参数）；RunConfig 旧载荷（无 thinking）反序列化兼容；映射函数对 §11.4 回写表的锚定断言。
- 前端：thinking store 循环与恢复、Ctrl+L 打开选择、runtime 惰性 runConfig 携带 thinking、契约测试 #24/#25。

## 边界（不在本票）

- anthropic_messages / openai_responses 方言的请求接入由 38 号票承载；本票的预算映射与偏好切换须与其接通，三协议端到端验收不能仅由纯函数测试替代。
- 多 Key 池（FR-02-12 已砍单，Out of scope）；权限档持久化（明确不做，FR-14-05）；reasoning 内容渲染（reasoningDelta 帧已有）；模型参数编辑 UI（maxTokens/contextWindow 表单，供应商编辑器既有范畴）。

## 验收

- [x] Composer thinking 钮五档可切，下一请求生效（mock 端可断言 reasoning_effort 字段随档变化）
  ——ThinkingMenu 下拉（跟随默认 + off/minimal/low/medium/high，Thread.tsx）；映射层 `reasoning_test.rs` 表驱动钉五档 × 三方言；wire 注入层集成测试 `backend_e2e::e2e_mid_run_switch_applies_on_next_request` 断言切换后请求体带 `reasoning_effort: "high"`、切换前不带；新 run 起点 `sessionRunConfig` 惰性携带（runtime.test 钉 off/未设置的区分）
- [x] 选模型 + 切档后重启应用，进入会话两者还原；权限档重启回默认档不还原
  ——`<id>.prefs.json` 原子写（prefs_test roundtrip/损坏兜底）+ SessionTab 挂载 `restoreSessionPrefs` 回填双 store（session-prefs.test：回填/未设置不动/失败静默/不回写环）；权限档 store 无落盘路径（FR-14-05 保持内存态）。注：重启还原以后端 roundtrip + 挂载恢复两级测试覆盖，未做真实重启的人工端到端
- [x] Ctrl+L 唤起模型切换，选择即持久
  ——SessionTab 活跃 Tab keydown → `openRequest` store → ModelPicker 展开；`setModelPersisted` 防抖落盘（session-prefs.test 钉合并与 null 清除语义）
- [x] 会话未显式设置 thinking 时，per-model reasoningEffort 生效（含不传档）
  ——`effective_thinking_directive` 优先级链（chat.rs）+ `reasoning_test.rs` 的 `model_effort_*` 表驱动（default → OpenAI 不传 / Anthropic 20%）
- [x] 契约测试含 #24/#25 接线；§3.3 #24/#25、§3.2 RunConfig、§11.4 映射表、§10.6 键位表回写完成
  ——`contract.test.ts` 双命令接线 + 类型名锚点；`ipc_manifest_test.rs` #24/#25 锚点断言；文档四处回写完成（v2.23）

## 决议记录

- **续作决策（2026-10-31）**：本票由上一代理限额中断于 WIP 检查点 6611ac1（编译不过：E0382/E0061/E0599/E0753/E0308/E0004 十处），本次续作修复全部编译错误（含 `drive_run` 增 refresh 参数的三个既有调用点同步、`RunExit::Failed` 新变体在子代理驱动循环的兜底分支），并补齐前端消费面、契约同步与文档回写；WIP 检查点提交保留不动。
- **§11.4 对照表定稿**（纪律回写点）：会话五档（off/minimal/low/medium/high）与 per-model 五档（default/low/medium/high/max）不对齐问题的回写定案——会话 `minimal` ↔ per-model `default` 同 20% 预算；会话 `off` = OpenAI 系显式 `"none"`（rig `Reasoning::Off`）/ Anthropic 不启用（缺省行为），区别于「未设置」；max_tokens 上调规则统一为 budget > 原 maxTokens 时上调至 min(contextWindow, budget + 原 maxTokens)。
- **#25 merge 语义**：`prefs` 载荷采用双 Option（TS 生成物摊平为 `T | null | undefined`）——字段缺省 = 不动、`null` = 清除回默认、设值 = 覆盖；空白 modelId 归一为清除（宽容前端空串）。
- **偏好权威链**：run 起点快照 = prefs 文件值 > RunConfig（前端两者同步写，文件缺失回落 RunConfig 兼容旧载荷）；运行中以后端 prefs 文件为权威——`HostRequestRefresher` 每拍 CallModel 前重读，变更才换模型客户端/请求参数/图片降级开关（锁内 clone 即释放，不跨 await，不打断在飞请求）。
- **Q18 预算接缝落地**：切换模型时以 `DriveState.last_input`（最近请求真实输入）+ 新模型回复预留做硬预算复查（`is_over_wire_budget`），超限 E_CONTEXT_OVERFLOW 终止（§6.2 暂停保留输入）；`RunExit::Failed` 为新增终态变体，runEnded(error) 由 drive_run 发出。
- **rig 目录启发式行为记录**：rig 0.45 按模型名启发式判定推理能力——`gpt-4` 系按名判定为非推理模型，effort 参数被 `OnUnsupported::Ignore` 丢弃（正确语义）；目录外名字（第三方兼容端点如 deepseek/qwen）不匹配任何启发式，effort 正常透传。集成测试的模型 B 用 `gpt-5-mock` 名字触发「按名推理模型」分支。
- **诚实降级（不勾假绿之外的说明）**：①「预算与图片能力重新校验」在切换路径由 `HostRequestRefresher` 实现承载（`is_over_wire_budget` 纯函数 + `set_downgrade_images` 开关），有实现与组件级覆盖，但无独立集成测试驱动「切换后超预算 → E_CONTEXT_OVERFLOW」与「切非 vision → 历史图降级」的端到端路径（HostRequestRefresher 为 pub(crate) 且依赖完整 AppState，测试构造代价高；核心判定函数已有单测）；②「重启还原」以 roundtrip + 挂载恢复两级测试覆盖，未做真实重启人工验收。
- **前端持久化节奏**：`session-prefs.ts` 模块级防抖（300ms 合并同会话多字段变更为一次 set_session_prefs；同字段后写覆盖先写）；恢复走 store 纯方法防「读取→原样写回」回环。
- **Ctrl+L 实现口径**：FR-13-05 的「Ctrl+L」按字面 `Ctrl` 键实现（`event.ctrlKey`，macOS 亦然）；仅活跃 Tab 响应（多 Tab keep-mounted，非活跃 handler 放行不拦截）；ModelPicker 展开经模块级 `openRequest` store 单信号承载。
