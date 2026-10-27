# 决策：未决项 C-9/C-10 默认方案确认

Type: grilling
Status: open

## Question

对技术设计文档附录 C 两项「待定」问题逐项确认或推翻建议默认方案（轻量 grilling，一次会话完成）：

1. **C-9**——suggest（跟进建议）与会话标题生成的「零工具小调用」是否计入用量：建议**计入会话累计、不计入轮次（turnUsage），uiSummary 标注「后台生成」**。
2. **C-10**——Windows 下 `read_workspace_file`/预览的大文件阈值与编码处理：建议 **M1 仅 UTF-8（检测失败即报错提示），GBK 支持缓到 P1**。

每项给结论（确认/调整/推翻+新方案），并回写技术设计文档附录 C 后从清单移除。若 grilling 中牵出新的未决问题，按 tracker 约定回填本图。
