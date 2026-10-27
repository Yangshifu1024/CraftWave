# 决策：未决项 C-9/C-10 默认方案确认

Type: grilling
Status: resolved

## Question

对技术设计文档附录 C 两项「待定」问题逐项确认或推翻建议默认方案（轻量 grilling，一次会话完成）：

1. **C-9**——suggest（跟进建议）与会话标题生成的「零工具小调用」是否计入用量：建议**计入会话累计、不计入轮次（turnUsage），uiSummary 标注「后台生成」**。
2. **C-10**——Windows 下 `read_workspace_file`/预览的大文件阈值与编码处理：建议 **M1 仅 UTF-8（检测失败即报错提示），GBK 支持缓到 P1**。

每项给结论（确认/调整/推翻+新方案），并回写技术设计文档附录 C 后从清单移除。若 grilling 中牵出新的未决问题，按 tracker 约定回填本图。

## Answer

（2026-10-27 grilling 定案，三轮问完前沿，用户逐项确认；C-10 推翻原建议默认。已回写技术设计文档 §3.2 命令表 #28、§8.3 工具表（read/write/edit/suggest 行）、§10.5 用量口径、变更记录 v2.2，附录 C 两行翻「已定」。沿用既有实践保留行并标已定，未做物理移除。）

### C-9 后台零工具小调用用量口径 —— 确认原建议

- **计入会话累计**：`tokens:update` 照发、`SessionMeta.totalTokens` 含之、重启回填后数字一致。
- **不计入 turnUsage**：轮次数字在 runEnded 后冻结，不因后台调用完成而跳动。
- **用量 Popover 不单列「后台生成」明细行**（M1）：差额每轮数十 token，标注保留在内部 Meta 工具结果与日志层面，前端不做特判。
- **失败静默放弃**：suggest/标题生成调用失败（模型报错、超时）→ 提示不出现、不重试、不计用量、无任何 UI 提示。

grilling 中向用户澄清了 suggest 的语义（每轮 runEnded 后的后台小调用，Composer 内 ghost span 提示"下一步可问什么"，非输入期预测），用户据此确认失败静默方案。

### C-10 Windows 编码与大文件 —— 推翻「仅 UTF-8」默认

用户裁定：**M1 必须支持其他编码自动判断，否则 Windows 用户开箱即遇不可用**。定案：

- **编码自动判断**（encoding_rs + chardetng，实现票落地时核版本）：① BOM（UTF-8/UTF-16LE/BE）直接定；② 无 BOM 先严格 UTF-8 校验；③ 不中则 chardetng 探测（GBK/Big5/Shift_JIS 等）按置信度取优；④ 仍定不了按最佳猜测渲染（预览宁乱码不报错）。GBK 不再缓 P1。
- **解码器为共用模块**：agent 的 read 工具与 read_workspace_file 同走一套——否则模型读 GBK 文件仍失败，"Windows 用户用不了"只解决一半。
- **落盘一律 UTF-8**：write/edit 新建与改写均 UTF-8，不做原编码写回（编辑原 GBK 文件后转 UTF-8，M1 接受此副作用）。
- **大文件分页读取**：单次响应 ≤512KB；无 range 读文件开头 512KB + `truncated=true`；range 按行翻页；不设文件级拒读硬限。M1 预览面 = read 工具卡「展开全文」（读全文走本命令，§10.3）；M2+ Inspector 产物页/PDF/xlsx 预览走同一命令自动继承。
- **二进制检测**：前 8KB 含 NUL 字节判二进制 → 报错「二进制文件不支持文本预览」（图片/Office 预览各有专门通道）。

无新牵出的未决问题，地图无需增票。
