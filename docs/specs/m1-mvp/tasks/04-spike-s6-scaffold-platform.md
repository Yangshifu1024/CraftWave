# S6 最小骨架与三平台构建验证（含 C-7）

Type: task
Status: open

## Question

搭建最小可运行工程骨架，并完成技术设计文档附录 A 的 **S6** 验证与 **C-7** 判定：

1. **骨架**：Tauri 2 + React 19 + TypeScript + Tailwind v4 + assistant-ui（@assistant-ui/react 0.15.x，kit 组件 copy-paste 装一路即可）跑通一个空聊天窗。
2. **工具链锁定**：rust-toolchain.toml 钉死 Rust 1.97 / edition 2024（rig 要求）；Node ≥22 / pnpm；MACOSX_DEPLOYMENT_TARGET=13.0。
3. **安全基线**：CSP `default-src 'self'` + `connect-src ipc: http://ipc.localhost`；Tauri capability 最小白名单下应用可正常启动（技术设计文档 §9.5）。
4. **三平台 CI**：GitHub Actions 矩阵（macOS / Windows / Linux）build 通过（远端仓库已存在：github.com/Yangshifu1024/CraftWave）。
5. **C-7 判定**：tauri-plugin-decoration 自绘标题栏与新版 Tauri 的兼容性实测；不兼容则记录退化方案（系统标题栏，功能不损）。

完成标准：三平台 CI 绿；C-7 有明确结论。结论回写技术设计文档 §2.1/§9.5，勾销附录 A-S6 与附录 C-7。
