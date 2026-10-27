//! `grep` 工具（ReadOnly，§8.3）：内嵌 ripgrep 生态
//! （grep-regex + grep-searcher + ignore），命中行上下文 ±0，maxResults 截断。

use serde::Deserialize;
use serde_json::json;

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};
use crate::util::errors::ToolError;

/// maxResults 缺省值（§8.3 未定默认，实现自定，票 Answer 已记录）。
pub const DEFAULT_MAX_RESULTS: usize = 100;
/// maxResults 硬上限。
pub const MAX_RESULTS_CAP: usize = 1000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepArgs {
    pub pattern: String,
    /// 搜索起点，缺省项目主目录。
    pub path: Option<String>,
    /// glob 过滤（如 "*.rs"）。
    pub glob: Option<String>,
    pub max_results: Option<usize>,
}

pub fn run(ctx: &ToolsContext, args: &GrepArgs) -> ToolOutcome {
    let root_display = args.path.clone().unwrap_or_else(|| ".".to_string());
    let root = match ctx.guard().and_then(|g| g.resolve(&root_display)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    let max = args
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, MAX_RESULTS_CAP);

    let matcher = match grep_regex::RegexMatcherBuilder::new().build(&args.pattern) {
        Ok(m) => m,
        Err(e) => {
            return ToolError::invalid_arg(format!("正则无效：{e}")).into();
        }
    };

    let mut builder = ignore::WalkBuilder::new(&root);
    builder.hidden(true);
    builder.require_git(false); // 非 git 项目同样应用 .gitignore（工具语义）
    builder.filter_entry(|entry| entry.file_name() != ".git");
    if let Some(glob) = &args.glob {
        let mut overrides = ignore::overrides::OverrideBuilder::new(&root);
        if let Err(e) = overrides.add(glob) {
            return ToolError::invalid_arg(format!("glob 无效：{e}")).into();
        }
        match overrides.build() {
            Ok(o) => {
                builder.overrides(o);
            }
            Err(e) => {
                return ToolError::invalid_arg(format!("glob 无效：{e}")).into();
            }
        }
    }

    let mut matches: Vec<String> = Vec::new();
    let mut truncated = false;
    let root_clone = root.clone();
    'outer: for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let display = path
            .strip_prefix(&root_clone)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        let mut sink = grep_searcher::sinks::UTF8(|_line_no, line| {
            if matches.len() >= max {
                truncated = true;
                return Ok(false);
            }
            let text = line.trim_end_matches(['\n', '\r']);
            matches.push(format!("{display}:{text}"));
            if matches.len() >= max {
                truncated = true;
                return Ok(false); // 停止当前文件
            }
            Ok(true)
        });
        let mut searcher = grep_searcher::SearcherBuilder::new()
            .binary_detection(grep_searcher::BinaryDetection::quit(0u8))
            .build();
        // 单文件失败（权限/IO）跳过，不中断整体。
        let _ = searcher.search_path(&matcher, path, &mut sink);
        if matches.len() >= max {
            break 'outer;
        }
    }

    ToolOutcome::success(
        json!({
            "pattern": args.pattern,
            "matches": matches,
            "count": matches.len(),
            "truncated": truncated,
        }),
        format!(
            "grep {}：{} 处命中{}",
            args.pattern,
            matches.len(),
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
        name: "grep",
        description: "在工作区内用正则搜索文件内容（内嵌 ripgrep 语义），逐行返回 命中文件:行内容，默认尊重 .gitignore，maxResults 截断。".to_string(),
        capability: Capability::ReadOnly,
        params_schema: json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Rust regex 正则" },
                "path": { "type": "string", "description": "搜索起点，缺省项目根" },
                "glob": { "type": "string", "description": "文件名过滤，如 \"*.rs\"" },
                "maxResults": { "type": "integer", "description": "命中上限，默认 100，最大 1000" }
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
    }
}

#[cfg(test)]
#[path = "grep_test.rs"]
mod grep_test;
