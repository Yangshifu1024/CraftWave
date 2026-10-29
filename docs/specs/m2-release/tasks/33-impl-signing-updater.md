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

### 前端（ui/）

- `ipc/updater.ts`（官方 `@tauri-apps/plugin-updater` JS API + `@tauri-apps/plugin-process`）：`check()` → 三态（有新版/已最新/失败）。
- 设置页「关于」页（registry about 页）增「检查更新」区：
  - 按钮 → 有新版弹 Dialog（新版本号 + release notes + 「下载并安装」）→ `downloadAndInstall()` 进度态（下载中/安装中）→ 完成提示「重启以完成更新」→ `relaunch()`；
  - 已最新/失败走 Toast（29 号票反馈层，未落地前行内提示）；
  - CSP 核对：updater 下载走 Rust 侧，前端无新外联域（§9.5 connect-src 不动，记核对结论）。
- **运行与队列门（2026-10-29 用户决策）**：运行时允许检查和下载；应用内任一会话有活跃 run 或待审批请求时，禁用安装与重启，安装前与重启前均重新检查。有待发队列时，用户须先明确暂停并持久保留队列才能更新；暂停队列在更新后仍等待用户明确继续。前端展示阻止原因，接线 06 号票队列暂停状态；尚未落地时先补齐字段级契约，不能默认放行。
- 启动静默检查：App 挂载后 `update.enabled` 才查；间隔判定记账 ui-state `lastUpdateCheckAt`（不敏感，不进 config）；`menu:action` 的 `check_update` 动作（§3.5 已定义键）随 macOS 原生菜单另票，本票只做设置页入口。

### 测试

- 更新门回归：任一会话运行中/等待审批时可检查下载但不可安装重启；待发队列未明确暂停时不可安装，明确暂停后持久保留，更新后不自动执行；下载完成后新出现的活跃 run 仍能阻止安装。

- latest.json 组装脚本本地单测（fixture 产物名/.sig → JSON 形态、三平台 key 断言、URL 拼接）；interval/enabled 判定单测。
- 前端 vitest：更新 Dialog 三态渲染与进度态（mock updater API）；静默检查间隔逻辑（lastUpdateCheckAt 记账）。
- 链路验证属发布面：连打两 tag（如 0.1.0 → 0.1.1）实测检查→下载→安装→重启到新版本。

## 边界（不在本票）

- Windows 签名（无证书，map Out of scope；SmartScreen「更多信息→仍要运行」引导维持 acceptance.md §1）；Linux 打包形态变更；增量/差分更新（tauri v2 全量包）；版本降级；更新源镜像/自建服务器；macOS 原生菜单栏与 About 菜单（FR-13-04 另票）。

## 验收

- [ ] 活跃任务/待审批请求阻止安装和重启；待发队列须用户明确暂停并保留后才可更新，更新后保持暂停。

- [ ] secrets 配齐后 tag 构建：macOS dmg 下载后**双击直开**（无右键放行/xattr），`codesign -vv` 与 `spctl -a -vv` 通过。
- [ ] 应用内「检查更新」→ 下载 → 安装 → 重启后版本号更新（两连 tag 实测）；无新版时提示已最新。
- [ ] `update.enabled=false` 或未到间隔：启动不发起更新请求（NFR-11 核对：更新外联仅 GitHub Releases，列入 34 号票隐私清单）。
- [ ] 清空全部 APPLE_/TAURI_ secrets 手动 dispatch：三平台未签名构建照常绿、无 updater 产物步骤失败。
- [ ] latest.json 三平台条目 URL 逐一可下载，signature 与对应 .sig 内容一致。
- [ ] §3.2 update 配置域、§12.4 updater 实测结论回写完成。
