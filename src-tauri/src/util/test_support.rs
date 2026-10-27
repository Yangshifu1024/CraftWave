//! 测试辅助：临时工作区 + ToolsContext。仅 `cargo test` 编译。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::tools::ToolsContext;

/// 全局自增序号：拼进每个测试的临时目录名前缀，并行测试也不重名。
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 临时项目目录 + 数据目录，各测试互不串扰。
pub struct TestWorkspace {
    /// 项目工作区目录（对应"用户的项目文件夹"），测试结束自动删除。
    pub project: tempfile::TempDir,
    /// 应用数据目录（对应 ~/.craftwave 的替身），测试结束自动删除。
    pub data: tempfile::TempDir,
}

/// 等价于 [`Self::new`]，便于按 Default 惯例构造。
impl Default for TestWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl TestWorkspace {
    /// 建两个带序号前缀的独立临时目录（项目 + 数据），并行测试互不串扰。
    pub fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            project: tempfile::Builder::new()
                .prefix(&format!("cw-proj-{id}-"))
                .tempdir()
                .expect("临时项目目录"),
            data: tempfile::Builder::new()
                .prefix(&format!("cw-data-{id}-"))
                .tempdir()
                .expect("临时数据目录"),
        }
    }

    /// 以本工作区的两个目录构造工具上下文（工具的路径操作都被圈在这两目录内）。
    pub fn ctx(&self) -> ToolsContext {
        ToolsContext::new(
            self.project.path().to_path_buf(),
            self.data.path().to_path_buf(),
        )
    }

    /// 项目目录内绝对路径。
    pub fn project_file(&self, rel: &str) -> PathBuf {
        self.project.path().join(rel)
    }
}
