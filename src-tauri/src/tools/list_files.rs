//! `list_files` 工具（ReadOnly，§8.3）：树形清单，上限 500 条目，
//! respectGitignore 默认 true（ignore crate）。

use serde::Deserialize;
use serde_json::json;

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};

/// 条目上限（§8.3）。
pub const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFilesArgs {
    /// 缺省为项目主目录。
    pub path: Option<String>,
    /// 最大深度（root 本身不计）。缺省不限。
    pub depth: Option<u32>,
    /// 默认 true。
    pub respect_gitignore: Option<bool>,
}

pub fn run(ctx: &ToolsContext, args: &ListFilesArgs) -> ToolOutcome {
    let root_display = args.path.clone().unwrap_or_else(|| ".".to_string());
    let root = match ctx.guard().and_then(|g| g.resolve(&root_display)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    if !root.is_dir() {
        return crate::util::errors::ToolError::invalid_arg(format!(
            "目标不是目录：{root_display}"
        ))
        .into();
    }

    let respect = args.respect_gitignore.unwrap_or(true);
    let mut builder = ignore::WalkBuilder::new(&root);
    builder.hidden(true); // 跳过隐藏文件
    builder.require_git(false); // 非 git 项目同样应用 .gitignore（工具语义）
    builder.git_ignore(respect);
    builder.git_global(respect);
    builder.git_exclude(respect);
    builder.ignore(respect);
    builder.parents(respect);
    if let Some(d) = args.depth {
        builder.max_depth(Some(d as usize));
    }
    builder.filter_entry(|entry| entry.file_name() != ".git");

    // 收集（深度优先）后按路径排序，保证确定性；达 500 截断。
    let mut entries: Vec<(usize, String, bool)> = Vec::new(); // (depth, 相对路径, is_dir)
    let mut truncated = false;
    for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 {
            continue; // root 本身不出现在清单里
        }
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let rel = match entry.path().strip_prefix(&root) {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => continue,
        };
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        entries.push((entry.depth(), rel, is_dir));
    }
    // 截断后如果恰好再无条目，truncated 可能虚标——重探一次不值得，
    // 达上限即认定截断（保守提示）。
    entries.sort_by(|a, b| a.1.cmp(&b.1));

    // 树形渲染：按层级缩进两空格，目录带 "/" 后缀（截断安全的缩进树）。
    let lines: Vec<String> = entries
        .iter()
        .map(|(depth, rel, is_dir)| {
            let name = rel.rsplit('/').next().unwrap_or(rel);
            let indent = "  ".repeat(depth.saturating_sub(1));
            if *is_dir {
                format!("{indent}{name}/")
            } else {
                format!("{indent}{name}")
            }
        })
        .collect();

    ToolOutcome::success(
        json!({
            "root": root_display,
            "count": lines.len(),
            "truncated": truncated,
            "entries": lines,
        }),
        format!(
            "列出 {} 共 {} 项{}",
            root_display,
            lines.len(),
            if truncated {
                "（已达上限截断）"
            } else {
                ""
            }
        ),
    )
}

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "list_files",
        description:
            "列出工作区目录内容为树形清单（上限 500 条目）。默认尊重 .gitignore；隐藏文件不列出。"
                .to_string(),
        capability: Capability::ReadOnly,
        params_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "目标目录，缺省为项目根" },
                "depth": { "type": "integer", "description": "最大深度" },
                "respectGitignore": { "type": "boolean", "description": "是否尊重 .gitignore，默认 true" }
            },
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
    }
}

#[cfg(test)]
#[path = "list_files_test.rs"]
mod list_files_test;
