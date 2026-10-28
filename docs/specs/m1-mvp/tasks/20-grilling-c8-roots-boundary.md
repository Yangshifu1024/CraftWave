# 20. 决策：C-8 临时会话 roots 边界安全复判

Type: grilling
Status: resolved

## Question

附录 C-8 上线前安全评审复判（HITL，主会话与用户做，子代理不得代答）：临时会话工作区=全局数据目录，其 roots 边界对 write/edit 是否过宽——数据目录下 config.json（明文 API Key，v2.5 决策 0600）、sessions/、knowledge/ 等都在 agent 可写面内。

输入材料：§8 围栏 P0 黑白名单现状、start_chat roots 按项目泛化的实现（16 号票）、数据目录实际内容面。

判断点：

1. 是否把数据目录从临时会话默认 roots 收窄（如收窄到专用 workspace 子目录）；
2. 围栏黑名单是否需补数据目录关键文件（config.json、intent 类内部文件）；
3. M1 维持 PRD 定义的代价与风险是否可接受。

结论回写：附录 C-8 状态改「已定」+ 正文对应章节修订 + 必要代码改动立票。

## Answer

grilling 两轮定案（2026-10-28，用户逐项确认）。

**事实基础（查实）**：① roots 双根对所有会话生效（§9.1）——项目会话绝对路径同样可达数据目录，非仅临时会话；② config.json 的 0600 防进程外不防进程内（agent=同进程），文件工具无路径 deny，read/grep/list 属 ReadOnly **全档静默**——Key 无感泄露的真通道；③ write/edit 走审批矩阵有门；④ command 属 Network 能力，confirm_each/auto_edit 逐条审批可见，仅 full_access 免审。

**决策**：

1. **Q1 = 收窄**：agent roots 从「项目主目录 + 数据目录」收窄为「项目主目录 + `<data_root>/workspace/` 子目录」。理由：「agent 永不触碰存储内部」是稳定不变量，deny 名单是把安全押在每次改存储都记得补名单上；数据目录零用户数据，迁移成本历史最低。PRD FR-16-01 同款「上线前复判」逻辑关闭——C-8 由此定案。
2. **Q2 = 接受 command 旁路为已声明风险**：`cat <data_root>/config.json` 类命令不受 safe_join 管（也不该受），M1 靠审批档兜底（confirm_each/auto_edit 逐条可见）+ full_access 用户自担；不做 command 路径级正则（误伤面大）。P1 的 FR-05-08（§9.3 受保护区检查）将来补写向纵深，读向始终靠审批档。
3. **Q3 = 临时会话两根同收窄**：两根同目录 = workspace 子目录（与既有「临时会话两根同目录」口径形状一致，无特判）。子目录命名 `workspace`（与「项目主目录」概念对仗）。
4. **Q4 = 落地三处回写 + 21 号实现票**：附录 C-8 已定、§9.1/§6.2/PRD FR-05-06 口径修订（v2.9）、GLOSSARY.md 首建；实现随 19 号 worktree 分支（[21 号票](21-impl-roots-narrowing.md)）。

不做：agent 面读会话历史等正当需求走产品 UI，不开 roots 例外。
