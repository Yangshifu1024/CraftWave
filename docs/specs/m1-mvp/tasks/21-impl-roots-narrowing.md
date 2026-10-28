# 21. 实现：agent roots 收窄——数据目录退出可见面（C-8 定案落地）

Type: task
Status: claimed

## Question

把 20 号票（C-8 grilling，2026-10-28 定案）的 roots 收窄落地：agent 可见面从「项目主目录 + 数据目录」收窄为「项目主目录 + 数据目录下 `workspace/` 子目录」，存储内部文件（config.json/ui-state.json/sessions/ 等）退出模型文件工具与 @ 补全的可见面。**随 19 号票的 worktree 分支（feat/t19-platform-debts）实现**，合并时一并回来。

范围：

1. **roots 定义**（§9.1/PRD FR-05-06 已改口径）：PathGuard 构造处 roots 集合更新——数据目录根 → `<data_root>/workspace/`；首次使用 create_dir_all（chat.rs 既有习惯）。
2. **临时会话两根**（§6.2 口径）：`resolve_session_roots` 临时会话形态改「两根同目录 = `<data_root>/workspace/`」；项目会话不变（项目主目录 + workspace 子目录）。
3. **@ 补全连带**：search_workspace_paths 走同一 roots 解析自动收窄；相对路径口径（项目主目录下相对、其余绝对）不变，数据目录专属条目现指 workspace 子目录条目。
4. **测试**：workspace_test 既有断言更新（`resolve_roots_temp_session_uses_data_root` → 改名与口径）、safe_join/fence 相关测试口径核对、@ 补全测试补 workspace 子目录条目形态。

不做（记录边界）：command 工具的路径级检查（Q2 声明风险：审批档兜底 + full_access 自担；P1 的 FR-05-08 受保护区检查 §9.3 另行承接写向）；`~/.craftwave/workspace/` 的迁移逻辑（当前无用户数据，零包袱）。

验收：cargo 三件套 + 前端三项全绿；临时会话与项目会话下 read/write/edit/list/grep/@ 补全对 config.json 均不可达（E_PATH_OUTSIDE_ROOTS）；workspace 子目录内正常读写。
