//! `edit` 工具（FileWrite，§8.4 算法）：oldText/newText 唯一匹配替换，
//! version? 乐观锁，三级匹配绝不猜，结果含 lineChanges{add,del}；
//! EOL 保持；落盘一律 UTF-8（C-10：原非 UTF-8 文件编辑后转 UTF-8 为接受副作用）。

use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::safety::tier::Capability;
use crate::tools::write::atomic_write_utf8;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};
use crate::util::encoding::decode_text;
use crate::util::errors::ToolError;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditArgs {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    /// sha256(原文件字节) 前 16 hex；不匹配 → E_EDIT_VERSION_STALE
    /// （仍可唯一匹配则降级为警告执行）。
    pub version: Option<String>,
}

/// 匹配结论：字节区间（L1/L2）或行区间（L3），或失败。
enum EditMatch {
    ByteRange {
        start: usize,
        len: usize,
        level: &'static str,
    },
    LineRange {
        start: usize,
        end: usize,
    },
    NoMatch,
    Ambiguous,
}

pub fn run(ctx: &ToolsContext, args: &EditArgs) -> ToolOutcome {
    if args.old_text.is_empty() {
        return ToolOutcome::failure(
            ToolError::invalid_arg("oldText 不能为空"),
            "编辑失败：oldText 为空",
        );
    }
    let resolved = match ctx.guard().and_then(|g| g.resolve(&args.path)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    let bytes = match std::fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => return ToolOutcome::from(ToolError::from(e)),
    };
    let actual_version = version_hex(&bytes);
    let stale = args.version.as_ref().is_some_and(|v| *v != actual_version);

    // 共用解码器（C-10）+ EOL 检测。归一为 LF 域做匹配与替换，写回前还原。
    let decoded = decode_text(&bytes);
    let has_crlf = decoded.text.contains("\r\n");
    let file_norm = decoded.text.replace("\r\n", "\n");
    let old_norm = args.old_text.replace("\r\n", "\n");
    let new_norm = args.new_text.replace("\r\n", "\n");

    let m = match_three_levels(&file_norm, &old_norm);
    let replaced = match &m {
        EditMatch::NoMatch => {
            let err = if stale {
                ToolError::edit_version_stale(format!(
                    "版本过期（当前 {actual_version}）且 oldText 无法唯一匹配：{}",
                    args.path
                ))
            } else {
                ToolError::edit_no_match(format!("oldText 未命中：{}", args.path))
            };
            return ToolOutcome::failure(err, format!("编辑失败：未找到匹配内容（{}）", args.path));
        }
        EditMatch::Ambiguous => {
            let err = if stale {
                ToolError::edit_version_stale(format!(
                    "版本过期（当前 {actual_version}）且 oldText 存在多处命中：{}",
                    args.path
                ))
            } else {
                ToolError::edit_ambiguous(format!(
                    "oldText 命中多处，请补充更多上下文：{}",
                    args.path
                ))
            };
            return ToolOutcome::failure(err, format!("编辑失败：匹配不唯一（{}）", args.path));
        }
        EditMatch::ByteRange { start, len, .. } => {
            let mut next = file_norm.clone();
            next.replace_range(*start..*start + *len, &new_norm);
            next
        }
        EditMatch::LineRange { start, end } => {
            let file_lines: Vec<&str> = file_norm.lines().collect();
            let new_lines: Vec<&str> = new_norm.lines().collect();
            let mut parts: Vec<&str> = Vec::with_capacity(file_lines.len());
            parts.extend_from_slice(&file_lines[..*start]);
            parts.extend_from_slice(&new_lines);
            parts.extend_from_slice(&file_lines[*end..]);
            let mut joined = parts.join("\n");
            // 文件以换行结尾且替换触底时保持尾换行。
            if file_norm.ends_with('\n') && *end >= file_lines.len() && !new_lines.is_empty() {
                joined.push('\n');
            }
            joined
        }
    };

    // EOL 保持：原文件 CRLF 则写回前还原。
    let final_text = if has_crlf {
        replaced.replace('\n', "\r\n")
    } else {
        replaced
    };
    if let Err(e) = atomic_write_utf8(&resolved, final_text.as_bytes()) {
        return ToolOutcome::from(e);
    }

    let (add, del) = line_changes(&old_norm, &new_norm);
    let level = match &m {
        EditMatch::ByteRange { level, .. } => *level,
        EditMatch::LineRange { .. } => "whitespace-normalized",
        EditMatch::NoMatch | EditMatch::Ambiguous => unreachable!("失败分支已提前返回"),
    };
    let mut data = json!({
        "path": args.path,
        "lineChanges": { "add": add, "del": del },
        "version": version_hex(final_text.as_bytes()),
        "matchLevel": level,
    });
    if stale {
        data["warning"] = json!(format!(
            "version 与当前文件不符（实际 {actual_version}），因 oldText 可唯一匹配而降级执行"
        ));
    }
    let mut outcome = ToolOutcome::success(
        data.clone(),
        format!("编辑 {}：+{add}/-{del} 行", args.path),
    )
    .with_detail(data);
    if stale {
        outcome.ui_summary = format!("编辑 {}：+{add}/-{del} 行（版本过期已降级执行）", args.path);
    }
    outcome
}

/// 三级匹配（§8.4 步骤 3）：每级仅接受唯一命中，绝不猜。
/// 某级 0 命中 → 降级；≥2 命中 → AMBIGUOUS（更宽的下一级只会更多命中）。
fn match_three_levels(file: &str, old: &str) -> EditMatch {
    // L1 精确匹配。
    match find_unique(file, old) {
        UniqueHit::One(start) => {
            return EditMatch::ByteRange {
                start,
                len: old.len(),
                level: "exact",
            };
        }
        UniqueHit::Many => return EditMatch::Ambiguous,
        UniqueHit::None => {}
    }
    // L2 整块缩进平移。
    match indent_shift_match(file, old) {
        IndentMatch::One { start, len } => {
            return EditMatch::ByteRange {
                start,
                len,
                level: "indent-shift",
            };
        }
        IndentMatch::Ambiguous => return EditMatch::Ambiguous,
        IndentMatch::None => {}
    }
    // L3 逐行首尾空白归一。
    line_normalized_match(file, old)
}

enum UniqueHit {
    None,
    One(usize),
    Many,
}

/// 非重叠出现计数；空 needle 恒 None（上游已拒空，防御）。
fn find_unique(haystack: &str, needle: &str) -> UniqueHit {
    if needle.is_empty() {
        return UniqueHit::None;
    }
    let mut it = haystack.match_indices(needle);
    let Some((first, _)) = it.next() else {
        return UniqueHit::None;
    };
    if it.next().is_some() {
        return UniqueHit::Many;
    }
    UniqueHit::One(first)
}

enum IndentMatch {
    None,
    One { start: usize, len: usize },
    Ambiguous,
}

/// L2：old 整体去掉公共缩进后，试每种缩进回填；收集全部命中（§8.4 L2）。
/// old 以换行结尾时把该换行并入替换区间（文件中必须确实存在）。
fn indent_shift_match(file: &str, old: &str) -> IndentMatch {
    let old_lines: Vec<&str> = old.lines().collect();
    let core = strip_common_indent(&old_lines);
    if core.iter().all(|l| l.trim().is_empty()) {
        return IndentMatch::None;
    }
    // 候选缩进：常见步长 + 文中实际出现的前导空白（去重）。
    let mut candidates: Vec<String> = ["", " ", "  ", "   ", "    ", "\t", "      ", "        "]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    for line in file.lines() {
        let indent = &line[..line.len() - line.trim_start().len()];
        if !indent.is_empty() && indent.len() <= 32 && !candidates.iter().any(|c| c == indent) {
            candidates.push(indent.to_string());
        }
    }
    let trailing_newline = old.ends_with('\n');
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for indent in &candidates {
        let candidate: String = core
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    (*l).to_string()
                } else {
                    format!("{indent}{l}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        match find_unique(file, &candidate) {
            UniqueHit::None => {}
            UniqueHit::Many => return IndentMatch::Ambiguous,
            UniqueHit::One(start) => {
                let mut len = candidate.len();
                if trailing_newline {
                    if !file[start + len..].starts_with('\n') {
                        continue; // 尾换行不存在，该命中无效。
                    }
                    len += 1;
                }
                if !hits.iter().any(|(s, _)| *s == start) {
                    hits.push((start, len));
                }
            }
        }
    }
    match hits.as_slice() {
        [] => IndentMatch::None,
        [(start, len)] => IndentMatch::One {
            start: *start,
            len: *len,
        },
        _ => IndentMatch::Ambiguous,
    }
}

/// L3：old 与全文均按行首尾空白归一后做连续块匹配，命中映射回全文行区间。
fn line_normalized_match(file: &str, old: &str) -> EditMatch {
    let pattern: Vec<&str> = old.lines().map(str::trim).collect();
    let file_lines: Vec<&str> = file.lines().collect();
    if pattern.is_empty() || pattern.len() > file_lines.len() {
        return EditMatch::NoMatch;
    }
    let mut hits: Vec<usize> = Vec::new();
    for start in 0..=(file_lines.len() - pattern.len()) {
        if file_lines[start..start + pattern.len()]
            .iter()
            .map(|l| l.trim())
            .eq(pattern.iter().copied())
        {
            hits.push(start);
        }
    }
    match hits.as_slice() {
        [] => EditMatch::NoMatch,
        [start] => EditMatch::LineRange {
            start: *start,
            end: start + pattern.len(),
        },
        _ => EditMatch::Ambiguous,
    }
}

/// strip 各非空行的最长公共前导空白（空行剥为空）。
/// 前导空白均为 ASCII，字节截取落在字符边界上。
fn strip_common_indent<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                ""
            } else {
                &l[min_indent..]
            }
        })
        .collect()
}

/// lineChanges 口径：del/add 分别为 old/new 的行数（lines().count()，
/// 末尾有无换行不影响计数），与替换区间边界解耦，可确定性测试。
fn line_changes(old: &str, new: &str) -> (u64, u64) {
    let count = |s: &str| {
        if s.is_empty() {
            0
        } else {
            s.lines().count() as u64
        }
    };
    (count(new), count(old))
}

/// sha256(文件字节) 前 16 hex（§8.4 version 口径）。
fn version_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "edit",
        description: "修改工作区内已有文件：用 oldText 定位唯一片段并替换为 newText。三级匹配（精确→缩进平移→逐行空白归一），多命中拒绝；version（读文件时返回的版本指纹）可做乐观锁。EOL 风格保持，落盘一律 UTF-8。".to_string(),
        capability: Capability::FileWrite,
        params_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "工作区内目标路径" },
                "oldText": { "type": "string", "description": "待替换的原文（须唯一命中）" },
                "newText": { "type": "string", "description": "替换后的文本（空串即删除）" },
                "version": { "type": "string", "description": "sha256 前 16 hex 乐观锁；不匹配时报版本过期（可唯一匹配则降级警告执行）" }
            },
            "required": ["path", "oldText", "newText"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: false,
            destructive_hint: true,
            idempotent_hint: false,
            open_world_hint: false,
        },
    }
}

#[cfg(test)]
#[path = "edit_test.rs"]
mod edit_test;
