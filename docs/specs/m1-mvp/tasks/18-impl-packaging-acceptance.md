# 18. 实现：三平台打包与验收剧本

Type: task
Status: claimed

## Question

把 M1 出口准则的打包关落地（FR-16-01）：三平台安装包矩阵可产出、CI 打包流水线实跑验证、验收剧本成文——地图最后一站「1 周真实使用验收」以本票交付的产物与剧本执行。

范围：

1. **macOS 仅 arm64（2026-10-28 用户决策：不再支持 Intel）**：PRD FR-16-01 的 universal（x64+arm64）口径随之收窄——三平台打包命令统一 `tauri build`（主机架构），产物收集路径统一 `target/release/bundle/`；关闭票时把口径变更回写 PRD/设计文档。
2. **本地 macOS 打包实证**：arm64 dmg 本地产出、可安装、应用可启动（签名/公证不做，FR-16-02 属 P1；未签名引导写进验收剧本）。
3. **CI 实跑验证**：build.yml workflow_dispatch 触发三平台全绿，产物收集步骤在三个平台都能命中文件（Windows nsis、Linux deb+AppImage）。
4. **验收剧本**：`docs/specs/m1-mvp/acceptance.md`——安装引导（未签名三平台口径）、真实项目使用要求、记录模板、NFR-06 零丢失核查点（强杀/重启恢复）、NFR-01/02/09/12 验收口径（NFR-09 按 v2.5 用户决策：明文 0600，PRD 原文钥匙串口径已被替代）。
5. 版本与标识核对：version 0.1.0 / identifier app.craftwave.desktop 维持。

不做（记录边界）：签名与公证（P1，FR-16-02）、自动更新（P1，FR-16-03）、体积预算硬门（P2，FR-16-05，顺手记录实测体积）、rpm 产物（FR-16-01 未要求）。

验收：本地 arm64 dmg 可安装启动；build.yml dispatch 三平台绿；剧本入库；检查链全绿。

## Answer

本地打包验证全绿；CI dispatch 实跑待用户授权后执行（三平台绿即关闭本票）。

**决策（2026-10-28，用户）**：macOS 放弃 Intel——FR-16-01 的 universal（x64+arm64）口径收窄为 arm64-only（PRD 行已注记，设计文档 v2.8 回写）。CodeWave 对照结论：其依赖树同样含 aws-lc-sys（tauri/rustls 链路带入），「不需要交叉编译」的真因是**从未跨架构构建**（release.yml 显式 aarch64-apple-darwin，主机=目标）；universal 方案取消后 aws-lc-sys 交叉编译问题整体消失。

**打包产物（本地实证）**：`CraftWave_0.1.0_aarch64.dmg` 6.78 MiB + CraftWave.app 14.97 MiB（FR-16-05 预算 25MB 余量充足）；release 冷编译 2m40s；冒烟启动通过（二进制拉起 3s 被 kill，dyld 加载正常，未签名）。

**build.yml**：流水线结构无需改动——三平台统一 `pnpm build`（主机架构）、产物收集路径统一 `target/release/bundle/`，仅补一行 arm64-only 决策注记；HandOff 预想的「universal 双架构适配」不再需要。

**release 面 strip 坑（本票最重要发现，已回写设计文档 §12.4）**：`[profile.release] strip = true` 把 proc-macro dylib 的 LINKEDIT 剥出 mis-aligned（`dlopen: mis-aligned LINKEDIT string pool`），dyld 拒载、rustc 报 E0463 can't find crate（phf_macros/tauri-macros 均中招）。修复：`[profile.release.build-override] strip = false`。日常检查链全在 dev 面（strip 不生效）探不到，CI 同样只跑 dev 面——首次 release 构建才暴露，故此前零症状。

**顺手清理**：release 面暴露的 5 处 `unused variable: e`（v2.5 debug_assertions 条件日志在 release 下成死绑定）——toolres.rs / index_store.rs / run_persistence.rs ×2 / chat.rs，统一补 `#[cfg(not(debug_assertions))] let _ = e;`，release check warning 清零。

**检查链**：cargo fmt / clippy -D warnings / test（233 lib + 25 集成）+ cargo check --release + 前端 typecheck / vitest / Biome 全绿。

**剩余**：build.yml 与代码修复 commit → push → workflow_dispatch 三平台实跑（dispatch 只跑 main 上的流水线，须先合入）。
