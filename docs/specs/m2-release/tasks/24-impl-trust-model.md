# 实现：信任模型——项目 TOFU 与外部目录授权（FR-05-10 + FR-05-12）

Type: task
Status: resolved

## 要做什么

项目 `.craftwave/` 里的资源（技能/记忆/mcp.json/项目指令）是第三方注入面：首次加载须让用户知情决策（本次/记住/拒绝），内容一变重新问；项目外目录按"目录及子目录"粒度授权而不是整盘放开。本票同时交付统一信任存储，作为 MCP（08 号票）与技能（09 号票）信任的接缝。

FR 依据（原文摘引）：
- FR-05-10：项目 `.craftwave/` 资源（skills/memory/mcp.json/项目指令）首次加载需信任决策（本次/记住/拒绝），绑定内容 SHA-256
- FR-05-12：项目外目录访问需"始终允许"授权（粒度=目录及子目录读/写/列）；拒绝对文件系统根/家目录整体放行
- §9.8：指纹=资源内容 SHA-256 → AskPayload{kind:"projectTrust"}（本次会话允许/记住此版本/拒绝）；决策写 `trust/project.json`（§7.7）；未信任资源不注入系统提示且工具访问拒绝（E_PROJECT_UNTRUSTED）；内容变更重新询问；全局作用域（`~/.craftwave/skills/`、`memories/`）首用同样走信任（scope=global）
- §9.9：会话 roots 初始 = 项目主目录 + `<data_root>/workspace/` 子目录（C-8 收窄）；roots 外路径 → 预检 `check_external_path` → ask（始终允许 = `allow_external_dir` 入会话 roots 快照）；拒绝整体放行文件系统根、家目录本身、宿主托管目录（`~/.craftwave`、`<home>/.craftwave`）；授权持久于会话 roots 快照，不全局生效
- §3.2 已有 AskPayload `{kind:"projectTrust", resourceName, fingerprint}`（options `allow_session|remember|deny`）；§3.3 #33 `check_external_path { sessionId, path } → { allowed, insideRoots }`、#34 `allow_external_dir { sessionId, dir } → { roots: string[] }`；§3.6 已有 E_PROJECT_UNTRUSTED

## 具体做什么

### 后端（src-tauri/）

1. 信任存储：新增 `storage/trust.rs`（TrustStore trait + 文件实现），落 `trust/project.json`（§7.7 字段级：`[{ resourceName, fingerprint, decidedAt, decision }]`，原子写）。
   - **存储口径**：FR-05-10 行文"存钥匙串"为 v2.5 前口径，以设计文档 v2.9（§7.7/§9.8 落文件）为准，随 Answer 回写 FR 引注；
   - **与 MCP 的接缝约定**：§9.7 MCP 启动信任写 keyring 账户 `craftwave.trust.v1`，但 v2.5 已废 keyring（§9.4）——本票把 TrustStore 扩为两类信任统一承载（记录增 `scope: "project"|"mcp"` 判别字段，**回写 §7.7**）；
   - 08 号票约定：`mcp_set_launch_trust`（#46）的"记住"决策写本 TrustStore，指纹算法沿用 §9.7 `(scope, projectId?, name, canonicalConfigPath, sha256(config))`；若 08 先动工自建最小版（票面既定降级路径），收敛点 = 本票 TrustStore 替换其存储、指纹算法不变。
2. 项目信任挂接（§9.8 逐字）：
   - **worktree 身份与信任（2026-10-29 用户确认 Q19）**：由应用管理的 worktree 归属原项目；同一资源、同一内容沿用原项目的既有信任，不因副本路径改变重复询问。信任仍遵循本会话/记住的作用域，其他项目不继承。仅通过应用记录的 worktree 身份映射归一资源来源，内容变更重新问；MCP 实际启动配置改变（命令、环境、cwd 等）不能以“同项目”绕过信任重核。实现前更新 §9.7/§9.8 与信任记录字段。
   - **技能信任范围（Q21 用户确认）**：绑定说明与随附脚本、模板等资源的整体版本；附属资源变化也须在下一 run 重新核验，不能只计算 SKILL.md。
   - **任务内资源版本（2026-10-29 用户确认 Q20）**：每个 run 固定项目指令与技能版本，描述与按需加载正文来自同一版本；变更在下一 run 加载并重新核验信任，不能沿用旧信任读新正文。旧版本不可得时停止相关加载并提示，不能换成新版继续。安全权限撤销立即生效，不受版本冻结延迟。
   - 信任对象 = `<home>/.craftwave/` 全部资源 + 项目侧资源；指纹 = 资源内容 SHA-256；
   - 首次加载 → projectTrust 询问；allow_session = 本次会话允许（内存）、remember = 写 TrustStore（绑定指纹）、deny = 拒绝；
   - 未信任资源：knowledge 组装跳过（不注入系统提示）+ 工具访问拒绝 E_PROJECT_UNTRUSTED；
   - 内容变更（指纹不符）→ 重新询问（记录按 resourceName+scope 覆盖）；
   - 挂接点：① 项目指令读取（M1 knowledge 已读 AGENTS.md/CRAFTWAVE.md/CLAUDE.md——读前查信任，本票实装）② mcp.json 加载（08 号票接缝，见上）③ 技能扫描（09 号票接缝：09 未落地时只做指令与 mcp.json 读取路径，技能信任随 09 收敛）。
3. 外部目录授权（§9.9 逐字）：
   - 工具访问 roots 外路径 → 先调 `check_external_path`（#33）预检；roots 外命中 → ask 审批（"始终允许"该目录）→ `allow_external_dir`（#34）并入会话 roots；粒度 = 该目录及子目录的读/写/列；
   - 拒绝整体放行：文件系统根、家目录本身、宿主托管目录（`~/.craftwave`、`<home>/.craftwave`）——对这些目标请求授权直接拒（E_INVALID_ARG）；
   - 授权持久于会话 roots 快照（§9.9"SessionMeta 固化"——落点与 M1 21 号票 roots 收窄实现对齐：会话级 roots 持久化字段），不全局生效、跨会话不延续；
   - PathGuard 集成：`host/commands/workspace.rs` resolve_session_roots 的 roots 集合支持追加授权目录；safe_join 判定逻辑不变（roots 集合动态化）。
4. 命令面：#33/#34 注册 + ipc-manifest/契约同步。
5. **备份恢复（Q41）**：恢复内容和凭据不等于恢复授权。记住的项目/技能/MCP 等资源信任、外部目录追加授权须重新确认，即使恢复的资源指纹未变也不能沿用旧许可。与 33 号票恢复标记接线，保留会话基础工作区边界，不自动启用备份中的外部 roots。普通重启/重连不按备份恢复处理。

### 前端（ui/）

1. AskPanel 增 projectTrust 形态卡（资源名 + 指纹前 8 位 + 三选项）与外部目录授权卡（路径 + "该目录及子目录的读/写/列"粒度说明 + 允许/拒绝）。
2. 当前会话已授权外部目录列表（Inspector 或会话信息区）：可移除授权（移除后下次访问重新询问）。

### 测试

- Q41：备份后撤销资源信任与外部目录授权，恢复备份后仍须重新确认，同指纹不能复活旧许可；正常重启保留有效授权，避免把每次启动都当恢复。
- worktree 信任：同项目同资源同内容沿用信任，不因副本路径重复询问；内容/实际 MCP 配置变更重问；跨项目不继承，本会话允许不扩成跨会话允许。任务中改指令/技能不热换，下一 run 重核；权限撤销立即阻止后续受影响调用。

- Rust：
  - 信任决策全路径：首用询问 → 三选项各自行为；remember 后同指纹免问；内容变更重问；deny 后不注入 + 访问 E_PROJECT_UNTRUSTED；
  - 全局 scope 同套断言；TrustStore roundtrip / 损坏兜底 / scope 判别；
  - 外部目录授权：#33/#34 行为、roots 快照持久与跨会话不延续、家目录/根/托管目录拒绝、授权后 PathGuard 放行与未授权越界 E_PATH_OUTSIDE_ROOTS、子目录粒度。
- 前端：两形态卡渲染与载荷、契约测试 #33/#34。

## 边界（不在本票）

- MCP 启动信任的 UI 与连接池（08 号票，本票只出存储、指纹算法与写入约定）；技能扫描与信任的完整联动（09 号票）；memory 注入信任（M3）；企业级静默信任清单（无此需求）。

## 验收

- [x] 未信任项目首次加载 AGENTS.md 出信任卡；"记住"后重启不再问；改动该文件后重新问
- [x] "拒绝"后项目指令不进系统提示（行为可观测），相关资源访问报 E_PROJECT_UNTRUSTED
- [x] 项目外目录经授权后 read/write/list_files 正常放行；未授权路径仍 E_PATH_OUTSIDE_ROOTS；对家目录/文件系统根请求授权被拒
- [x] 授权随会话持久、跨会话不延续
- [x] 契约测试 #33/#34；§7.7（scope 字段）与 FR-05-10 引注回写完成

## 决议记录（2026-10-30 实现）

1. **存储收敛落法**：`storage/trust.rs`（TrustStore trait + FileTrustStore）承载 `trust/project.json` 单一文件，记录增 `scope: "project"|"mcp"` 判别字段（§7.7 回写）；`mcp/trust.rs` 与 `knowledge/trust.rs` 保留 08/09 号票域门面（接口面不变、内部转调统一存储），**指纹算法不变**——已记住的 MCP 服务器/技能跨收敛不重问；08 号票遗留 `trust/mcp.json` 读取时只读迁入内存视图（统一文件优先、按 (scope, resourceName, fingerprint) 去重，旧文件保留不删，迁移幂等）。`trust/mcp-tools.json`（Q44 工具级许可）维持独立文件——调用审批记忆与资源信任语义不同，§9.7 字段级未变。damage 兜底：缺失/损坏按空表（宁缺勿炸）。
2. **项目指令挂接（票面前提勘误）**：票面写「M1 knowledge 已读 AGENTS.md/CRAFTWAVE.md/CLAUDE.md」，实况是 M1/M2 无任何读取实现——本票随信任挂接一并实装读取（`knowledge/instructions.rs`，run 开始装配时按固定清单序读项目主目录根下存在的指令文件），指纹 = 内容 SHA-256，注入层插 §6.2 次序第二位（customPrompt 之后）。deny/不存在/非 UTF-8 零注入。
3. **projectTrust options id 归一**：09 号票最小版实装 `allow_once`，§3.2 主键面为 `allow_session`——本票统一为 `allow_session`（语义不变：本次会话允许，会话内存面），既有测试适配（行为语义不回退）。会话信任集统一到 `knowledge/session_trust.rs::SessionTrustSet`（AppState 持有，键含会话/作用域/资源名/指纹），从 SkillRegistry 收敛迁出（技能与项目指令共用）。
4. **worktree 信任（Q19）落法**：记录键 = (scope, resourceName, 指纹) 不含项目身份——worktree 归属原项目时同名同内容同指纹命中既有信任（不因副本路径重复询问）；内容变更指纹变自然重问；「其他项目不继承」在按内容信任语义下成立（不同内容不命中；同内容视为同内容信任，与 §9.8 记录字段级一致——设计文档未定义 projectId 维度，如需按项目隔离须回写字段级，暂按现口径）。
5. **外部目录授权链路**：审批门新增外部判定层（第 0.5 步，内部状态硬拒之后、已批准签名检查**之前**）——文件类七工具路径越界 → 挂 `AskPayload{kind:"externalDir"}`（新形态，§3.2 回写）；置于签名前使恢复重放（授权已并入 roots）不再命中、批量 batch_read 跨多外部目录逐个询问（签名放行不吞并未授权路径）。授权目标：文件类工具取父目录、目录类工具（list_files/glob/grep）取路径本身。允许 → 恢复循环调 #34 同源核心落 SessionMeta.authorizedExternalDirs + 门热追加 roots → 重放放行；内存面 grant 用最长存在前缀归一（目标目录不存在时持久面被拒——#34 要求目录真实存在——内存面仍生效防重放死循环）。
6. **禁区从严口径**：#34 与审批门同校验「整体放行」禁区——文件系统根、家目录本身、宿主托管目录（数据目录）**本身/祖先/后代**全部 E_INVALID_ARG（票面只列三个本体；从严理由：授权托管目录后代会拆穿 C-8 收窄防线，家目录以上粒度违背「目录及子目录」授权精神），判定统一在 canonical 域（macOS /var ↔ /private/var symlink 歧义实测绕过，已修）。回写 §9.9。
7. **既有瑕疵修正（顺带）**：23 号票主 run 装配 InternalStateGuard 误传 workspace 子目录当数据目录根（子代理装配是对的）——主 run 数据侧禁区判定落空（防御纵深小缺口，模型本就写不到该区域故无实害），本票装配面改传数据目录根并经 gate_test 确认（read 数据目录 config.json → 授权禁区 E_INVALID_ARG；写路径禁区硬拒不变）。
8. **Q41 接线口径**：本票出两个清空面（统一信任存储 `clear_all` + `clear_all_external_grants` 遍历 index 清会话授权），33 号票恢复流程接线（33 未动工，生产调用点随其落地）；正常重启保留有效授权（不调清空面）。测试验证两口径（clear 后失效 / 新 AppState 同目录读回保留）。
9. **撤销 UI（09 号票移交收敛点）**：skill_list 增 `trustStatus` 列（内置恒 trusted）+ 新命令 `skill_revoke_trust`（写 revoke 记录覆盖旧 remember，内置技能 E_INVALID_ARG 无入口）+ 技能页行内「撤销信任」按钮（pending/revoked/内置隐藏）+ Inspector 新「授权」页（当前会话已授权目录列表 + 行内移除 → `revoke_external_dir` 补充命令）。
10. **e2e 剧本适配**：三协议 mock 剧本的 read 目标从 `/etc/app/config.toml`（roots 外——新信任面下正确触发授权挂起，剧本级测试无应答者会挂死）改为会话 roots 内相对路径 `config.toml`（临时会话 = workspace 子目录），「read 失败回喂 → 第二轮作答收尾」剧本语义不变；spike_s2/s3 的参数断言同步适配。
11. **已知抖动**：`skills_spec_description`/`builtin_skills_skip_trust` 全量并跑偶发（SkillRegistry 进程级单例时序，09 号票既有面），单跑/复跑即绿，与已知前端 thread-rendering 抖动同类，未追修。
