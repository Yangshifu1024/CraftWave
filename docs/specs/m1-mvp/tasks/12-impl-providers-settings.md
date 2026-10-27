# 实现：Provider 管理与设置切片（CW schema v2 + keyring + 设置页）

Type: task
Status: resolved

## Question

按《技术设计文档》§11（Provider 与设置）落地 M1 provider 管理切片：

1. providers 模块：CW schema v2 配置文件读写与 config 协调器（含并发写协调）、内置供应商预设清单、自定义 OpenAI 兼容端点（baseUrl 覆盖）。
2. keyring 提交协议：API Key 存取走系统钥匙串；keyring 交互抽象成 trait，测试用内存 mock，真实钥匙串冒烟测试标 `#[ignore]`（CI 无钥匙串服务）。
3. Tauri 命令：按 §3.2 命令表中 provider/settings 相关 P0 命令逐一定义并注册（以文档为准；M2 命令如 test_provider/Key 池不做）。
4. TS 契约：对应类型按 §12.3 现行方案手写 + manifest 校验（延续既有契约测试做法）。
5. 最小设置页：供应商列表、内置预设选择、自定义端点表单、Key 录入（写 keyring）——UI 骨架级可用即可，视觉/布局打磨属桌面体验阶段（三栏布局不在本票）。

边界：不含 M2 增量（Key 池轮换、quota 适配器、test_provider 连通性、OAuth）；不含运行中 run 的 provider 热切换语义（如 §11 有定义则按文档纸面实现，不确定则记录票 Answer）。

完成标准：schema roundtrip 与协调器逻辑 Rust 单测绿、keyring mock 协议测试绿、命令层可通、契约测试绿、检查链全过；如实现与 §11 有出入，Answer 记录并最小修订对应行，无出入则声明。

## Answer

**命令清单落地（§3.3 P0 provider/settings 域 4 个，全部注册进 invoke_handler）**：`get_config`（#6）、`save_config`（#7）、`save_provider`（#8，newKey → §9.4 协议）、`delete_provider`（#9，config 移除后同步删 keyring 账户）。命令层为薄壳，逻辑在 `providers::service`（可脱离 Tauri 单测）。M2 命令（test_provider、Key 池、quota、OAuth）未做，符合票面边界；`get/set_session_prefs`、`get/set_ui_state` 载体为会话/UI 状态域，归 storage/UI 状态实现票，不属于本切片。

**模块归属**：票面预留说明点——文档 §2.2/§7.6 把配置协调器钉在 storage，照办：`storage/config_store.rs` 持协调器（校验/归一化 → intent → 外部落子 → 原子写 → 缓存热替换 → 删意向），以泛型 `ConfigPayload` trait 解耦，`providers/types.rs` 的 AppConfig 实现该 trait（providers → storage 单向依赖，符合 §2.2 方向图）。其余（schema v2 类型、预设、keyring、编排）落 `providers/`。原子写在 `util/atomic.rs`（§7.4：临时文件同目录 + fsync + rename；Windows 走 std rename 的 MoveFileExW REPLACE_EXISTING 语义）。

**并发写协调**：M1 单进程 + 单实例（§7.4），协调器以进程内写锁串行化全部提交；读-改-写（save_provider/delete_provider）在锁内取当前配置（`commit_from_current`/`update`），并发测试验证不丢更新。跨进程 FileCoordinator 维持文档口径留 1.x。

**keyring 抽象与 mock**：`KeyringStore` trait（set/get/delete，delete 幂等屏蔽各平台 NoEntry 差异）；`MemoryKeyring` 内存实现（测试用，亦作语义参考）；`OsKeyring` 生产实现（keyring 4.2.0，默认 v1 feature = Apple keychain / Windows native / zbus-secret-service）。协议核心 `commit_keys`：envelope `{"generation","keys"}` 数组整体覆写、generation 递增（§11.2 结构就位，M1 单 Key）、写后读回 sha256 前 16 位指纹比对；intent.json 三字段 `{opId,providerId,keyFingerprint}` 不含明文；外部落子失败 → config 不变更、意向保留。真实钥匙串冒烟测试 `os_keyring_真实读写删冒烟` 标 `#[ignore]`（本地 `cargo test -- --ignored` 手动跑）。删除顺序取舍：先 config 后 keyring（反向会因 config 写失败留悬空凭据引用；孤儿账户无害），文档未钉死删除时序，此处为 M1 定案。

**依赖版本**：keyring 4.2.0（crates.io 当前最新稳定）、uuid 1（v4）、sha2 0.10、dev 侧 tempfile 3；前端新增 zustand 5.0.16 直接依赖（原为传递依赖，锁 `^5.0.16` 与 lockfile 现状一致）。

**TS 契约与 manifest**：`ui/src/ipc/types.ts` 手写镜像 §3.2 provider/settings 域类型；`ui/src/ipc/client.ts` 唯一 invoke 出口 + `COMMANDS` 注册表；Rust 侧 `host::commands::COMMANDS/FRAMES/EVENTS/TOOLS` 清单 → `export_ipc_manifest` 单测写 `target/ipc-manifest.json`；前端 `contract.test.ts` 双向扫描（client 注册命令 ⊆ manifest、provider/config 切片 4 命令全部接线、全源码无未知 invoke 字面量、frames 覆盖 §3.4 九帧）。注意：C-2 定案的 ts-rs 12 payload 类型生成**未在本票执行**——票面明确本票按"现行方案手写 + manifest 校验"，ts-rs 接管 payload 类型面留后续票，届时 manifest 的 argsType/retType 名单可由生成物精简。

**设置页**：`ui/src/settings/SettingsPage.tsx` 骨架级四要素齐全（供应商列表、内置预设选择、自定义 OpenAI 兼容端点空表单、Key 密码框走 save_provider），`stores/settings.ts` zustand 草稿模型（§10.1），App 挂"设置"按钮全屏覆盖。预设清单 Rust `providers/presets.rs` 为权威（6 家：OpenAI/Anthropic/DeepSeek/Kimi/智谱/OpenRouter，含建议模型），前端 `settings/presets.ts` 持同源副本（§3.3 无 list_presets 命令，M1 不加命令；slug 对齐、漂移风险由两侧测试钉住，收敛方案留设置页打磨票）。外观/安全/诊断组、脏状态三选、意向残留 UI 提示（`ConfigStore::pending_intent` 已暴露 API）属桌面体验/后续票。

**与 §11/§9.4/§7.6 的出入及处理**：
1. §3.3 #8 与 §7.6 两处把钥匙串协议引用写作"§9.6"（§9.6 实为通知与后台运行），系笔误，已最小修订为 §9.4——协议本体无出入。
2. §11.1 模型解析到 rig Model 的构造（ResolvedModel→rig Model、apiFormat 方言）不在本票（属 backend/run 时切片）；`ApiFormat::official_base_url()` 已就位供其使用。
3. 运行中 provider 热切换：§11.1 即答案——客户端按 run 构建、无长连接池状态，activeModelId 改后下次 run 生效，无需额外语义；未发现文档其他热切换定义。
4. 其余（账户命名、generation、指纹读回、intent 生命周期、credentialCount 回填、Key 上限 10、save_config 返回归一化结果）逐条与文档一致，无出入。

**测试清单**：Rust 51 例全绿、其中 1 例 `#[ignore]`（真钥匙串冒烟）——新增 45：util/atomic 3、providers types 8、presets 3、keyring 9（含冒烟）、service 8、storage 协调器 6、manifest 1，另有既有 spike frames 6 + mock_chat 5 + tests/ 集成 5（lib 46 + 集成 5 = 51）。前端 vitest 39 例（契约 4 + settings store 8 + 既有 27）全绿；`cargo fmt --check`/`cargo build`/`cargo clippy --all-targets -- -D warnings`/`cargo test` 与 `pnpm typecheck`/`pnpm test`/`pnpm lint` 七项全过。

**备注**：仓库无 GLOSSARY.md（票面所列前置文件之一不存在，未阻塞）；AGENTS.local.md 亦不存在。
