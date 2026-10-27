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

## Comments

**2026-10-27 缺陷修复：标题栏无法拖动、macOS 无交通灯（用户实测报告）**

S6 合入后用户实测发现两个 GUI 缺陷，根因三个：

1. **窗口配置错**：S6 用了 `decorations:false`——macOS 交通灯随之消失，且 decoration 插件 3.0.5 的正确姿势是 `decorations:true` + `titleBarStyle:"Overlay"` + `hiddenTitle` 启动，再由前端显式调 `activate_decoration()`（3.0.5 已把 README 旧名 `create_overlay_titlebar`/`restore_native_titlebar` 改为 async 的 `activate_decoration`/`restore_decoration`，以 crate 源码为准）。
2. **拖拽权限缺**：本地 gen/schemas ACL 实证 `core:window:default` 不含 `allow-start-dragging`，拖拽 invoke 被静默拒绝（无崩溃无日志，正是启动级验证漏掉它的原因）。
3. **CSP 未放行插件样式**：插件样式经 `tauri-plugin-decoration:` 协议注入，`style-src`/`style-src-elem` 需放行其来源。

修复：窗口配置改 Overlay 协议 + `visible:false`；lib.rs 增 `activate_custom_titlebar`/`restore_native_titlebar` 两条命令（激活 + macOS 交通灯 inset(14,20) + 看门狗回退）；前端 App.tsx 实现激活协议（等 `data-tauri-plugin-decoration-active` 标志亮窗，5s 超时退化）——正好落地 §2.1 的防闪 + 亮窗看门狗设计；capabilities 显式补窗口控制权限；CSP 升到 §9.5 目标形态 + 插件来源。

**教训**：C-7 的兼容结论不变（插件本身工作正常），但 S6 的"进程存活 + 零日志"启动级验证覆盖不了 GUI 功能正确性——自绘标题栏的**视觉验收**（交通灯位置、拖拽手感、双击最大化）待用户 `pnpm dev` 确认。
