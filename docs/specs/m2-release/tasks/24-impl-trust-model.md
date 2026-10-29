# 实现：信任模型——项目 TOFU 与外部目录授权（FR-05-10 + FR-05-12）

Type: task
Status: open

## 要做什么

项目 `.craftwave/` 里的资源（技能/记忆/mcp.json/项目指令）是第三方注入面：首次加载须让用户知情决策（本次/记住/拒绝），内容一变重新问；项目外目录按"目录及子目录"粒度授权而不是整盘放开。本票同时交付统一信任存储，作为 MCP（08 号票）与技能（09 号票）信任的接缝。

FR 依据（原文摘引）：
- FR-05-10：项目 `.craftwave/` 资源（skills/memory/mcp.json/项目指令）首次加载需信任决策（本次/记住/拒绝），绑定内容 SHA-256
- FR-05-12：项目外目录访问需"始终允许"授权（粒度=目录及子目录读/写/列）；拒绝对文件系统根/家目录整体放行
- §9.8：指纹=资源内容 SHA-256 → AskPayload{kind:"projectTrust"}（本次会话允许/记住此版本/拒绝）；决策写 `trust/project.json`（§7.7）；未信任资源不注入系统提示且工具访问拒绝（E_PROJECT_UNTRUSTED）；内容变更重新询问；全局作用域（`~/.craftwave/skills/`、`memories/`）首用同样走信任（scope=global）
- §9.9：会话 roots 初始 = 项目主目录 + 数据目录；roots 外路径 → 预检 `check_external_path` → ask（始终允许 = `allow_external_dir` 入会话 roots 快照）；拒绝整体放行文件系统根、家目录本身、宿主托管目录（`~/.craftwave`、`<home>/.craftwave`）；授权持久于会话 roots 快照，不全局生效
- §3.2 已有 AskPayload `{kind:"projectTrust", resourceName, fingerprint}`（options `allow_session|remember|deny`）；§3.3 #33 `check_external_path { sessionId, path } → { allowed, insideRoots }`、#34 `allow_external_dir { sessionId, dir } → { roots: string[] }`；§3.6 已有 E_PROJECT_UNTRUSTED

## 具体做什么

### 后端（src-tauri/）

1. 信任存储：新增 `storage/trust.rs`（TrustStore trait + 文件实现），落 `trust/project.json`（§7.7 字段级：`[{ resourceName, fingerprint, decidedAt, decision }]`，原子写）。
   - **存储口径**：FR-05-10 行文"存钥匙串"为 v2.5 前口径，以设计文档 v2.9（§7.7/§9.8 落文件）为准，随 Answer 回写 FR 引注；
   - **与 MCP 的接缝约定**：§9.7 MCP 启动信任写 keyring 账户 `craftwave.trust.v1`，但 v2.5 已废 keyring（§9.4）——本票把 TrustStore 扩为两类信任统一承载（记录增 `scope: "project"|"mcp"` 判别字段，**回写 §7.7**）；
   - 08 号票约定：`mcp_set_launch_trust`（#46）的"记住"决策写本 TrustStore，指纹算法沿用 §9.7 `(scope, projectId?, name, canonicalConfigPath, sha256(config))`；若 08 先动工自建最小版（票面既定降级路径），收敛点 = 本票 TrustStore 替换其存储、指纹算法不变。
2. 项目信任挂接（§9.8 逐字）：
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

### 前端（ui/）

1. AskPanel 增 projectTrust 形态卡（资源名 + 指纹前 8 位 + 三选项）与外部目录授权卡（路径 + "该目录及子目录的读/写/列"粒度说明 + 允许/拒绝）。
2. 当前会话已授权外部目录列表（Inspector 或会话信息区）：可移除授权（移除后下次访问重新询问）。

### 测试

- Rust：
  - 信任决策全路径：首用询问 → 三选项各自行为；remember 后同指纹免问；内容变更重问；deny 后不注入 + 访问 E_PROJECT_UNTRUSTED；
  - 全局 scope 同套断言；TrustStore roundtrip / 损坏兜底 / scope 判别；
  - 外部目录授权：#33/#34 行为、roots 快照持久与跨会话不延续、家目录/根/托管目录拒绝、授权后 PathGuard 放行与未授权越界 E_PATH_OUTSIDE_ROOTS、子目录粒度。
- 前端：两形态卡渲染与载荷、契约测试 #33/#34。

## 边界（不在本票）

- MCP 启动信任的 UI 与连接池（08 号票，本票只出存储、指纹算法与写入约定）；技能扫描与信任的完整联动（09 号票）；memory 注入信任（M3）；企业级静默信任清单（无此需求）。

## 验收

- [ ] 未信任项目首次加载 AGENTS.md 出信任卡；"记住"后重启不再问；改动该文件后重新问
- [ ] "拒绝"后项目指令不进系统提示（行为可观测），相关资源访问报 E_PROJECT_UNTRUSTED
- [ ] 项目外目录经授权后 read/write/list_files 正常放行；未授权路径仍 E_PATH_OUTSIDE_ROOTS；对家目录/文件系统根请求授权被拒
- [ ] 授权随会话持久、跨会话不延续
- [ ] 契约测试 #33/#34；§7.7（scope 字段）与 FR-05-10 引注回写完成
