# CraftWave 工作交接（HandOff）

> 生成于 2026-10-27。上一会话以 wayfinder 地图驱动 M1 实现，本文件供新设备上的全新会话接续。
> 规则：本文件只做索引与状态快照，细节一律引用票文件/文档/提交，不复述。

## 一、必读顺序（新会话开场）

1. `AGENTS.md`——提交纪律、检查链、工具链（有本地补充再读 `AGENTS.local.md`）。
2. `docs/specs/m1-mvp/map.md`——wayfinder 地图：目标、决策索引（Decisions so far）、剩余雾区（Not yet specified）。
3. `docs/技术设计文档.md`——开发直接依据（P0 字段级）；附录 C 为未决问题清单。
4. 票目录 `docs/specs/m1-mvp/tasks/`——一票一文件，Answer 节是各票结论。

## 二、当前状态快照（main = `6eae3df`，已推送 origin）

- **地图进度**：01–13 号票全部关闭。四张 spike（S1/S2+S3/S4/S5/S6）+ 三张实现切片（storage/tools/providers）+ 核心链路（13 号）均已合并进 main。
- **测试基线**：Rust 178 单测 + 18 集成（1 例真钥匙串冒烟 `#[ignore]`）；前端 vitest 55 例；biome/tsc 全绿。
- **已通的能力**：`backend/` 正式 RunLoop（AgentRun 步进、64ms 帧节流、callId 同源映射、取消全链 <1s 实测）、`start_chat`/`cancel_run` 已注册、前端帧渲染 + runMetrics（C-9 口径）、storage 分段 JSONL + 会话树命令、五文件类工具 + 围栏/权限四档、CW schema v2 + keyring + 设置页骨架、IPC manifest 宏（命令↔TS 契约双向锁定）。
- **13 号票三处文档修订**（变更记录 v2.3）：§3.4 callId 改桥接派生映射、§3.2 RunConfig.tier 定 camelCase、§4.3 BackendEvent trait 暂缓。

## 三、下一步（按地图雾区顺序）

1. **工具系统接线票**：command/plan/ask/suggest 工具 + 审批门 + 工具卡片 UI（S1/S2+S3 已钉路线，backend 挂起机制层已就绪）。
2. **持久化剩余**：index.json、run 生命周期落盘点（§7.3 时机已钉，承载面未做）、分支/编辑/重生成 UI、崩溃恢复。
3. **多会话与项目** → **桌面体验**（三栏/托盘/快捷键）→ **打包验收**（C-8 安全复判在此毕业成票）。
4. 小项：rate 的 TextDelta 自算、steer/followUp、错误分类 §11.3、ts-rs 引入（C-2 后半）。

## 四、工作方式（本图约定）

- **wayfinder 模式**：一票一会话；动工先 `Status: claimed`；完成写 `## Answer` + `resolved` + 地图 Decisions 追加一行；新决策进雾区按 tracker 约定立票。
- **并行 worktree 模式**（本日实证有效）：`git worktree add ../<name> -b <branch> main` + 后台子代理各领一票；合并前先在 scratch 分支试合并并跑全链；多代理改同一文档时各自只动自己章节/行。
- **grilling 票**（HITL）留在主会话与用户做，子代理不得代答。

## 五、环境与坑（新设备必看）

- **前端检查从仓库根跑**：`pnpm typecheck/test/lint` 定义在根 package.json（lint 只在根，Biome 在根 devDependencies）；在 `ui/` 下跑 `pnpm lint` 会命中 PATH 上无关同名工具。
- 检查链：`src-tauri/` 下 `cargo fmt --check && cargo build && cargo clippy --all-targets -- -D warnings && cargo test`；根下前端三项。`MACOSX_DEPLOYMENT_TARGET=13.0` 已在 `.cargo/config.toml [env]`。
- 工具链：`rust-toolchain.toml` 钉 1.97.1（rig MSRV）；crates 走 rsproxy 镜像；GitHub 抖动时命令前缀 `https_proxy=http://127.0.0.1:7890`。
- `sha2` 全仓统一 **0.11**（storage/tools/providers 三方已验证兼容），勿回 0.10。
- 测试不进源码文件：Rust 用 `*_test.rs` / `tests/`，前端用 `*.test.ts(x)`。
- 提交：Conventional Commits 中文主题、按路径精确 add（禁 `add -A`）、禁 `--no-verify`、重构与功能分开；代码合并需用户点头，tracker/文档记账类提交按既有实践直提 main。

## 六、建议技能（新会话按需调用）

- `wayfinder`——以地图继续找路/开工票（Work through the map 模式，传 map 路径）。
- `implement` / `implement-spec`——实现票执行。
- `grilling` + `domain-modeling`——决策票（HITL）。
- `research`——外部调研票。
- `handoff`——下次交接（注意：本次按用户要求写到了仓库根 `HandOff.md`，技能默认是系统临时目录）。

## 七、敏感信息

无。API Key 走系统钥匙串（keyring trait + mock），仓库与配置文件不含明文密钥。
