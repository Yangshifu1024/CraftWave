# 01 Spike 工程搭建

Status: resolved
Type: task

## 目标

建立阶段 0 的独立 Spike 工程（不碰主仓），跑出第一个 GPUI 空窗，钉死初始依赖版本。

## 步骤

1. 在主仓外建独立目录（如 `~/Code/craftwave-spike/`），`cargo init`；
2. 添加依赖并 **pin rev**：
   - `gpui`：zed 仓库 git 依赖，pin 具体 rev（取一个近期稳定 tag/commit）；
   - `gpui-component`：pin `fd3bc2bbb8a2c4dfe268c1475682476ada54cd0c`（与 Wake 同款起步，features 按 03/04 号票需要加，起步含 `tree-sitter-languages`）；
   - `tray-icon`、`global-hotkey`（02 号票用）；
3. 写最小 main：`gpui::Application` + `open_window` + 一个空 div + 亮暗主题跟随；
4. `MACOSX_DEPLOYMENT_TARGET=13.0 cargo run` 实测；
5. 把选定的 rev 与理由写进 Cargo.toml 注释与 07 号票草稿。

## 验收

- `cargo run` 出空窗，亮暗跟随系统；
- 依赖 pin 成文（rev + 选择理由）；
- 编译时长记录一次（首次全量），作为后续工程预估参考。

## Comments

### 完成结论（2026-10-31）

**Answer**：Spike 工程就绪，空窗验证通过。工程位置 `~/Code/craftwave-spike/`（主仓外）。

#### 依赖路线重大修正（07 号票决策输入提前落地）

初版照搬 Wake 的 git 依赖 zed 写法，经用户质疑后查证：**gpui 已官方发布 crates.io**（0.2.2，2026-10-30 更新），gpui-kit 0.8.1 同在。已切换并验证：

```toml
gpui = { version = "0.2.2", features = ["runtime_shaders"] }
gpui-kit = "0.8.1"
```

- 「拉 zed 全家桶」问题消失：crates.io 拉压缩包，Cargo.lock 锁 976 个传递依赖（含 gpui-kit 全家）；
- crates.io 0.2.2 的 API 面与精读结论一致（`Application::new().run` / `open_window` / `WindowOptions` / `TitlebarOptions` / `Render` 全部可用）；
- Wake 那条「gpui 浮动跟 zed main、无版本锁」的最大供应链风险直接消失。

#### 本机环境发现：无完整 Xcode → runtime_shaders 绕过

- 本机 xcode-select 指向 CommandLineTools，**无 metal 编译工具**（`xcrun metal` 失败），gpui 默认 build 期编译 shader 会失败；
- 解法：开 `runtime_shaders` feature——build 期只拼接 shader 源码，运行时由 Metal 驱动编译，无需 Xcode（build.rs 177-195 双分支确认）；
- 代价：每次启动运行时编 shader（启动稍慢）；**CI 打包与正式发版走默认预编译路径（macos runner 自带完整 Xcode）**，此差异记入阶段 5 CI 注意项；
- 若后续 runtime_shaders 出现渲染异常，回退方案是本机安装完整 Xcode（需用户操作）。

#### 编译与运行数据

- 全量编译（crates.io 路线）：一次通过；增量重编（加 runtime_shaders）44.6s；
- 运行验证：进程存活、System Events 可见 GUI 进程、无 panic 输出；
- future-incompat 警告：block 0.1.6 / proc-macro-error2 2.0.1（上游依赖，不阻塞）。

#### 遗留

- gpui-kit 0.8.1 的 API 面核对（TextView::markdown / AutoScroll / List 是否与渲染规范一致）——03 号票实操时核对，对不上回退「gpui-component git fd3bc2b + 同期 zed rev」组合。
