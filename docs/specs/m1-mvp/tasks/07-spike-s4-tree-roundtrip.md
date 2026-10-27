# S4 消息树持久化 roundtrip（含 C-4 钉死）

Type: prototype
Status: resolved
Blocked by: 04

## Question

验证消息树持久化 roundtrip（技术设计文档附录 A **S4**，附录 C-4 钉死）：

1. **roundtrip**：assistant-ui 内执行编辑已发消息 / 重生成 / 分支切换 → `ExportedMessageRepository` 导出 → 按技术设计文档 §6.1 节点 schema 落盘（JSON）→ 重载还原完整树（含 activeLeaf 指针与分支结构）。
2. **C-4 钉死**：以实际安装版本（0.15.x）核对 `ExportedMessageRepository` 的精确结构与 API（字段名、parentId 语义、activeLeaf 表示），与设计文档假设比对，差异列清单。
3. **D12 评估**：顺带评估 rig-memory 的会话格式能否直接承载该树（省一层转换），给出可行/不可行结论。

可用 ScriptedBackend（脚本化假流）驱动，不依赖 rig 真实模型调用。

完成标准：roundtrip 通过；C-4 有结论；D-12 有建议。结论回写技术设计文档 §6/§7.2，勾销附录 A-S4 与附录 C-4。

## Answer

（2026-10-27 完成。分支 `spike/s4-tree-roundtrip`：`a00db50`，已并入 main。驱动方式 = headless LocalRuntime（`@assistant-ui/core` 直建 `LocalRuntimeCore`，不经 React 渲染）+ ScriptedBackend 假流，零真实模型依赖；`@assistant-ui/core@0.3.31` 已加为 ui devDep（headless 入口）。）

### 1. roundtrip：通过（vitest 11 例，`ui/src/chat/tree-persistence.test.ts`）

编辑已发消息（`composer.beginEdit()+send()`，语义=原消息父下 append 兄弟分支）、重生成（`reload()`）、分支切换（`switchToBranch({branchId})`）→ `thread.export()` 导出 → §6.1 schema 转换落盘 JSON → 读回 `thread.import()` 还原。断言覆盖：线性会话、编辑分支（两版本同 parentId、headId 指新叶、重载后可见路径=编辑后分支）、重生成兄弟、切回旧分支后 activeLeaf 保持、§6.1 形状符合性、reasoning part、tool-call↔toolCall+toolResult 拆分合并（含 argsJson/ok/summary/拓扑序）、挂起工具只落 toolCall、JSONL 行等价承载（生产分段 JSONL 预演）、空树。roundtrip 逐节点一致（节点集合/parentId/role/parts/status/createdAt/activePath 全等）。转换层 `ui/src/chat/tree-persistence.ts` 生产可演进；spike 期 JSON 文件落盘，生产为 append-only 分段 JSONL（已在 §7.2 注记全树载荷与追加序匹配）。

### 2. C-4 钉死（已定，回写附录 C-4 与 §6.3）

实际结构（`@assistant-ui/core` 0.3.31，专项测试钉死）：

```ts
type ExportedMessageRepository = {
  headId?: string | null;
  messages: Array<{ message: ThreadMessage; parentId: string | null; runConfig?: RunConfig }>;
};
```

字段级差异 vs 设计假设：①节点无平级 id 字段，id 内嵌 `message.id`；②`parentId` 必填非可省、`null`=根（存储条目映射 null→省略字段）；③activeLeaf = 顶层 `headId?: string|null`（名字/可缺省性不同，语义一致；import 缺省取末条，空树 null）；④`thread.export()` 返回**全树**（含全部分支），非仅活动路径；⑤message part 名为 `content`（text/reasoning/tool-call 内联 result/isError/source/file/image 等），`createdAt` 为 Date 非 epoch ms，assistant 带 `status`、user 带 attachments——命名/形态差异由转换层吸收；⑥工具结果内联于 tool-call part 的 `result`/`isError`（§3.2 的独立 toolResult WirePart 靠拆分/合并转换，已测）；⑦import 时 LocalRuntime 将 running/requires-action 的 assistant status 改写为 incomplete/cancelled（M1 只落 complete 不受影响；§6.1 已增 `status?`）；⑧`fromThreadMessageLike` 丢弃空白 text part、isError undefined→false（语义等价，比对口径已归一）；⑨树操作 API 钉死：编辑/重生成/切分支均为 MessageRuntime 公共 API（`getBranches` 仅内部态，公共面走 React hook 或导出结构自算）。

### 3. D12：不可行，维持现行决策（复刻 rig-session/4 原则）

以 rig v0.45.0 tag 源码查证（`crates/rig-memory`、`rig-core/src/conversation/`）：①`Conversation = Vec<Arc<Message>>` 线性 + `MessageId` 线性序号，**无 parentId，树结构不存在**；②编辑=Splice 区间重写、分支=`fork_log` 整份复制成新 log——「随时切回旧分支」需自建树索引重放原始行，等于绕开其格式；③`Entry.message: Arc<rig::Message>` 为封闭 serde 枚举，planSnapshot/runMarker/branchSummary 塞不进（`Aside` 是唯一通用挂载点但语义不合）；④严格单格式读取（`FORMAT="rig-session/4"`），自定义条目会让 rig 读不了；⑤`state` 行 KV「最新赢」与 meta 追加行可审计语义冲突。**可复刻纪律得到确认并可补强**：append+长度校验一步（并发写被拒，对应 §7.4）、崩溃半行 Repair（§7.3 口径）、**落盘前 JSON redact 脱敏（rig `redact::json_text` 同款，已增补进 §13-D12 复刻清单）**。

**文档回写**：§6.1（增 `status?`）、§6.3（导出结构/加载/树操作 API 钉死）、§7.2（全树载荷注记）、§13-D12（补强理由）、附录 A-S4 勾销、附录 C-4 翻「已定」。

**验证**：ui `pnpm install`（--frozen-lockfile 亦过）/ typecheck / test（16/16：runtime 5 + tree-persistence 11）/ 根 lint 全绿；本票零 Rust 改动。
