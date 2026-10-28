# 20. 决策：C-8 临时会话 roots 边界安全复判

Type: grilling
Status: claimed

## Question

附录 C-8 上线前安全评审复判（HITL，主会话与用户做，子代理不得代答）：临时会话工作区=全局数据目录，其 roots 边界对 write/edit 是否过宽——数据目录下 config.json（明文 API Key，v2.5 决策 0600）、sessions/、knowledge/ 等都在 agent 可写面内。

输入材料：§8 围栏 P0 黑白名单现状、start_chat roots 按项目泛化的实现（16 号票）、数据目录实际内容面。

判断点：

1. 是否把数据目录从临时会话默认 roots 收窄（如收窄到专用 workspace 子目录）；
2. 围栏黑名单是否需补数据目录关键文件（config.json、intent 类内部文件）；
3. M1 维持 PRD 定义的代价与风险是否可接受。

结论回写：附录 C-8 状态改「已定」+ 正文对应章节修订 + 必要代码改动立票。
