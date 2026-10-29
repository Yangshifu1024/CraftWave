# 实现：macOS 签名公证与自动更新器

Type: task
Status: open

## 要做什么

FR-16-02（macOS 半）+ FR-16-03（PRD 行 580/581，出处 §12.4/§10.7）：

- tag 构建的 macOS 安装包带 Developer ID 签名与 notarization——用户下载后**直接双击可开**，不再右键放行/xattr（M1 acceptance.md §1 的未签名引导口径就此对 macOS 作废）。
- 应用内「检查更新 → 下载 → 安装 → 重启」走 tauri updater + GitHub Releases latest.json，检查可开关可调频率。
- **用户已注册 Apple Developer 账号（2026-10-29 决策）；Windows 维持未签名**（无证书，map Out of scope；引导文案维持 M1 acceptance.md §1 口径）。
- 证书/密钥属 HITL 要素——代理先把 CI 骨架与守卫写好，secrets 配齐即自动生效。

## HITL：需用户操作并提供（票内精确清单）

1. Apple Developer 后台创建 **Developer ID Application** 证书（含私钥导出 `.p12`）+ 导出密码 + 证书身份全名（如 `Developer ID Application: <姓名> (<TeamID>)`）。
2. 公证凭据（二选一，推荐 A）：
   - A. **App Store Connect API Key**：`.p8` 文件 + Key ID + Issuer ID（notarytool 原生支持，CI 免交互）；
   - B. Apple ID + **App 专用密码**（appleid.apple.com 生成）+ Team ID。
3. tauri updater 签名密钥对：本机 `pnpm tauri signer generate -w ~/.tauri/craftwave.key`（自设密码）→ **公钥**交代理进 tauri.conf.json，**私钥 + 密码**进 secrets。
4. GitHub secrets 名单（代理按此名接线，本票 Answer 记配置完成日）：
   - `APPLE_CERTIFICATE_P12`（base64）、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_SIGNING_IDENTITY`；
   - `APPLE_API_ISSUER`、`APPLE_API_KEY_ID`、`APPLE_API_KEY`（.p8 内容 base64；选 B 路线则改 `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`）；
   - `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。

## 具体做什么

### CI 与发布（.github/workflows/build.yml 增量，基于现行文件：tag/dispatch 触发 → release 草稿 job → 三平台 build job → 收集安装包 → gh release upload）

- **macOS 签名公证步骤**（macOS build job 内，每段套 `if: env.APPLE_CERTIFICATE_P12 != ''` 式守卫——**secrets 未配时整段跳过，退化为 M1 未签名构建，不失败**）：
  1. 解码 `.p12` → 创建临时 keychain（`security create-keychain` → `import-keychain` → `set-key-partition-list -S apple-tool:,apple:` 免交互授权）；
  2. tauri build 带 `APPLE_SIGNING_IDENTITY` env → bundler 对 .app/dmg 自动 codesign；
  3. 公证：`xcrun notarytool submit <dmg> --wait`（A 路线 env `APPLE_API_ISSUER/APPLE_API_KEY_ID` + .p8 落盘为 key-path；B 路线先 `xcrun notarytool store-credentials` 预存 profile 再 `--keychain-profile`）；
  4. `xcrun stapler staple` 订书 → 产物挂 Release 草稿（既有「收集安装包」步骤自动命中，产物名不变：`CraftWave_<ver>_aarch64.dmg`）。
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

- Windows 签名（无证书，map Out of scope；SmartScreen「更多信息→仍要运行」引导维持 acceptance.md §1）；Linux 打包形态变更；增量/差分更新（tauri v2 全量包）；版本降级；更新源镜像/自建服务器；macOS 原生菜单栏与 About 菜单（FR-13-04 另票）。

## 验收

- [ ] 删除会话时明确说明历史备份可能保留内容，备份保持完整且有系统目录入口，恢复说明覆盖旧时点内容重新出现的情况。
- [ ] 手动恢复备份后记住信任、外部目录授权和审批记忆重新确认，队列暂停，旧审批不能直接执行；不影响正常重连。
- [ ] Q36 备份清单、独立目录、凭据权限和关闭应用后的手动恢复步骤齐全，关联文件恢复验证通过。
- [ ] 首次格式升级前完成可恢复备份，失败则暂停升级；更新说明含旧版不可读提示，手动安装路径同样受保护。
- [ ] 活跃任务/待审批请求阻止安装和重启；待发队列须用户明确暂停并保留后才可更新，更新后保持暂停。

- [ ] secrets 配齐后 tag 构建：macOS dmg 下载后**双击直开**（无右键放行/xattr），`codesign -vv` 与 `spctl -a -vv` 通过。
- [ ] 应用内「检查更新」→ 下载 → 安装 → 重启后版本号更新（两连 tag 实测）；无新版时提示已最新。
- [ ] `update.enabled=false` 或未到间隔：启动不发起更新请求（NFR-11 核对：更新外联仅 GitHub Releases，列入 34 号票隐私清单）。
- [ ] 清空全部 APPLE_/TAURI_ secrets 手动 dispatch：三平台未签名构建照常绿、无 updater 产物步骤失败。
- [ ] latest.json 三平台条目 URL 逐一可下载，signature 与对应 .sig 内容一致。
- [ ] §3.2 update 配置域、§12.4 updater 实测结论回写完成。
