# S1 assistant-ui 镜像工具桥接

Type: prototype
Status: open
Blocked by: 04

## Question

验证 assistant-ui 镜像工具桥接方案可行性（技术设计文档 §10.3，附录 A **S1**）：

- 前端 `defineToolkit` 声明**镜像工具**（后端工具的渲染副本，不在前端执行）；
- Rust 侧 Channel 帧（ToolStart / ToolProgress / ToolResult）驱动镜像工具的 execute resolve；
- 工具卡片状态机（参数准备中 → 运行中 → 完成/错误）正常流转，不闪烁不重建；
- 半截 JSON 参数增量以「参数准备中」态展示。

**判定标准**：桥接可行、状态机正常。**否则**退 D11 路线（自定义 data part 驱动工具卡片），说明差异与代价。

完成标准：二选一有明确结论与依据。结论回写技术设计文档 §10.3，勾销附录 A-S1。
