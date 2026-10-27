//! 测试辅助：临时工作区 + ToolsContext。仅 `cargo test` 编译。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::tools::ToolsContext;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 临时项目目录 + 数据目录，各测试互不串扰。
pub struct TestWorkspace {
    pub project: tempfile::TempDir,
    pub data: tempfile::TempDir,
}

impl Default for TestWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl TestWorkspace {
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
