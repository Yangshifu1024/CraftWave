# 实现：监督看门狗与模型停滞检测

Type: task
Status: resolved

## 要做什么

落实 FR-03-07 与技术设计 §5.7，及时纠偏重复失败和空转，终止失控或停滞的 run；补齐原 36 票遗漏的 P1 本体。Q33 已确认计时边界，34 号收口前完成。

## 具体做什么

- 在既有 RunState/TurnDriver 接缝维护容量 32 的工具调用结果窗口，签名为工具名＋参数哈希，turn 边界评估，沿用 §5.7 阈值：同签名连续失败 3 次纠偏、5 次终止；无文本且无工具成功的空转 8/14 步纠偏/终止；同参数只读调用成功重复 4 次纠偏；同文件连续分段读 10 次纠偏。
- 纠偏注入 hidden system 提示，不写入用户可见消息树；终止走既有 Error/E_RUN_STALLED 收尾，保留已输出内容及工具结果，不重复执行副作用；06 号待发队列按失败暂停。
- 模型停滞仅在请求阶段计时：从请求发出开始，收到有效模型增量时刷新。等待人工审批、工具执行、子代理排队或执行不计入；前端帧、工具进度与重连不能刷新计时。
- 默认 stall_timeout=300 秒，可配置 5–3600 秒；保存边界校验，旧配置回填默认值。配置字段、设置页接线、有效模型增量判定与计时/恢复状态先回写 §3.2/§5.7，再实现。
- 复用取消与收尾链路，停滞终止取消在飞请求、收齐工具后仅发一次 runEnded；错误消息说明停滞或重复失败原因。工具及子代理继续遵循各自超时、预算和级联取消规则。

## 测试

- 使用独立 _test.rs 文件与可控时钟，验证全部阈值上下界、纠偏与终止次数、重置条件；不依赖真实 300 秒等待。
- 请求发出后一直无增量触发停滞；有效增量刷新；工具执行 600 秒、审批等待和子代理等待不误触发；UI 帧/重连不能续命。
- ScriptedBackend 验证 E_RUN_STALLED 保留输出、不重复工具副作用、终态唯一、待发队列暂停；配置默认值与 5–3600 边界回归。

## 边界

- 不新增独立任务管理框架，不代替工具自身超时；不改变已确认的审批、队列或子代理治理规则。

## 验收

- [x] §5.7 全部纠偏/终止阈值可复现，有独立测试。
- [x] 模型请求阶段计时与有效增量刷新正确，工具/审批/子代理等待不误杀。
- [x] 超时配置可改可持久化，旧配置兼容，非法值拒绝。
- [x] E_RUN_STALLED 收尾唯一、结果保留、队列暂停；34 号核查引用本票测试。
- [x] §3.2/§5.7 配置与状态接缝已补齐。

## 决议记录（2026-10-30 实现定稿）

- **字段级先回写**：§3.2 AppConfig 增 `watchdog` 组（`stallTimeoutSecs` 缺省 300 / 合法 5–3600 / serde default 回填 / 按 run 装配生效），§5.7 补四条字段级接缝（有效模型增量判定、计时与恢复状态、终止收尾、配置生效面），变更记录 v2.16。
- **有效模型增量判定**：流事件桥接产出的三类增量帧（textDelta / reasoningDelta / toolArgs）到达即刷新；Start/End 生命周期事件、未建模透传负载与帧面其他帧（stepStarted/toolStarted/toolResult/usage/compactDone）一律不刷新。刷新点在流循环出帧回调内、先于 64ms 节流器，不经帧枢纽——UI 帧、attach_run 重连、工具进度结构性不能续命。
- **计时器随单次模型请求生灭**：`drive_run` 每拍 CallModel 以 `tokio::select!` 让流式调用与停滞截止竞争（biased，停滞分支优先 poll）；停滞胜出即丢弃流 future（在飞 HTTP 请求随之关闭，与取消同形）。审批挂起时 `drive_run` 已退出（计时器销毁），恢复重新驱动重新计时——审批等待天然不计入；工具执行在 `execute_batch` await（无竞争），天然不计入；子代理（10 号票未落地）沿用同一结构性豁免。停滞超时值可注入，测试以毫秒级小值验证、不依赖真实 300 秒。
- **工具窗口与看门狗状态挂 `DriveState`**：跨审批挂起延续（恢复不丢历史；挂起时批内零执行，恢复重放不重复计数）。
- **纠偏水位语义**：同签名失败段（锚 = 段起点前一条记录的签名，锚变化即段重开复位）3 次纠偏一次、继续失败不再重复纠偏、5 次终止；无进展 8 步纠偏一次、有进展步清零复位、14 步终止；只读成功重复与分段读按级（每满 4/10 次）纠偏，防纠偏风暴。四类信号评估顺序 = 失败 → 空转 → 只读重复 → 分段读，终止优先。
- **"文本"判定**：仅 textDelta（正文）算有文本进展；reasoningDelta 刷新停滞计时但不算无进展信号的文本进展——向用户可见的输出才算进展，纯思考空转应被抓住。
- **"提示改用 batch_read"落地口径**：注册表当前无 batch_read 工具（§8.3 有行、实现随 M2 工具包票），分段读纠偏提示改为"一次性读取更大窗口（read 的 offset/limit）或改用 grep 定位"；batch_read 落地后可再回改文案。
- **终止收尾**：`RunExit::Stalled`（新退出态）——`runEnded(error, { code: E_RUN_STALLED })` 恰好一条、已发出的增量与 toolResult 帧保留不撤回、触发批已收齐工具不重放；`run:error` 事件 code 从 `RunFailure` 结构带出（看门狗终止 = E_RUN_STALLED，其余失败维持 E_INTERNAL 兜底）；followUp 队列收尾动作抽纯函数 `followup_disposition`（钉住 Stalled → 失败暂停）。
- **TurnDriver 泛型化**：`Model<Chat>` 具体类型改为 `Model<W: Wire<Op=Completion>, T: Transport<W>>`——流事件面（`Item<StreamEvent>`）与响应面对全部 completion wire 同形，方法体零改动；测试侧"ScriptedBackend" = rig test_utils 剧本假模型（dev-dependencies 启用 `rig/test-utils`，feature unification 保证 cargo build 生产面不变）+ 自定义悬空/慢速 transport 覆盖停滞与持续刷新场景；压缩编排 5 处签名随之泛型化（生产调用点类型推断零改动）。
- **设置页**：工作区与智能体页增"运行监督"区（数字输入 5–3600，走草稿 + saveAll）；agent 页脏判定切片扩为 `{ agent, watchdog }`。

## 实测检查链（2026-10-30，MACOSX_DEPLOYMENT_TARGET=13.0）

cargo fmt --all -- --check ✅ / cargo build ✅ / cargo clippy --all-targets ✅ / cargo test 375 passed ✅ / pnpm typecheck ✅ / pnpm test 193 passed ✅ / pnpm lint ✅
