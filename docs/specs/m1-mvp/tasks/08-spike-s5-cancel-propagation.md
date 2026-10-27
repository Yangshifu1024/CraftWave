# S5 取消传播全链路

Type: prototype
Status: open
Blocked by: 05

## Question

验证取消传播全链（技术设计文档附录 A **S5**，基于「S2+S3 rig 流事件保真与手动工具循环」确定的 backend 形态）：

- 前端 `abortSignal` → `invoke("cancel_run")` → Rust 侧 `CancellationToken` → 模型流停止 + 进行中命令进程组收割（macOS 进程组 kill；Windows Job Object 方案至少纸面确认）→ `runEnded(cancelled)` 到前端，**全链 <1s**；
- 取消时收齐全部工具结果并补 `cancelled` 标记的语义验证（已写文件不回滚但明确标注）。

完成标准：全链时延实测 <1s；标记语义有结论。结论回写技术设计文档 §5.3，勾销附录 A-S5。
