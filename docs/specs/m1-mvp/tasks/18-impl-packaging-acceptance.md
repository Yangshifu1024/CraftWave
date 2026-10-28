# 18. 实现：三平台打包与验收剧本

Type: task
Status: claimed

## Question

把 M1 出口准则的打包关落地（FR-16-01）：三平台安装包矩阵可产出、CI 打包流水线实跑验证、验收剧本成文——地图最后一站「1 周真实使用验收」以本票交付的产物与剧本执行。

范围：

1. **macOS universal（FR-16-01 明确 x64+arm64）**：build.yml macOS job 补双架构 target（rustup target add x86_64-apple-darwin aarch64-apple-darwin）+ `tauri build --target universal-apple-darwin`；产物收集路径适配 `target/universal-apple-darwin/release/bundle/`。
2. **本地 macOS 打包实证**：universal dmg 本地产出、可安装、应用可启动（签名/公证不做，FR-16-02 属 P1；未签名引导写进验收剧本）。
3. **CI 实跑验证**：build.yml workflow_dispatch 触发三平台全绿，产物收集步骤在三个平台都能命中文件（Windows nsis、Linux deb+AppImage）。
4. **验收剧本**：`docs/specs/m1-mvp/acceptance.md`——安装引导（未签名三平台口径）、真实项目使用要求、记录模板、NFR-06 零丢失核查点（强杀/重启恢复）、NFR-01/02/09/12 验收口径（NFR-09 按 v2.5 用户决策：明文 0600，PRD 原文钥匙串口径已被替代）。
5. 版本与标识核对：version 0.1.0 / identifier app.craftwave.desktop 维持。

不做（记录边界）：签名与公证（P1，FR-16-02）、自动更新（P1，FR-16-03）、体积预算硬门（P2，FR-16-05，顺手记录实测体积）、rpm 产物（FR-16-01 未要求）。

验收：本地 universal dmg 可安装启动；build.yml dispatch 三平台绿；剧本入库；检查链全绿。

## Answer
