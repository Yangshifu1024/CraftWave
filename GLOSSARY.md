# CraftWave 术语表（GLOSSARY）

> 单一语境。术语按主题分组，只收领域概念，不写实现细节；实现口径见《技术设计文档》。

## 会话与工作区

- **会话（Session）**：一次持续的人机协作上下文，含消息树、run 历史、用量累计。分**项目会话**与**临时会话**两形态。
- **项目会话（Project Session）**：挂在一个已注册项目下的会话；工作区边界以项目主目录为基准。
- **临时会话（Ad-hoc Session）**：不挂项目的会话（随手聊/试）；两根同目录 = 数据目录下 `workspace/` 子目录。
- **项目主目录（Project Home）**：项目注册表登记的代码仓库根目录；agent 相对路径的解析基准。
- **数据目录（Data Directory）**：`~/.craftwave/`——配置、会话存储、用量状态等应用内部数据的家。**不在 agent 可见面内**（C-8 复判定案）。
- **workspace 子目录（Workspace Subdirectory）**：`~/.craftwave/workspace/`——数据目录下专供 agent 文件工具读写的唯一区域。
- **roots（会话可见根集）**：agent 文件工具与 @ 补全允许触及的目录集合 = 项目主目录 + workspace 子目录。越界一律 `E_PATH_OUTSIDE_ROOTS`；full_access 档也不解除。

## 安全与审批

- **权限四档（Permission Tiers）**：plan（计划档）/ confirm_each（逐次审批）/ auto_edit（自动写）/ full_access（完全放开）。
- **围栏（Fence）**：命令文本上的灾难级硬拦层（黑名单正则命中即 Block，任何档位不豁免）。
- **审批门（Approval Gate）**：按权限档矩阵决定写/网络类动作是否需要用户确认的运行时机制。
- **command 旁路（Command Bypass）**：已声明风险——shell 命令可读 agent 可见面外的文件（如数据目录内配置），M1 靠审批档人工把关兜底，full_access 档用户自担。
