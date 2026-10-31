//! `edit` 工具（FileWrite）：oldText/newText 唯一匹配替换，
//! version? 乐观锁，三级匹配绝不猜，结果含 lineChanges{add,del}；
//! EOL 保持；落盘一律 UTF-8（原非 UTF-8 文件编辑后转 UTF-8 为接受副作用）。

use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::safety::tier::Capability;
use crate::tools::write::atomic_write_utf8;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};
use crate::util::encoding::decode_text;
use crate::util::errors::ToolError;

/// edit 工具参数：定位用 oldText + 替换用 newText，可选 version 乐观锁。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditArgs {
    /// 工作区内目标文件路径。
    pub path: String,
    /// 待替换的原文片段（必须在文件中唯一命中，否则拒绝）。
    pub old_text: String,
    /// 替换后的文本（空串即删除该片段）。
    pub new_text: String,
    /// sha256(原文件字节) 前 16 hex；不匹配 → E_EDIT_VERSION_STALE
    /// （仍可唯一匹配则降级为警告执行）。
    pub version: Option<String>,
}

/// 匹配结论：字节区间（L1/L2）或行区间（L3），或失败。
/// `pub(crate)`：同批 edit 合并预演复用（区间以 `byte_range` 统一归算）。
pub(crate) enum EditMatch {
    /// L1/L2 命中：在归一化文本上的字节区间 [start, start+len)。
    ByteRange {
        start: usize,
        len: usize,
        /// 命中的匹配级别名（"exact" / "indent-shift"，写入结果的 matchLevel）。
        level: &'static str,
    },
    /// L3 命中：行区间 [start, end)（行号从 0 计，含 start 不含 end）。
    LineRange { start: usize, end: usize },
    /// 全部级别都 0 命中。
    NoMatch,
    /// 某级别命中不止一处（绝不猜，直接拒绝）。
    Ambiguous,
}

// EditMatch 的辅助方法：行区间与字节区间的统一归算（同批合并预演用）。
impl EditMatch {
    /// 归算为归一化 LF 域上的替换字节区间 [start, start+len) 与替换文本
    /// （含 L3 触底时的尾换行语义，与单文件 edit::run 的行为逐一对齐）。
    /// `line_offsets` 为文本各行起始字节偏移（末位 = 文本长度，触底用）。
    pub(crate) fn byte_range(
        &self,
        file_norm: &str,
        line_offsets: &[usize],
        new_norm: &str,
    ) -> Option<(usize, usize, String)> {
        let line_count = line_offsets.len() - 1;
        match self {
            EditMatch::ByteRange { start, len, .. } => Some((*start, *len, new_norm.to_owned())),
            EditMatch::LineRange { start, end } => {
                // 区间 = 第 start 行首字节到第 end 行首字节（触底 = 文本末尾）；
                // 触底且文件以换行结尾且替换非空 → 替换文本补尾换行（与
                // edit::run 的 L3 分支一致：保持文件尾换行）。
                let start_byte = line_offsets.get(*start).copied()?;
                let end_byte = if *end < line_count {
                    line_offsets.get(*end).copied()?
                } else {
                    file_norm.len()
                };
                let replacement =
                    if *end >= line_count && file_norm.ends_with('\n') && !new_norm.is_empty() {
                        format!("{new_norm}\n")
                    } else {
                        new_norm.to_owned()
                    };
                Some((start_byte, end_byte - start_byte, replacement))
            }
            EditMatch::NoMatch | EditMatch::Ambiguous => None,
        }
    }
}

/// 执行 edit：读文件 → 校验 version → 三级匹配 → 替换 → 原子落盘，
/// 返回含 lineChanges 与新 version 的结果。
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
    // version 指纹与当前文件不符 → 记为"过期"；不立即报错，
    // 若 oldText 仍能唯一匹配则降级为警告继续执行（见下方 warning 分支）。
    let stale = args.version.as_ref().is_some_and(|v| *v != actual_version);

    // 共用解码器 + EOL 检测。归一为 LF 域做匹配与替换，写回前还原。
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
    // 写后完整性自检：重读比对哈希 + 严格 UTF-8 + EOL 保持实证。
    // 失败 → ok=false 回喂 E_INTERNAL。
    if let Err(f) = crate::tools::postcheck::verify_written(
        &resolved,
        final_text.as_bytes(),
        crate::tools::postcheck::EolExpect::Preserve(has_crlf),
    ) {
        return ToolOutcome::failure(
            crate::tools::postcheck::postcheck_error(&args.path, &f),
            format!("编辑已落盘但自检失败：{}", args.path),
        );
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

/// 同批同文件多个 edit 的合并结论：预演槽位。
/// 一个可合并成员 = 在归一化 LF 域上的替换区间 + 替换文本 + 结果面字段。
struct GroupEditSlot {
    /// 成员在组内的下标（结果按源序对位回填）。
    index: usize,
    /// 替换区间起始字节。
    start: usize,
    /// 替换区间长度（字节）。
    len: usize,
    /// 替换文本（LF 归一域）。
    replacement: String,
    /// 匹配级别名（结果 matchLevel）。
    level: &'static str,
    /// version 指纹与原文件不符（降级警告执行）。
    stale: bool,
    /// 行变更 { add, del }。
    add: u64,
    del: u64,
}

/// 归一化文本的行起始字节偏移表（末位 = 文本长度；"a\nb\n" → [0, 2, 4]）。
/// L3 行区间归算字节区间的基准。
fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            offsets.push(i + 1);
        }
    }
    offsets
}

/// 同批同文件多个 edit 的合并原子提交（预演 + 一次 rename 生效）。
///
/// 流程：读原文件 → 全员基于同一原内容预演（三级匹配 → 字节区间）→
/// 区间重叠的后续成员拒绝（E_INVALID_ARG，源序前者保留）→ 接受成员按
/// start 降序应用 → EOL 还原 → 备份原文件到同目录 tmp → 单次原子落盘
/// （`write` 闭包，生产为 tmp+fsync+rename）→ 失败恢复备份（回滚整组）。
///
/// 返回与 `members` 等长的（成员下标, 结果）序列，顺序 = 成员源序：
/// - 任一成员匹配失败（NO_MATCH/AMBIGUOUS）→ 整组不合并，全部成员按源序
///   逐个独立执行 [`edit::run`](Self::run)（各出各的结果，含匹配错误）；
/// - 预演通过的接受成员共用一次落盘：结果各自带 lineChanges 与同一最终
///   version（及 stale 警告）；落盘失败 → 全组失败（文件保持原样）。
///
/// `write` 为落盘闭包（测试注入失败用；生产传 [`atomic_write_utf8`] 包装）。
pub(crate) fn apply_edit_group(
    ctx: &ToolsContext,
    path: &str,
    members: &[EditArgs],
    write: impl Fn(&std::path::Path, &[u8]) -> Result<(), ToolError>,
) -> Vec<ToolOutcome> {
    // 成员源序下标（结果对位）。
    let mut results: Vec<Option<ToolOutcome>> = vec![None; members.len()];
    let resolved = match ctx.guard().and_then(|g| g.resolve(path)) {
        Ok(p) => p,
        Err(e) => {
            // 路径越界等守卫失败：全员同一错误（正常执行路径也会如此）。
            return members
                .iter()
                .map(|_| ToolOutcome::from(e.clone()))
                .collect();
        }
    };
    let bytes = match std::fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            let err = ToolOutcome::from(ToolError::from(e));
            return members.iter().map(|_| err.clone()).collect();
        }
    };
    let actual_version = version_hex(&bytes);
    let decoded = decode_text(&bytes);
    let has_crlf = decoded.text.contains("\r\n");
    let file_norm = decoded.text.replace("\r\n", "\n");
    let offsets = line_start_offsets(&file_norm);

    // —— 预演：全员基于同一原内容算替换区间 ——
    let mut slots: Vec<GroupEditSlot> = Vec::new();
    let mut demote = false; // 任一成员匹配失败 → 整组降级独立执行
    for (index, args) in members.iter().enumerate() {
        let old_norm = args.old_text.replace("\r\n", "\n");
        let new_norm = args.new_text.replace("\r\n", "\n");
        let m = match_three_levels(&file_norm, &old_norm);
        match m.byte_range(&file_norm, &offsets, &new_norm) {
            Some((start, len, replacement)) => {
                let (add, del) = line_changes(&old_norm, &new_norm);
                let level = match &m {
                    EditMatch::ByteRange { level, .. } => *level,
                    EditMatch::LineRange { .. } => "whitespace-normalized",
                    EditMatch::NoMatch | EditMatch::Ambiguous => unreachable!("byte_range 已过滤"),
                };
                slots.push(GroupEditSlot {
                    index,
                    start,
                    len,
                    replacement,
                    level,
                    stale: args.version.as_ref().is_some_and(|v| *v != actual_version),
                    add,
                    del,
                });
            }
            // 匹配失败无法预演：降级标记（独立执行路径回喂具体错误）。
            None => demote = true,
        }
    }
    if demote {
        // 整组降级：按源序逐个独立执行（后者基于前者修改后的文件，M1 语义）。
        return members.iter().map(|args| run(ctx, args)).collect();
    }

    // —— 重叠检查：按 start 排序后相邻比较，重叠者（源序后者）拒绝 ——
    slots.sort_by_key(|s| (s.start, s.index));
    let mut accepted: Vec<GroupEditSlot> = Vec::new();
    for slot in slots {
        if let Some(prev) = accepted.last()
            && slot.start < prev.start + prev.len
        {
            let outcome = ToolOutcome::failure(
                ToolError::invalid_arg(format!(
                    "与同批同文件的前一个 edit 替换区间重叠（本区间 {start}–{end} 字节，前者 {pstart}–{pend} 字节），已拒绝执行：{path}；请将该编辑与前一个合并后重发",
                    start = slot.start,
                    end = slot.start + slot.len,
                    pstart = prev.start,
                    pend = prev.start + prev.len,
                )),
                format!("编辑拒绝：与同批 edit 区间重叠（{path}）"),
            );
            results[slot.index] = Some(outcome);
            continue;
        }
        accepted.push(slot);
    }

    // —— 应用：按 start 降序在归一化文本上替换（防止偏移失效）——
    accepted.sort_by_key(|s| std::cmp::Reverse(s.start));
    let mut replaced = file_norm.clone();
    for slot in &accepted {
        replaced.replace_range(slot.start..slot.start + slot.len, &slot.replacement);
    }
    // EOL 保持：原文件 CRLF 则写回前还原。
    let final_text = if has_crlf {
        replaced.replace('\n', "\r\n")
    } else {
        replaced
    };
    let final_bytes = final_text.into_bytes();
    let final_version = version_hex(&final_bytes);

    // —— 批次事务：备份 → 原子落盘 → 失败回滚整组 ——
    let commit = || -> Result<(), ToolError> {
        let dir = resolved
            .parent()
            .ok_or_else(|| ToolError::invalid_arg("目标路径无父目录"))?;
        let backup = dir.join(format!(
            ".cw-bak-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&backup, &bytes).map_err(ToolError::from)?;
        match write(&resolved, &final_bytes) {
            Ok(()) => {
                let _ = std::fs::remove_file(&backup);
                Ok(())
            }
            Err(e) => {
                // 回滚整组：恢复备份内容（rename 原子性下通常原文件未动，
                // 恢复为幂等兜底），清理备份，向上传原错误。
                let _ = std::fs::write(&resolved, &bytes);
                let _ = std::fs::remove_file(&backup);
                Err(e)
            }
        }
    };
    match commit() {
        Ok(()) => {
            // 写后完整性自检：合并提交是一次 rename 生效，
            // 校验挂在其出口——重读比对哈希 + 严格 UTF-8 + EOL 保持。
            // 失败 → 整组接受成员以失败收口回喂（E_INTERNAL，含阶段）。
            let postcheck = crate::tools::postcheck::verify_written(
                &resolved,
                &final_bytes,
                crate::tools::postcheck::EolExpect::Preserve(has_crlf),
            )
            .err()
            .map(|f| crate::tools::postcheck::postcheck_error(path, &f));
            match postcheck {
                None => {
                    for slot in accepted {
                        let mut data = json!({
                            "path": path,
                            "lineChanges": { "add": slot.add, "del": slot.del },
                            "version": final_version,
                            "matchLevel": slot.level,
                        });
                        let mut summary = format!("编辑 {}：+{}/-{} 行", path, slot.add, slot.del);
                        if slot.stale {
                            data["warning"] = json!(format!(
                                "version 与当前文件不符（实际 {actual_version}），因 oldText 可唯一匹配而降级执行"
                            ));
                            summary.push_str("（版本过期已降级执行）");
                        }
                        let outcome = ToolOutcome::success(data.clone(), summary).with_detail(data);
                        results[slot.index] = Some(outcome);
                    }
                }
                Some(err) => {
                    let summary = format!("编辑已落盘但自检失败（{}）：{}", path, err.message);
                    let outcome = ToolOutcome::failure(err, summary);
                    for slot in accepted {
                        results[slot.index] = Some(outcome.clone());
                    }
                }
            }
        }
        Err(e) => {
            // 落盘失败：整组回滚——文件保持原样，全部接受成员以失败收口。
            let summary = format!("编辑失败：同批合并提交已回滚（{}）", e.message);
            let outcome = ToolOutcome::failure(e, summary);
            for slot in accepted {
                results[slot.index] = Some(outcome.clone());
            }
        }
    }
    results
        .into_iter()
        .enumerate()
        .map(|(i, r)| r.unwrap_or_else(|| panic!("成员 {i} 结果未回填")))
        .collect()
}

/// 三级匹配：每级仅接受唯一命中，绝不猜。
/// 某级 0 命中 → 降级；≥2 命中 → AMBIGUOUS（更宽的下一级只会更多命中）。
/// `pub(crate)`：同批 edit 合并预演复用同一匹配实现。
pub(crate) fn match_three_levels(file: &str, old: &str) -> EditMatch {
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

/// 出现次数结论：0 命中 / 唯一命中（带起始字节偏移）/ 多处命中。
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

/// L2 缩进平移匹配结论：唯一命中（带区间）/ 无命中 / 多处命中。
enum IndentMatch {
    None,
    One { start: usize, len: usize },
    Ambiguous,
}

/// L2：old 整体去掉公共缩进后，试每种缩进回填；收集全部命中。
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
pub(crate) fn line_changes(old: &str, new: &str) -> (u64, u64) {
    let count = |s: &str| {
        if s.is_empty() {
            0
        } else {
            s.lines().count() as u64
        }
    };
    (count(new), count(old))
}

/// sha256(文件字节) 前 16 hex（version 乐观锁指纹口径）。
pub(crate) fn version_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

/// edit 工具规格（能力 FileWrite：plan 档过滤、confirm_each 档审批）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "edit".to_string(),
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
        origin: crate::tools::ToolOrigin::Static,
    }
}

#[cfg(test)]
#[path = "edit_test.rs"]
mod edit_test;
