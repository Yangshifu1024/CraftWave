# 21. 实现：agent roots 收窄——数据目录退出可见面（C-8 定案落地）

Type: task
Status: resolved

## Question

把 20 号票（C-8 grilling，2026-10-28 定案）的 roots 收窄落地：agent 可见面从「项目主目录 + 数据目录」收窄为「项目主目录 + 数据目录下 `workspace/` 子目录」，存储内部文件（config.json/ui-state.json/sessions/ 等）退出模型文件工具与 @ 补全的可见面。**随 19 号票的 worktree 分支（feat/t19-platform-debts）实现**，合并时一并回来。

范围：

1. **roots 定义**（§9.1/PRD FR-05-06 已改口径）：PathGuard 构造处 roots 集合更新——数据目录根 → `<data_root>/workspace/`；首次使用 create_dir_all（chat.rs 既有习惯）。
2. **临时会话两根**（§6.2 口径）：`resolve_session_roots` 临时会话形态改「两根同目录 = `<data_root>/workspace/`」；项目会话不变（项目主目录 + workspace 子目录）。
3. **@ 补全连带**：search_workspace_paths 走同一 roots 解析自动收窄；相对路径口径（项目主目录下相对、其余绝对）不变，数据目录专属条目现指 workspace 子目录条目。
4. **测试**：workspace_test 既有断言更新（`resolve_roots_temp_session_uses_data_root` → 改名与口径）、safe_join/fence 相关测试口径核对、@ 补全测试补 workspace 子目录条目形态。

不做（记录边界）：command 工具的路径级检查（Q2 声明风险：审批档兜底 + full_access 自担；P1 的 FR-05-08 受保护区检查 §9.3 另行承接写向）；`~/.craftwave/workspace/` 的迁移逻辑（当前无用户数据，零包袱）。

验收：cargo 三件套 + 前端三项全绿；临时会话与项目会话下 read/write/edit/list/grep/@ 补全对 config.json 均不可达（E_PATH_OUTSIDE_ROOTS）；workspace 子目录内正常读写。

## Answer

随 19 号票分支落地（2026-10-28，merge main 后实现）：

### 改动
- **收窄单一落点 = `resolve_session_roots`**（host/commands/workspace.rs）：
  - 项目会话两根 = 项目主目录 + `<data_root>/workspace/`；
  - 临时会话（含未知会话兜底）两根同目录 = `<data_root>/workspace/`。
  PathGuard 的两个构造处（`tools::ToolsContext::guard` 与
  `read_workspace_file_core`）与 @ 补全遍历的 roots 全部经此解析——收窄后
  config.json/ui-state.json/sessions/ 等存储内部文件同时退出模型文件工具与
  @ 补全可见面，无需各处分别改。
- **chat.rs ToolsContext 装配**：解构 `resolve_session_roots` 双根并两根
  `create_dir_all`（workspace 子目录首次使用即建，canonicalize 前置）；
  宿主侧边车（spill/logs，ExecutorEnv.data_root）仍挂 `AppState.store` 根
  不经本 roots——收窄的是 agent 可见面，不是存储布局。
- **read_workspace_file_core**：workspace 子目录同款首次使用即建。
- **注释口径同步**：safe_join.rs 模块头与 roots 字段注释、tools/mod.rs
  `ToolsContext::new` 契约注释（`data_root` 形参语义=「第二根」，生产传
  收窄后的 workspace 子目录，测试传临时目录）。safe_join/fence 既有测试
  核对：PathGuard 本身对 root 集合无语义假设（`multi_root_second_root_
  is_honored` 用任意目录），零改动即兼容。
- **@ 补全连带**：search_workspace_paths_core 走同一 roots 自动收窄；
  临时会话两根同目录由既有 visited 去重只扫一遍；workspace 专属条目绝对
  路径口径不变（临时会话下基准即 workspace，相对路径形态同旧数据目录）。

### 测试（workspace_test 7→9 例）
- 三例 roots 断言改新口径：项目会话第二根=workspace 子目录；
  `resolve_roots_temp_session_uses_data_root` 改名
  `resolve_roots_temp_session_two_roots_at_workspace_subdir` 并加「数据目录
  本体不在 roots」断言；未知会话兜底同步改。
- 搜索：workspace 子目录条目命中且绝对路径返回；数据目录根的文件
  （config.json 等）不进命中。
- 预览读取：新增不可达双口径断言——`../config.json` → E_PATH_OUTSIDE_ROOTS
  （越 workspace 出 roots）、裸名 `config.json` → E_NOT_FOUND（解析进
  workspace，不会命中数据目录根）；新增
  `read_preview_allows_workspace_subdir_file_in_project_session`（项目会话
  下 workspace 内绝对路径放行 + 数据目录本体绝对路径拒绝）——验收
  「workspace 子目录内正常读写」的读侧锚点。

### 不做（票面记录边界，未动）
- command 工具路径级检查（Q2 声明风险：审批档兜底 + full_access 自担）；
- `~/.craftwave/workspace/` 迁移逻辑（无存量用户数据）。

### 检查链
cargo fmt --check / build / clippy -D warnings / test（lib 242 + 集成全绿）
+ pnpm typecheck / test（111）/ lint 全绿；manifest 契约面零变化
（无命令/事件增删，contract.test.ts 无需改）。
