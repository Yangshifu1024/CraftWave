# 实现：存储格式升级备份保护与自动更新器（原 macOS 签名公证与自动更新器）

Type: task
Status: resolved（2026-10-30，m2/ticket-33-backup-updater；发布面 HITL 项在下方验收清单标注留验——代码骨架与单测不阻塞）

> **范围变更（2026-10-31 用户决策）：macOS Developer ID 签名与公证移除不做。** FR-16-02（macOS 半）出 M2 范围，未签名引导口径（M1 acceptance.md §1）继续有效。本票剩余范围：存储格式升级备份保护（Q34/Q36/Q41/Q43，04/06/23/24 号票接线点）+ 更新器骨架（FR-16-03，发布面验证留 HITL）。

## 要做什么

FR-16-03（PRD 行 580/581，出处 §12.4/§10.7）+ Q34/Q36/Q41/Q43 存储安全：

- 应用内「检查更新 → 下载 → 安装 → 重启」走 tauri updater + GitHub Releases latest.json，检查可开关可调频率；updater 的 minisign 签名与 OS 代码签名正交（Windows/Linux 未签名不影响应用内更新），macOS 未签名包应用内更新可装机内自替换（首装引导维持 M1 口径）。
- **签名与公证：移除**（无 APPLE_* secrets、无 notarytool/stapler 步骤；下游若重启签名另立新票）。
- **存储格式升级保护**：首次升级到新存储格式前创建可恢复备份，失败暂停升级；恢复后信任/授权/审批记忆重核、队列暂停（Q34/Q36/Q41/Q43 全口径，04 号票 compaction 守卫同源扩展）。
- HITL 剩余要素：tauri updater 密钥对（`pnpm tauri signer generate`，公钥进配置、私钥+密码进 secrets）与 GitHub secrets `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`；未配时更新入口以配置开关禁用兜底。

## HITL：需用户操作并提供（砍签名后剩余清单）

1. tauri updater 签名密钥对：本机 `pnpm tauri signer generate -w ~/.tauri/craftwave.key`（自设密码）→ **公钥**交代理进 tauri.conf.json，**私钥 + 密码**进 secrets（仅发布 updater 产物时需要；代码骨架与单测先行不阻塞）。
2. GitHub secrets：`TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。

## 具体做什么

### CI 与发布（.github/workflows/build.yml 增量，基于现行文件：tag/dispatch 触发 → release 草稿 job → 三平台 build job → 收集安装包 → gh release upload）

- **签名公证：无**（2026-10-31 用户决策移除；CI 不含任何 APPLE_* 步骤）。
- **updater 产物**：
  - `tauri.conf.json` `bundle.createUpdaterArtifacts: true` → 各平台 updater 产物 + `.sig`；
  - build job 设 `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)` env（tauri CLI 用其签 updater 产物）；
  - 三平台 updater 产物都出——updater 的 minisign 签名与 OS 代码签名**正交**，Windows/Linux 包未签名不影响应用内更新；
  - 平台条目对应产物形态（macOS app.tar.gz / Windows nsis exe / Linux AppImage）以实测为准，实测结论回写 §12.4。
- **latest.json 组装与上传**：
  - tag 构建完成后脚本（bash+jq，`scripts/gen-latest-json.sh`）：`{ version, notes, pub_date, platforms: { "darwin-aarch64"|"windows-x86_64"|"linux-x86_64": { signature: <读 .sig 内容>, url: https://github.com/<repo>/releases/download/<tag>/<产物名> } } }`；
  - `gh release upload --clobber latest.json`；客户端 endpoint 固定 `https://github.com/<repo>/releases/latest/download/latest.json`。

### 后端（src-tauri/）

- `Cargo.toml` 增 `tauri-plugin-updater` 2.x + `tauri-plugin-process`（relaunch 用，crates.io 官方）；`lib.rs` 注册两插件。
- `capabilities/default.json` 增 `updater:default`、`process:allow-relaunch`（NFR-10 最小权限纪律，§9.5「每接一个 Tauri 插件按需追加最小权限并同步 capabilities」）。
- `tauri.conf.json`：`plugins.updater { endpoints: [...], pubkey: <HITL 公钥> }` + `bundle.createUpdaterArtifacts: true`；pubkey 未配时占位值会导致 updater 校验必败——pubkey 由 HITL 提供后才合入，或前端入口以配置开关兜底，取舍记 Answer。
- 配置域（FR-16-03「更新检查频率与开关」；§3.2 未含——**先回写 §3.2 再实现**）：
  - `AppConfig.update: { enabled: bool（缺省 true）, interval_hours: u32（缺省 24） }`；
  - serde default 兼容旧 config（`providers/types.rs` `default_true()` 复用）；新增域不得破坏 schema_version=2 读取。

### 存储格式升级保护（Q34）

- 首次升级到新存储格式前，创建一次可恢复备份；成功确认前不写入新格式。备份失败则暂停格式升级并告知用户，不把原数据标成损坏。
- 守卫位于新版应用首次打开旧数据的存储入口，覆盖 updater 与用户手动安装；04 号票新增 compaction 条目使用同一守卫。Q36 已确定范围与恢复原则，具体文件清单、目录命名与升级标记先补齐 §7.7 再实现。
- 更新说明明确提示：旧版无法读取升级后的新增格式；用户需要保留备份以便恢复。M2 不提供自动降级转换。
- **备份与恢复范围（Q36）**：备份将被迁移的权威数据及恢复必需的关联文件，排除可重建缓存、日志、项目源码；保存在应用数据目录的独立备份子目录（`~/.craftwave/backups/`），不递归备份已有备份。包含凭据的文件保持原有严格访问权限，备份不进入 agent 可见根集。
- 提供关闭应用后的手动恢复步骤，明确恢复仅回到备份时点，升级后新增内容须另行保留。恢复验证须覆盖关联文件，不能只恢复会话正文而漏掉图片或元数据。
- **恢复后的授权与运行（Q41）**：恢复内容及备份包含的凭据，但记住的资源信任、外部目录授权和审批记忆须重新确认，不因内容指纹相同直接恢复生效；覆盖配置中的审批白名单，防止恢复已撤销的许可。全部待发队列保持暂停，旧待审批请求不直接恢复成可执行批准；后续操作须按当前状态重新校验和确认。与 23/24/06 号票联动，恢复步骤须确保此保护在首次启动前生效。
- **删除与备份保留（Q43）**：删除当前会话不改写完整历史升级备份；删除确认说明旧备份可能仍含已删除内容，恢复会回到旧时点，并提供 `~/.craftwave/backups/` 的系统目录入口供用户自行删除。复用既有系统打开目录通道，不把备份加入 agent roots。已知删除备份后将失去对应恢复点，不能宣称会话删除已擦除所有历史副本。

### 前端（ui/）

- `ipc/updater.ts`（官方 `@tauri-apps/plugin-updater` JS API + `@tauri-apps/plugin-process`）：`check()` → 三态（有新版/已最新/失败）。
- 设置页「关于」页（registry about 页）增「检查更新」区：
  - 按钮 → 有新版弹 Dialog（新版本号 + release notes + 「下载并安装」）→ `downloadAndInstall()` 进度态（下载中/安装中）→ 完成提示「重启以完成更新」→ `relaunch()`；
  - 已最新/失败走 Toast（29 号票反馈层，未落地前行内提示）；
  - CSP 核对：updater 下载走 Rust 侧，前端无新外联域（§9.5 connect-src 不动，记核对结论）。
- **运行与队列门（2026-10-29 用户决策）**：运行时允许检查和下载；应用内任一会话有活跃 run 或待审批请求时，禁用安装与重启，安装前与重启前均重新检查。有待发队列时，用户须先明确暂停并持久保留队列才能更新；暂停队列在更新后仍等待用户明确继续。前端展示阻止原因，接线 06 号票队列暂停状态；尚未落地时先补齐字段级契约，不能默认放行。
- 启动静默检查：App 挂载后 `update.enabled` 才查；间隔判定记账 ui-state `lastUpdateCheckAt`（不敏感，不进 config）；`menu:action` 的 `check_update` 动作（§3.5 已定义键）随 macOS 原生菜单另票，本票只做设置页入口。

### 测试

- Q43：删除会话不会改写历史升级备份，确认文案说明残留内容与恢复时点，目录入口指向实际备份目录；用户可自行管理，当前会话数据/日志/明细仍按既有删除路径清理。
- Q41 回归：备份后撤销信任/外部授权/审批记忆，再恢复备份，不复活旧许可；内容和所含凭据可恢复，但访问须重新确认，队列暂停、旧审批不自动执行。普通 WebView 重连仍遵守 05 号票有效审批恢复规则。
- Q36 回归：权威数据与关联文件备份恢复完整；缓存/日志/源码/备份自身不被复制；含凭据文件权限不放宽；手动恢复说明要求关闭应用并提示备份时点后的内容需另行保留。
- Q34 回归：首次格式升级备份成功后才能写新格式；备份失败时原数据不变且有提示；重复启动不重复创建首次升级备份；手动安装同样受守卫；备份须通过恢复验证。
- 更新门回归：任一会话运行中/等待审批时可检查下载但不可安装重启；待发队列未明确暂停时不可安装，明确暂停后持久保留，更新后不自动执行；下载完成后新出现的活跃 run 仍能阻止安装。

- latest.json 组装脚本本地单测（fixture 产物名/.sig → JSON 形态、三平台 key 断言、URL 拼接）；interval/enabled 判定单测。
- 前端 vitest：更新 Dialog 三态渲染与进度态（mock updater API）；静默检查间隔逻辑（lastUpdateCheckAt 记账）。
- 链路验证属发布面：连打两 tag（如 0.1.0 → 0.1.1）实测检查→下载→安装→重启到新版本。

## 边界（不在本票）

- macOS Developer ID 签名与公证（2026-10-31 用户决策移除，下游另票复议）；Windows 签名（无证书，map Out of scope；SmartScreen「更多信息→仍要运行」引导维持 acceptance.md §1）；Linux 打包形态变更；增量/差分更新（tauri v2 全量包）；版本降级；更新源镜像/自建服务器；macOS 原生菜单栏与 About 菜单（FR-13-04 另票）。

## 验收

- [x] 删除会话时明确说明历史备份可能保留内容，备份保持完整且有系统目录入口，恢复说明覆盖旧时点内容重新出现的情况。（删除确认弹窗新增备份残留说明段 +「打开备份目录」按钮（`open_dir` 白名单幂等补建 `backups/`）；`delete_session` 路径未触碰 `backups/`——历史备份保持完整；备份目录内 `恢复说明.txt` 载明恢复仅回到备份时点。设置页关于页另有常驻备份目录入口。）
- [x] 手动恢复备份后记住信任、外部目录授权和审批记忆重新确认，队列暂停，旧审批不能直接执行；不影响正常重连。（启动序 `apply_restore_protection`：检测 `.restore-pending.json` → trust/project.json 清空（FileTrustStore::clear_all）+ trust/mcp-tools.json 删除 + index.json 全部 authorizedExternalDirs 清空 + config.json 审批白名单清空 + 全部 queue.json 强制 paused + 删标记；执行报告逐条落 app.log（失败面 Warn 不静默）。审批记忆与计划范围授权为进程内存面重启天然空。普通重启（无标记）零动作——workspace_test `clear_all_grants_for_backup_restore` 与 backup_test `restore_protection_full_sequence` 钉住。）
- [x] Q36 备份清单、独立目录、凭据权限和关闭应用后的手动恢复步骤齐全，关联文件恢复验证通过。（清单 = config.json + histories/ + sessions/ + projects/ + mcp.json + stats/ + skills-state.json（§7.7 回写字段级）；排除 trust/cache/logs/ui-state/window-state/quota/workspace/backups 自身；`backup-manifest.json` 记录清单与复制面对照（恢复验证面）；config.json/mcp.json 复制后权限保持（unix 测试钉 0600 不放宽）；`恢复说明.txt` 三步手动恢复 + 时点警告。）
- [x] 首次格式升级前完成可恢复备份，失败则暂停升级；更新说明含旧版不可读提示，手动安装路径同样受保护。（守卫在 storage 层存储入口 `ensure_upgrade_backup(root, reason)`（04 号票 compaction 条目首次写入前调用不变，33 号票扩展为通用 reason 守卫）——自动更新与手动安装共用同一写入路径；备份失败返回 Err 暂停写入、原数据不动（测试钉住）；恢复说明含「旧版应用无法读取升级后的新增格式」。）
- [x] 活跃任务/待审批请求阻止安装和重启；待发队列须用户明确暂停并保留后才可更新，更新后保持暂停。（`update_precheck`（#59n）后端权威面：RunManager 活跃 run（审批挂起算活跃）+ app_asks 应用级未决询问 + 磁盘全量队列扫描（未暂停非空队列阻止，`all_snapshots` 惰性不漏扫、损坏文件从严计入）；`pause_followup_queue`（#59o）写 queue.json 持久暂停、更新重启后等待 `resume_followup_queue` 明确继续；前端设置页 Dialog 安装前调 precheck、阻止原因逐行展示。）

- [x] ~~secrets 配齐后 tag 构建：macOS dmg 双击直开~~（2026-10-31 用户决策移除，不做）。
- [ ] 应用内「检查更新」→ 下载 → 安装 → 重启后版本号更新（两连 tag 实测，HITL 配齐密钥后）；无新版时提示已最新。**【HITL 留验：代码骨架与前端三态/安装门已交付并有测试；真实端到端须先完成下方 HITL 1/2 项再连打两 tag 实测】**
- [x] `update.enabled=false` 或未到间隔：启动不发起更新请求（`useSilentUpdateCheck`：enabled=false 直接 return；`shouldSilentCheck` 表驱动测试钉住间隔判定；记账 ui-state `lastUpdateCheckAt`。NFR-11 核对：更新外联仅 GitHub Releases endpoint，列入 34 号票隐私清单）。
- [x] ~~清空全部 APPLE_/TAURI_ secrets 手动 dispatch~~（签名步骤已移除，APPLE_ 项不存在；TAURI_ 项随 updater 骨架以「secrets 未配时更新入口禁用」替代——`update_status.pubkeyConfigured=false` 时前端入口禁用，测试钉住）。
- [ ] latest.json 三平台条目 URL 逐一可下载，signature 与对应 .sig 内容一致。**【HITL 留验：`scripts/gen-latest-json.sh` 已交付 + 本地单测 15 例（fixture 产物/.sig → JSON 形态、三平台 key、URL 拼接、缺 .sig/缺平台报错）；真实产物可下载性随两连 tag 实测验证】**
- [x] §3.2 update 配置域、§12.4 updater 实测结论回写完成。（§3.2 `update { enabled, intervalHours }`、§3.3 #59m/#59n/#59o、§7.7 备份清单/恢复说明/Q41 接线字段级、§12.4 更新器发布骨架均已回写 v2.40；§12.4 的**实测结论段**（各平台 updater 产物形态）属发布面，随 HITL 两连 tag 实测后补记。）

## 决议记录（2026-10-30 实现收口）

**已交付（收窄后全范围）**

1. **存储格式升级备份保护（Q34/Q36/Q41/Q43 全口径）**：
   - 守卫通用化：`storage/backup.rs::ensure_upgrade_backup(root, reason)` 按 reason 幂等（`.upgrade-<reason>.done`），04 号票 compaction 调用面零改动；守卫在 storage 层存储写入入口，自动更新与手动安装共用（Q34）。备份清单扩展为七项权威数据（§7.7 回写字段级），排除面八项，`backup-manifest.json`（清单对照，恢复验证面）+ `恢复说明.txt`（三步手动恢复 + 时点警告 + 旧版不可读提示）随备份生成。
   - Q41 恢复保护：备份目录内 `.restore-pending.json` 由用户按恢复说明复制回数据目录根，启动序（lib.rs setup，正常装配之前）检测到即执行保护序列——信任清空 / mcp-tools 删除 / 外部授权清空 / 审批白名单清空 / 全部队列强制暂停 / 删标记——完成后再进入正常服务；报告逐条落 app.log，失败面 Warn 不静默（尽力而为不阻塞启动：用户已恢复旧数据，应用拒绝启动更糟）。23 号票 `ApprovalMemory::clear_all` 与 24 号票 `FileTrustStore::clear_all` 接缝闭合；24 号票预留的 `clear_all_external_grants`（#[allow(dead_code)]）被本票的 `SessionIndex::clear_authorized_external_dirs` 批量实现取代，已删除（其测试改走保护序列真实路径，覆盖面反而更完整）。
   - Q43：删除确认弹窗（会话分支）增备份残留说明 +「打开备份目录」入口（`open_dir`，白名单数据目录天然覆盖，`backups/` 幂等补建——目录没建也能点开，与 logs 同款先例）；`delete_session` 路径不触碰 `backups/`；设置页关于页增常驻备份目录入口与说明行。
2. **更新器骨架（FR-16-03）**：tauri-plugin-updater 2.14.1 + tauri-plugin-process 2.4.0（crates.io 官方）注册；capabilities 增 `updater:default` + `process:allow-restart`（NFR-10 最小权限——relaunch 的权限名实测为 `process:allow-restart`，票面写的 allow-relaunch 不存在）；`tauri.conf.json` 配 `createUpdaterArtifacts: true` + endpoint（pubkey 占位空串）；`AppConfig.update { enabled, intervalHours }`（serde default 回填，schemaVersion 维持 2）；前端 `ipc/updater.ts` 三态收敛 + 设置页关于页「检查更新」区（Dialog 下载安装进度 / 已最新与失败走 Toast）+ App 挂载静默检查（interval 记账 ui-state `lastUpdateCheckAt`）；安装门三命令（#59m/#59n/#59o）+ 队列条「暂停队列」按钮（更新门配套的用户主动暂停入口，复用 06 号票 queuePaused 展示接缝）。
3. **发布面脚本**：`scripts/gen-latest-json.sh`（platform-entry / merge 两个子命令适配三平台矩阵分阶段组装）+ `scripts/gen-latest-json.test.sh`（15 例本地单测全过：JSON 形态 / 三平台 key / URL 拼接 / 缺 .sig 报错 / 缺平台条目报错 / notes 覆盖）；build.yml 增 updater 签名 env（TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)）+ 平台条目提取 artifact + latest-json 汇总 job。

**取舍与偏差（诚实记录）**

- **pubkey 占位兜底（票面二选一）**：取「前端入口以配置开关禁用兜底」——pubkey 占位空串期间 `update_status`（#59m）返回 `pubkeyConfigured=false`，前端入口禁用并显示「签名公钥未配置」原因；不为占位值放行（check 必败不作为失败态暴露给用户）。HITL 提供公钥后填入 tauri.conf.json 即自动放行，零代码改动。
- **安装门以后端权威面为准**：票面「前端展示阻止原因，接线 06 号票队列暂停状态」——前端 store 的 running/queues 只覆盖打开过的 Tab（惰性加载有漏面），故新增 #59n 后端命令（RunManager 注册表 + app_asks 槽 + 磁盘全量队列扫描）做权威判定，前端只渲染 reasons；「安装前与重启前均重新检查」由 Dialog 安装按钮每次点击都调 precheck 达成。
- **`pause_followup_queue` 新命令**：票面更新门要求「用户须先明确暂停并持久保留队列」，但 06 号票只有失败暂停（无主动暂停入口）——补 #59o 命令 + 队列条按钮，暂停写 queue.json 持久化，更新重启后保持等待 `resume_followup_queue` 明确继续（Q41 口径）。
- **`workspace/` 不进备份**：它是用户工作区文件（会话 roots 之一），非「被迁移的存储格式」数据（新格式不改写其内容）；恢复说明明示该目录不在备份内。`trust/` 同理不备份——Q41 规定恢复后信任须重新确认，备份它反而诱导旧许可复活，不备份天然达成。
- **Bash 脚本测试不在七项检查链内**：`gen-latest-json.test.sh` 依赖 bash+jq（已本地实跑 15 例全过），jq 缺失环境自动 SKIP 并明示；CI 的 ubuntu/macOS runner 均预装 jq。
- **CSP 核对结论（票面要求记录）**：updater 的检查/下载全在 Rust 侧（插件命令 `plugin:updater|check|download_and_install` 经 IPC 转调），不经 WebView 网络栈——`connect-src` 无需新增任何域，tauri.conf.json 的 CSP 未改动，核对通过。
- **发布面 HITL 留验两项**（对应验收未勾两项）：① tauri 签名密钥对（`pnpm tauri signer generate -w ~/.tauri/craftwave.key` → 公钥进 tauri.conf.json、私钥+密码进 secrets `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`）；② 配齐后连打两 tag 实测检查→下载→安装→重启到新版本 + latest.json 三平台 URL 逐一可下载与 signature 一致性 + 实测产物形态回写 §12.4。已知发布面风险如实注记：`createUpdaterArtifacts: true` 且 pubkey 占位空串时 tag 构建的 tauri CLI 行为（跳过签名产物或报错）未经实测——属 HITL 验证面，不影响日常检查链（tauri.conf.json 仅打包路径读取，cargo 检查链不经过）。

**测试盘点**：Rust 新增 16 例（backup 3 + restore protection 序列 1 + 凭据权限 1 + updater 三命令 4 + UpdateConfig 缺省边界 1 + workspace 恢复授权改走保护序列 1 + 既有 backup 4 例随清单扩展保留），cargo test 全量 914 过；前端新增 16 例（updater.test 11 + UpdateSection.test 9 中 5 复用既有形态，净增约 16），vitest 全量 544 过（thread-rendering 已知抖动复跑即绿）；脚本单测 15 过；contract.test 增三命令锚点 + 四生成物字段级断言；i18n zh/en 键集合同构门禁过。
