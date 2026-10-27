# CraftWave 工作交接（HandOff）

> 生成于 2026-10-28（同日二次更新）。14 号票合并后追加两项用户决策优化：废除 keyring（Key 明文存 config.json + 0600）与 dev 模式原始错误输出（技术设计文档 v2.5）。
> 规则：本文件只做索引与状态快照，细节一律引用票文件/文档/提交，不复述。

## 一、必读顺序（新会话开场）

1. `AGENTS.md`——提交纪律、检查链、工具链（有本地补充再读 `AGENTS.local.md`）。
2. `docs/specs/m1-mvp/map.md`——wayfinder 地图：目标、决策索引（Decisions so far）、剩余雾区（Not yet specified）。
3. `docs/技术设计文档.md`——开发直接依据（P0 字段级）；附录 C 为未决问题清单。
4. 票目录 `docs/specs/m1-mvp/tasks/`——一票一文件，Answer 节是各票结论。

## 二、当前状态快照（main = `35a7dd8`，本地领先 origin 4 个提交未推送）

- **地图进度**：01–14 号票全部关闭。四张 spike（S1/S2+S3/S4/S5/S6）+ 三张实现切片（storage/tools/providers）+ 核心链路（13 号）+ 工具系统接线（14 号）均已合并进 main。
- **测试基线**：Rust 233 单测/集成（lib 209 + 集成 24，另 1 例真钥匙串冒烟 `#[ignore]`）；前端 vitest 75 例；biome/tsc 全绿。测试方法命名已全仓英文化（纪律见五）。
- **已通的能力**：在 13 号（backend RunLoop + 帧渲染 + 取消全链）之上，14 号新增——注册表 8 工具（command/plan/ask 接入，suggest 为 run 尾后台小调用不开放模型）；审批门生产化（矩阵×围栏×计划门×签名放行，**批级预判零执行挂起** + DriveState 延续恢复 + deny 补帧 + autoConfirm 300s）；ask 协议（AskSlot/resolve_ask，挂起与问卷双路径共用）；事件面 5 键（EventSink 抽象：ask:opened/closed、plan:update、run:suggestions、tokens:update）；`approve_plan` 命令（§3.3 26c）；前端正式 toolkit（参数与 Rust ToolSpec 经 manifest `toolSpecs` 双向锁定）+ 7 卡片 + AskPanel + suggest ghost + sessionUsage 事件校正。
- **同日追加（v2.5 用户决策）**：① **keyring 已废除**——API Key 明文存 config.json（ProviderConfig.apiKey；credentialRef/credentialCount 已删），config.json 落盘/加载统一收紧 0600，intent.json 协议移除，keyring crate 依赖删除（§9.4 改写）；② dev 模式原始错误输出：前端 DEV 条件 console（start_chat reject + runEnded error 帧）、Rust debug_assertions eprintln（send_error_ended）。另修复：lib.rs 曾漏 manage RunManager 导致 GUI start_chat 报 "state not managed"（headless 测试盲区，已补）。
- **14 号票顺带修正两处 13 号缺陷**（测试钉死）：挂起重放重复执行（改批级预判）；跨轮桥接 callId 撞车（派生 id 加 turn 前缀）。文档修订 v2.4 六处。

## 三、下一步（按地图雾区顺序）

1. **持久化剩余票**：index.json、run 生命周期落盘（§7.3 时机已钉）、toolres 边车与工具卡详细数据（#29）、分支/编辑/重生成 UI、崩溃恢复。
2. **多会话与项目** → **桌面体验**（三栏/托盘/快捷键/折叠分组 §10.4）→ **打包验收**（C-8 安全复判在此毕业成票）。
3. 小项：rate 的 TextDelta 自算、steer/followUp、错误分类 §11.3、ts-rs 引入（C-2 后半）。

## 四、工作方式（本图约定）

- **wayfinder 模式**：一票一会话；动工先 `Status: claimed`；完成写 `## Answer` + `resolved` + 地图 Decisions 追加一行；新决策进雾区按 tracker 约定立票。
- **并行 worktree 模式**（实证有效）：`git worktree add ../<name> -b <branch> main` + 后台子代理各领一票；合并前先在 scratch 分支试合并并跑全链；多代理改同一文档时各自只动自己章节/行。
- **grilling 票**（HITL）留在主会话与用户做，子代理不得代答。

## 五、环境与坑（新设备必看）

- **前端检查从仓库根跑**：`pnpm typecheck/test/lint` 定义在根 package.json（lint 只在根，Biome 在根 devDependencies）；在 `ui/` 下跑 `pnpm lint` 会命中 PATH 上无关同名工具。
- 检查链：`src-tauri/` 下 `cargo fmt --check && cargo build && cargo clippy --all-targets -- -D warnings && cargo test`；根下前端三项。`MACOSX_DEPLOYMENT_TARGET=13.0` 已在 `.cargo/config.toml [env]`。
- 工具链：`rust-toolchain.toml` 钉 1.97.1（rig MSRV）；crates 走 rsproxy 镜像；GitHub 抖动时命令前缀 `https_proxy=http://127.0.0.1:7890`。
- **测试方法命名一律英文**（snake_case，2026-10-27 用户指令，全仓已统一）；断言消息保持中文。
- **rig 非流式坑**：`Model::call`（单次补全，suggest 用）期待 `chat.completion` JSON，mock_openai 已支持流式/非流式两形态——新写后台小调用测试直接复用。
- 新依赖：`anstream`（ANSI 清洗，Rust）、`@types/json-schema`（ui devDep，toolkit 参数类型）；`sha2` 全仓统一 **0.11**，勿回 0.10。
- 测试不进源码文件：Rust 用 `*_test.rs` / `tests/`，前端用 `*.test.ts(x)`。
- 提交：Conventional Commits 中文主题、按路径精确 add（禁 `add -A`）、禁 `--no-verify`、重构与功能分开；代码合并需用户点头，tracker/文档记账类提交按既有实践直提 main。

## 六、建议技能（新会话按需调用）

- `wayfinder`——以地图继续找路/开工票（Work through the map 模式，传 map 路径）。
- `implement` / `implement-spec`——实现票执行。
- `grilling` + `domain-modeling`——决策票（HITL）。
- `research`——外部调研票。
- `handoff`——下次交接（注意：本次仍按用户要求写到仓库根 `HandOff.md`，技能默认是系统临时目录）。

## 七、敏感信息

无。API Key 走系统钥匙串（keyring trait + mock），仓库与配置文件不含明文密钥。
