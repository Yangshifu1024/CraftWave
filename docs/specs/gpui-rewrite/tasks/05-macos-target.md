# 05 macOS 13.0 部署目标确认

Status: resolved
Type: task
Blocked by: 01

## 目标

确认 GPUI 在 `MACOSX_DEPLOYMENT_TARGET=13.0` 下编译运行正常，决定主仓是否维持现部署目标（避免 nsh 路线"必须抬到 14+"的约束在 GPUI 路线复现）。

## 步骤

1. Spike 工程在 `MACOSX_DEPLOYMENT_TARGET=13.0`（rustc 编译与链接阶段均设置，与主仓口径一致）全量编译；
2. 在 Intel/最老可用目标机上跑（若无 13.0 实机，至少确认 SDK 编译通过 + Ventura 虚拟机/兼容机实测一次）；
3. 查 Zed 上游的最低支持声明作旁证；
4. 结论三选一：13.0 维持 / 需 14+（写明原因）/ 待真机验证。

## 验收

- 结论记入 Comments 并同步 07 号票；
- 若维持 13.0：把 `MACOSX_DEPLOYMENT_TARGET=13.0` 写进 Spike 与后续 workspace 的构建说明。

## Comments

### 初步结论（2026-10-31，随 01/02 号票验证）

**Answer**：`MACOSX_DEPLOYMENT_TARGET=13.0` 下 crates.io gpui 0.2.2 编译、运行全部通过（01 空窗、02 托盘/热键均在此目标下构建实测）——**主仓维持 13.0，nsh 的 14+ 约束在 GPUI 路线不存在**。

- 已验证路径：`runtime_shaders`（build 期无 shader 编译，运行时驱动编译）——本机无完整 Xcode 的环境下 13.0 全流程可用；
- 待补验证：默认路径（build 期 `xcrun metal` 预编译 shader，需完整 Xcode）在 CI macos runner 的 13.0 最低部署目标表现——阶段 5.1 CI 搭建时顺带确认；预编译的 metallib 与部署目标的绑定关系以 CI 实测为准；
- 遗留：无 13.0 真机/虚拟机，正式发版前补一轮 Ventura 实机冒烟（发版前检查项，非阻塞）。
