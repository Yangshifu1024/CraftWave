# S4 消息树持久化 roundtrip（含 C-4 钉死）

Type: prototype
Status: open
Blocked by: 04

## Question

验证消息树持久化 roundtrip（技术设计文档附录 A **S4**，附录 C-4 钉死）：

1. **roundtrip**：assistant-ui 内执行编辑已发消息 / 重生成 / 分支切换 → `ExportedMessageRepository` 导出 → 按技术设计文档 §6.1 节点 schema 落盘（JSON）→ 重载还原完整树（含 activeLeaf 指针与分支结构）。
2. **C-4 钉死**：以实际安装版本（0.15.x）核对 `ExportedMessageRepository` 的精确结构与 API（字段名、parentId 语义、activeLeaf 表示），与设计文档假设比对，差异列清单。
3. **D12 评估**：顺带评估 rig-memory 的会话格式能否直接承载该树（省一层转换），给出可行/不可行结论。

可用 ScriptedBackend（脚本化假流）驱动，不依赖 rig 真实模型调用。

完成标准：roundtrip 通过；C-4 有结论；D-12 有建议。结论回写技术设计文档 §6/§7.2，勾销附录 A-S4 与附录 C-4。
