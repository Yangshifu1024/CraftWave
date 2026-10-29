# 实现：内部状态保护与审批记忆（FR-05-08 + always_session/commandAllowlist）

Type: task
Status: open

## 要做什么

补上 C-8 复判留下的"写向纵深"：模型虽写不到数据目录（§9.1 roots 收窄后 safe_join 已挡 FileWrite 侧），但 command 的重定向/tee 可写任意路径——配置文件（含 API Key 明文）、mcp.json、索引、锁文件必须成为写目标禁区。同时补齐 M1 14 号票挂账的审批记忆：always_session（本会话免批）与 commandAllowlist（全局白名单）闭环 + 设置页"审批四开关 + 命令白名单"组。

FR 依据（原文摘引）：
- FR-05-08：配置/凭据引用/MCP 配置/索引/锁文件不可被模型文件写或命令重定向篡改（`E_INTERNAL_STATE`）
- 设计文档变更记录 v2.9（C-8）：command 旁路（读 Key 类）记为已声明风险（审批档兜底 + full_access 自担），**写向纵深归 P1 FR-05-08**（即本票）
- FR-14-04：……审批四开关 + 命令白名单……
- §3.2 AskPayload approval options mode 已定 `allow_once | always_session | deny`（always_session 键面 M1 未实装——14 号票 Answer："审批选项只给 allow_once/deny 两项，always_session 命令记忆归 M2"）

## 具体做什么

### 后端（src-tauri/）

1. 内部状态保护（gate.rs 新判定层，插在围栏步骤之后、矩阵之前）：
   - 禁区清单（数据侧）：`~/.craftwave/` 全目录（config.json 含 apiKey 明文、projects/、sessions/、histories/、ui-state.json、stats/、quota/、trust/、logs/、cache/）+ 项目侧 `<home>/.craftwave/`（project.json、项目 mcp.json）+ 全局 mcp.json + 段元数据 `.segmeta.json`——即 FR 行"配置/凭据引用/MCP 配置/索引/锁文件"的文件级展开；清单常量集中一处供测试枚举；
   - 判定输入 = **22 号票接缝**：`safety/fence/ast.rs` 的写目标提取产物（词法目标 + canonical）；22 未落地时本票先以词法重定向目标提取兜底，22 落地后收敛（与 24/08 号票同风格）；
   - 判定规则：写目标（词法或 canonical，含 symlink 指入禁区）命中禁区 → 工具结果回喂 `E_INTERNAL_STATE`（§3.6"试图写宿主内部状态"）；不挂起、任何档位与白名单不豁免；
   - 读不禁：C-8 已声明风险（读 Key 类），不越权扩到读路径。
2. 审批记忆——always_session：
   - 命令类审批选项增"始终允许（本会话）"（§3.2 mode 键面已有）；选中 → 会话级签名集登记（(name, argsJson) 口径同 M1 `backend/gate.rs` approved 集）；
   - **升域**：签名集从 per-run ApprovalGate 提升到会话级（RunManager/SessionState 持有），跨 run 生效、会话关闭即清。
3. 审批记忆——commandAllowlist 消费（字段 M1 就位、无判定面）：
   - 命中语义 = 命令首词+子命令**词序列前缀匹配**（条目 `git status` 匹配 `git status …`，不匹配 `git statusx`）；
   - 命中 → 围栏 Confirm 与矩阵 NeedsApproval 免除；黑名单 Block 永不豁免（§8.5 灾难级铁律）；
   - 字段级语义设计文档缺失，此为本票定义，**回写 §3.2 approval 注记**；
   - 白名单写入入口：审批弹窗勾选"加入白名单" → save_config 持久；SecurityPage 可增删。
4. **审批四开关**（FR-14-04 行文，字段级缺失——本票拟定，实现前回写 §3.2/FR-14-04 对齐）：
   - ① 审批总开关 enabled（已有）② autoConfirm（已有）③ 会话内记忆启用（always_session 选项是否展示，默认开）④ 白名单启用（整体开关，默认开）。
5. 实现顺序建议：禁区清单常量 + 判定层（词法兜底）先行（篡改向量测试可先绿）→ always_session 升域 → 白名单匹配与四开关 → 22 号票落地后把判定输入切换到 AST 提取并回归。
6. 命令面：无新命令（记忆走既有 resolve_ask + save_config）。

### 前端（ui/）

1. AskPanel 审批形态：命令类审批增"始终允许（本会话）"选项与"加入白名单"勾选（载荷 mode=always_session / 白名单走 save_config）。
2. SecurityPage：审批四开关组 + 命令白名单可管理（手填命令前缀增删 + 词边界校验提示；现仅能删，补增）。

### 测试

- Rust：
  - **篡改向量测试集**——写 `~/.craftwave/config.json`、重定向覆盖 mcp.json、改 `.segmeta.json`/锁文件，全部 E_INTERNAL_STATE；
  - 读命令不受影响；symlink 指入禁区也拦；22 未接时词法兜底路径同套断言；
  - 审批记忆：always_session 后同命令跨 run 免批、新会话不延续；白名单命中免 Confirm、黑名单仍 Block；四开关逐项生效（③关闭不出选项、④关闭白名单不豁免）。
- 前端：AskPanel 新选项载荷、SecurityPage 四开关与白名单管理、契约测试（无新命令，断言 approval options 键面与 ApprovalConfig 字段）。

## 边界（不在本票）

- E_DELETE_GUARDED 删除守卫与 .git 保护（§9.1 既有）；读 Key 类旁路（已声明风险，不扩权）；MCP 工具级 always_allow（§9.7，08 号票）；计划门/范围门（25 号票）；AST 提取本体（22 号票）。

## 验收

- [ ] 篡改向量（写 config.json、重定向覆盖 mcp.json、改锁文件/segmeta）全部被拒并回喂 E_INTERNAL_STATE
- [ ] 同一命令经"本会话始终允许"后跨 run 免批；新会话恢复审批
- [ ] 白名单命令免 Confirm 全局生效且 UI 可增删；黑名单命令任何配置下仍 Block
- [ ] 审批四开关逐项生效，字段与 §3.2/FR-14-04 回写一致
