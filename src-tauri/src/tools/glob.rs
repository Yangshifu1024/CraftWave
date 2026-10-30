//! `glob` 工具（ReadOnly，§8.3 M2 表）：按文件名模式检索工作区文件，
//! globset 匹配（支持 `**` 递归语义），ignore::WalkBuilder 遍历（与
//! list_files 同源 ignore 栈：尊重 .gitignore、跳过隐藏文件与 .git），
//! 上限 500 条目截断。

use serde::Deserialize;
use serde_json::json;

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};
use crate::util::errors::ToolError;

/// 条目上限（§8.3 glob 行「上限 500」，与 list_files 同口径）。
pub const MAX_ENTRIES: usize = 500;

/// glob 工具参数：匹配模式 + 可选起点目录。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobArgs {
    /// 文件名模式，如 `*.rs`、`**/*.rs`、`src/**/*.{ts,tsx}`（globset 语义）。
    pub pattern: String,
    /// 遍历起点，缺省为项目主目录。
    pub path: Option<String>,
}

/// 执行 glob：解析起点 → 编译模式 → 遍历目录树（尊重 .gitignore、跳过
/// 隐藏文件与 .git）→ 按路径匹配，上限 500 截断，结果按路径排序。
pub fn run(ctx: &ToolsContext, args: &GlobArgs) -> ToolOutcome {
    if args.pattern.trim().is_empty() {
        return ToolError::invalid_arg("pattern 不能为空：如 \"*.rs\" 或 \"**/*.rs\"").into();
    }
    let root_display = args.path.clone().unwrap_or_else(|| ".".to_string());
    let root = match ctx.guard().and_then(|g| g.resolve(&root_display)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    if !root.is_dir() {
        return ToolError::invalid_arg(format!("目标不是目录：{root_display}")).into();
    }
    // globset 匹配器：默认 `*` 不跨目录分隔符为 gitwildmatch 变体——这里用
    // 默认 Glob（literal_separator=false，`*` 可跨段），`*.rs` 与 `**/*.rs`
    // 都能递归命中任意深度的同名后缀文件（对「找文件」场景更宽容）。
    let matcher = match globset::Glob::new(&args.pattern) {
        Ok(g) => g.compile_matcher(),
        Err(e) => {
            return ToolError::invalid_arg(format!("pattern 无效：{e}")).into();
        }
    };

    // 遍历配置与 list_files 完全同源（§8.3 工具语义一致）：
    // hidden(true) 跳隐藏文件、require_git(false) 非 git 项目也应用
    // .gitignore、filter_entry 排除 .git 目录本身。
    let mut builder = ignore::WalkBuilder::new(&root);
    builder.hidden(true);
    builder.require_git(false);
    builder.git_ignore(true);
    builder.git_global(true);
    builder.git_exclude(true);
    builder.ignore(true);
    builder.parents(true);
    builder.filter_entry(|entry| entry.file_name() != ".git");

    // 收集匹配条目（文件与目录都参与匹配，kind 区分），按路径排序保证
    // 确定性；达 500 截断（保守提示，同 list_files 口径）。
    let mut entries: Vec<(String, &'static str)> = Vec::new();
    let mut truncated = false;
    for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 {
            continue; // 起点目录本身不进结果
        }
        let rel = match entry.path().strip_prefix(&root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if !matcher.is_match(&rel) {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let kind = if entry.file_type().is_some_and(|t| t.is_dir()) {
            "dir"
        } else {
            "file"
        };
        entries.push((rel, kind));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let paths: Vec<serde_json::Value> = entries
        .iter()
        .map(|(path, kind)| json!({ "path": path, "kind": kind }))
        .collect();
    ToolOutcome::success(
        json!({
            "pattern": args.pattern,
            "root": root_display,
            "count": entries.len(),
            "truncated": truncated,
            "paths": paths,
        }),
        format!(
            "glob {}：命中 {} 项{}",
            args.pattern,
            entries.len(),
            if truncated {
                "（已达上限 500 截断）"
            } else {
                ""
            }
        ),
    )
}

/// glob 工具规格（能力 ReadOnly，全档放行，§8.5）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "glob".to_string(),
        description:
            "按文件名模式在工作区检索文件（如 \"*.rs\"、\"**/*.rs\"、\"src/**/*.{ts,tsx}\"），\
             递归匹配任意深度，默认尊重 .gitignore，隐藏文件不列出；上限 500 条按路径排序返回。\
             找文件用本工具，搜内容用 grep。"
                .to_string(),
        capability: Capability::ReadOnly,
        params_schema: json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "文件名模式，如 \"*.rs\" 或 \"**/*.rs\"" },
                "path": { "type": "string", "description": "遍历起点，缺省项目根" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        origin: crate::tools::ToolOrigin::Static,
    }
}

#[cfg(test)]
#[path = "glob_test.rs"]
mod glob_test;
