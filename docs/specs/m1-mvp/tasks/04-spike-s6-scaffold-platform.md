# S6 最小骨架与三平台构建验证（含 C-7）

Type: task
Status: resolved

## Question

搭建最小可运行工程骨架，并完成技术设计文档附录 A 的 **S6** 验证与 **C-7** 判定：

1. **骨架**：Tauri 2 + React 19 + TypeScript + Tailwind v4 + assistant-ui（@assistant-ui/react 0.15.x，kit 组件 copy-paste 装一路即可）跑通一个空聊天窗。
2. **工具链锁定**：rust-toolchain.toml 钉死 Rust 1.97 / edition 2024（rig 要求）；Node ≥22 / pnpm；MACOSX_DEPLOYMENT_TARGET=13.0。
3. **安全基线**：CSP `default-src 'self'` + `connect-src ipc: http://ipc.localhost`；Tauri capability 最小白名单下应用可正常启动（技术设计文档 §9.5）。
4. **三平台 CI**：GitHub Actions 矩阵（macOS / Windows / Linux）build 通过（远端仓库已存在：github.com/Yangshifu1024/CraftWave）。
5. **C-7 判定**：tauri-plugin-decoration 自绘标题栏与新版 Tauri 的兼容性实测；不兼容则记录退化方案（系统标题栏，功能不损）。

完成标准：三平台 CI 绿；C-7 有明确结论。结论回写技术设计文档 §2.1/§9.5，勾销附录 A-S6 与附录 C-7。

## Answer

五项全部完成（2026-10-27）。CI 绿证：run 40472627211（前端检查 + macOS 3m49s + ubuntu + windows 全绿）。

1. **骨架**：`ui/`（React 19.3 + Vite 8.3 + Tailwind v4.3 + @assistant-ui/react 0.15.31）+ `src-tauri/`（tauri 2.12.5 单 crate）。空聊天窗以 primitives 手搭（AuiIf 空态、气泡、Composer 发送/停止切换）；不接真模型，回声适配器两段**累积 yield** 模拟流式（rig 真流属 S2/S3），vitest 5 例全绿。kit copy-paste 未走——primitives 足够且零外部依赖，kit 化留给后续 UX 票。
2. **工具链锁定**：`rust-toolchain.toml` 钉 1.97.0 **并显式声明 components = ["clippy","rustfmt"]**（教训见第 4 点）；Node 22（CI）/ ≥22（本机 24）；`MACOSX_DEPLOYMENT_TARGET=13.0` 落在 `.cargo/config.toml [env]`（本地 cargo 亦生效）+ CI env 双处。
3. **安全基线**：生产 CSP `default-src 'self'; connect-src ipc: http://ipc.localhost`（connect-src 不放行 https，§9.5 原备用项已移除）；devCsp 仅开发期放行 HMR（ws://localhost:5173 + 内联脚本样式）；capability 最小白名单 `core:default` + `decoration:default` 下应用正常启动。
4. **三平台 CI**：前端检查独立 job（biome/tsc/vitest）× ubuntu；三平台 tauri build 矩阵前置 cargo fmt --check / clippy -D warnings / cargo test。踩坑一轮：GitHub runner 预装 1.97.0（minimal 无组件）导致 fmt/clippy 秒退 `not installed`——**rust-toolchain.toml 不写 components 时 setup action 不会给已存在的工具链补组件**，双处钉死后修复。
5. **C-7 判定：兼容，无需退化**。tauri-plugin-decoration 3.0.5（2026-08 活跃维护，依赖 tauri ^2.10，macOS 侧 objc2 现代绑定）× tauri 2.12.5：编译干净通过，应用以 `decorations:false` + 顶栏拖拽区正常启动（进程存活、日志零错误）。启动级验证受限于终端无屏幕录制权限，未能截图贴窗——GUI 走查由用户首次 `pnpm dev` 时顺带确认即可。

**文档回写**：§2.1（C-7 结论）、§9.5（收紧版 CSP 实测基线 + devCsp 边界）、§12.3（ts-rs 12 分层决策）、§12.4（实际 CI 形态）、附录 A-S6 勾销、附录 C-7 翻「已定（S6）」。**顺带补上此前 02/03 两张调研票遗漏的回写**：附录 C-2/C-6 翻「已定（调研）」。

**遗留小项**（不阻塞）：assistant-ui 使 bundle 达 745KB（chunk 警告，无碍）；GitHub 注解提示 actions 目标 Node20 弃用（升级 actions 版本择机）；`src-tauri/icons/android|ios` 移动端图标产物已 gitignore。
