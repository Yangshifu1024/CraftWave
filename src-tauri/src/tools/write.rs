//! `write` 工具（FileWrite，§8.3）：仅新建，已存在拒绝（E_INVALID_ARG）
//! 引导用 edit；落盘一律 UTF-8（C-10）。原子写：同目录 tmp + rename。

use serde::Deserialize;
use serde_json::json;

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};
use crate::util::errors::ToolError;

/// write 工具参数：目标路径 + 完整文件内容。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteArgs {
    /// 工作区内目标文件路径（仅新建，已存在会拒绝）。
    pub path: String,
    /// 完整文件内容（一律以 UTF-8 落盘）。
    pub content: String,
}

/// 执行 write：路径守卫校验 → 已存在拒绝（引导用 edit）→ 建父目录
/// → 原子写落盘。
pub fn run(ctx: &ToolsContext, args: &WriteArgs) -> ToolOutcome {
    let resolved = match ctx.guard().and_then(|g| g.resolve(&args.path)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    // 仅新建：目标已存在（含断链 symlink）即拒绝。
    if std::fs::symlink_metadata(&resolved).is_ok() {
        return ToolOutcome::failure(
            ToolError::invalid_arg(format!(
                "文件已存在：{}（用 edit 修改，不要覆盖）",
                args.path
            )),
            format!("文件已存在：{}", args.path),
        );
    }
    // 父目录不存在则创建（均已在 roots 内）。
    if let Some(parent) = resolved.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return ToolOutcome::from(ToolError::from(e));
    }
    match atomic_write_utf8(&resolved, args.content.as_bytes()) {
        Ok(()) => {}
        Err(e) => return ToolOutcome::from(e),
    }
    ToolOutcome::success(
        json!({
            "path": args.path,
            "bytesWritten": args.content.len(),
        }),
        format!(
            "新建文件 {}（{} 字节，UTF-8）",
            args.path,
            args.content.len()
        ),
    )
}

/// 原子写 UTF-8：同目录写 tmp 文件 + fsync + rename 覆盖。
/// write（仅新建）与 edit 共用。
pub(crate) fn atomic_write_utf8(target: &std::path::Path, bytes: &[u8]) -> Result<(), ToolError> {
    use std::io::Write;
    let dir = target
        .parent()
        .ok_or_else(|| ToolError::invalid_arg("目标路径无父目录"))?;
    let tmp = dir.join(format!(
        ".cw-tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let scope = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, target)?;
        Ok(())
    };
    match scope() {
        Ok(()) => Ok(()),
        Err(e) => {
            // 清理残留 tmp，失败不掩盖原错误。
            let _ = std::fs::remove_file(&tmp);
            Err(ToolError::from(e))
        }
    }
}

/// write 工具规格（能力 FileWrite：plan 档过滤、confirm_each 档审批）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "write",
        description: "在工作区内新建文件（仅新建，文件已存在时拒绝）。内容一律以 UTF-8 落盘。修改已有文件请用 edit。".to_string(),
        capability: Capability::FileWrite,
        params_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "工作区内目标路径" },
                "content": { "type": "string", "description": "完整文件内容（UTF-8）" }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: false,
            destructive_hint: true,
            idempotent_hint: true,
            open_world_hint: false,
        },
    }
}

#[cfg(test)]
#[path = "write_test.rs"]
mod write_test;
