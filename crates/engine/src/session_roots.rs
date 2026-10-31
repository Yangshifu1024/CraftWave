//! 会话 roots 解析（自 host/commands/workspace.rs 剥离：命令面与 run 装配共用）。

use std::fs;
use std::path::PathBuf;

use crate::app_state::AppState;

/// 会话 roots 集合（含授权目录）。
///
/// - 项目会话：两根 = 项目主目录 + `<data_root>/workspace/`；
/// - 临时会话（含未知会话兜底）：两根同目录 = `<data_root>/workspace/`；
/// - 第三集合 = SessionMeta 授权的外部目录（会话 roots 快照：
///   「始终允许」并入后随 index.json 持久化，会话内跨 run 生效、跨会话
///   不延续）。
///
/// 数据目录本体（config.json/sessions/ 等存储内部文件）退出模型文件工具
/// 与 @ 补全的可见面。目录不在这里落盘创建，由使用方 create_dir_all
/// 或按不存在口径报错。
#[derive(Debug, Clone)]
pub struct SessionRoots {
    /// 项目主目录（相对路径解析基准与首个 root；临时会话 = workspace 根）。
    pub project_root: PathBuf,
    /// 数据目录下的 workspace 子目录（第二根）。
    pub workspace_root: PathBuf,
    /// 已授权外部目录（canonical 绝对路径；授权记录按字符串保留，此处
    /// canonicalize 失败的条目跳过——PathGuard 对不存在的根会报错）。
    pub extra_dirs: Vec<PathBuf>,
}

impl SessionRoots {
    /// 全部根（PathGuard 构造序：项目主目录在前——相对路径解析基准）。
    pub fn all(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.project_root.clone(), self.workspace_root.clone()];
        roots.extend(self.extra_dirs.iter().cloned());
        roots
    }

    /// 初始两根（不含授权目录；insideRoots 判定口径）。
    pub fn core(&self) -> Vec<PathBuf> {
        vec![self.project_root.clone(), self.workspace_root.clone()]
    }
}

/// 会话 roots 解析（含授权目录；命令面与 run 装配共用）。
pub fn resolve_session_roots(app: &AppState, session_id: &str) -> SessionRoots {
    let workspace_root = app.store.root().join("workspace");
    let project_root = app
        .store
        .index()
        .get(session_id)
        .and_then(|meta| meta.project_id)
        .and_then(|pid| app.projects.home_of(&pid))
        .unwrap_or_else(|| workspace_root.clone());
    let extra_dirs = app
        .store
        .index()
        .get(session_id)
        .map(|meta| {
            meta.authorized_external_dirs
                .iter()
                .filter_map(|d| fs::canonicalize(d).ok())
                .collect()
        })
        .unwrap_or_default();
    SessionRoots {
        project_root,
        workspace_root,
        extra_dirs,
    }
}
