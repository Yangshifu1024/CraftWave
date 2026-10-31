# 07 Spike 结论与依赖 pin 决策

Status: resolved
Type: task
Blocked by: 02, 03, 04, 05

## 目标

汇总阶段 0 全部结论，形成阶段 1/2 的开工依据；落依赖策略决策。

## 步骤

1. 汇总：02（托盘/热键集成形态）、03（性能基线数字）、04（IME 结论与坑）、05（部署目标结论）；
2. 依赖策略决策（二选一，写明理由）：
   - A：gpui-component pin rev（Wake 同款，组件库旧但经过同型验证）；
   - B：gpui-kit 0.7（pin GPUI 版本并 re-export，生态演进方向，API 面可能有差异）；
   - 混合亦可（如组件库 pin 旧 rev + 系统通知等新能力单独评估），但每项依赖都必须 pin；
3. `gpui` zed rev 定版；
4. 列出阶段 2 开工前的接口三件套清单（engine API 面 / 帧快照类型 / 主题 token 表），作为阶段 2.A 的输入；
5. 结论回写 spec.md「决策与背景」节（若有修订）。

## 验收

- 一页结论：性能数字、集成形态、pin 表、部署目标、阶段 2 输入清单；
- spec.md 相应节同步更新。

## Comments

### 完成结论（2026-10-31）——阶段 0 全部收口，零失败项

**Answer**：阶段 0 七票全部 resolved，无 go/no-go 失败项。**依赖决策定稿 + 阶段 1/2 开工依据齐备。**

#### 依赖策略决策（定稿）

**gpui-kit 门面（crates.io），不碰 zed 仓库**——比预设的 A/B 二选一更好的第三选项：

```toml
gpui-kit = "0.8.1"     # 门面：gpui-pre-0.3.10 家族 + gpui-base/component 0.8.1
gpui-fps = "0.8.1"     # 官方性能 HUD（03 号票验收工具）
tray-icon = "0.21"     # 02 号票
global-hotkey = "0.7"  # 02 号票
```

- **不依赖 `gpui = "0.2.2"` 直接版本**——它与 gpui-kit 内部的 gpui-pre 家族是两个实例，trait 互不相认（踩过：混用编译期即炸）；
- 版本语义化 + Cargo.lock 锁定，Wake「gpui 浮动跟 zed main」的供应链风险不存在；
- Wake 那套「pin zed rev + pin 组件库 git rev」的回退组合**不需要**——API 面核对全部通过（TextView::markdown / AutoScroll / List / Textarea / TitleBar / Notification / 主题 token）；
- 升级纪律不变：版本号变更走显式提交。

#### 各票结论一览

| 票 | 结论 |
|---|---|
| 01 工程搭建 | crates.io 门面路线；本机无完整 Xcode 可开发（gpui-pre 无需 build 期 metal）；编译增量 1-2s、全量分钟级 |
| 02 托盘+热键 | tray-icon + global-hotkey 与 GPUI 事件循环共存实测通过（50ms 轮询桥 + 关窗重建） |
| 03 性能基线 | 3000 条混合消息：FRAME 5.5-12.6ms、GPU <2%、MEM ~130MB；用户实测「性能没问题」；渲染三坑记录（flex-row 默认 / list 高度 Auto / gpui_kit::init） |
| 04 IME | 全流程通过；Textarea + submit_on_enter + subscribe_in 定为 composer 规范 |
| 05 部署目标 | 维持 13.0（无 nsh 的 14+ 约束）；CI 预编译路径留阶段 5 确认 |
| 06 解耦预演 | 无需 trait——剥 command 壳 + 挪 AppState + 剥 backup.rs，阶段 1.3 一天内 |

#### 阶段 2 开工输入（接口三件套，2.A 落地）

1. engine API 面：会话生命周期 / 帧流（64ms 节流聚合进 Rust 侧）/ 树持久化；
2. 帧快照 Rust 类型（含 `Msg::UserInput` 一类消息形态的正式化）；
3. 主题 token 表：UX 方案 A 部分档位 → ActiveTheme 语义色 + `Zpx` 缩放（04 号票已验证 token 同源机制）。

#### Spike 工程资产

`~/Code/craftwave-spike/`（launchctl 托管进程已停）：main.rs（托盘/热键/菜单骨架）+ chat_harness.rs（消息流/流式/composer/统计）——阶段 2 的骨架直接从这里长出来。
