//! `batch_read` 工具（ReadOnly）：一次读取多个文本文件。
//!
//! 设计要点：
//! - 文件数上限 ≤[`DEFAULT_BATCH_READ_LIMIT`]（缺省 20，可经
//!   `AppConfig.tools.batchReadLimit` 调整）；超限整批拒绝（E_INVALID_ARG）。
//! - 逐文件复用 read 的文本路径（util::encoding 解码、行窗口默认 2000 行），
//!   本工具不接受逐文件 offset/limit——批量调研场景要的是每个文件
//!   的开头窗口，精细翻页用单文件 read。
//! - **512KB 单响应上限跨全批次共享**：批内按源序逐文件扣减预算；预算
//!   截断的文件标注 truncated 与续读 nextOffset；预算耗尽后的文件不读取
//!   （error 说明「批响应预算耗尽」），单文件失败不影响其余文件。
//! - 结果 data=`{ files: [{ path, ok, content?, error?, truncated, … }],
//!   truncatedFiles }`；模型据此行动的续读指引写入 error/nextOffset。

use serde::Deserialize;
use serde_json::json;

use crate::safety::tier::Capability;
use crate::tools::read::{DEFAULT_LINE_LIMIT, read_text_window};
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};
use crate::util::encoding::MAX_RESPONSE_BYTES;
use crate::util::errors::ToolError;

/// batch_read 工具参数：目标路径列表（相对或绝对，须在 roots 内）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReadArgs {
    /// 要读取的文件路径列表（按源序逐个读取）。
    pub paths: Vec<String>,
}

/// 执行 batch_read：源序逐文件读取，共享 512KB 响应预算；单文件失败不失败
/// 整批。`limit` 为本批文件数上限（`AppConfig.tools.batchReadLimit` 的运行面，
/// 执行器装配时注入，缺省 [`DEFAULT_BATCH_READ_LIMIT`]）。
pub fn run(ctx: &ToolsContext, args: &BatchReadArgs, limit: u32) -> ToolOutcome {
    if args.paths.is_empty() {
        return ToolError::invalid_arg("paths 不能为空：至少给出一个文件路径").into();
    }
    if args.paths.len() > limit as usize {
        return ToolError::invalid_arg(format!(
            "单批最多读取 {limit} 个文件（实际 {} 个）：请减少 paths 或分批调用",
            args.paths.len()
        ))
        .into();
    }

    // 共享预算：批内所有文件的内容字节合计 ≤512KB（512KB 单响应上限
    // 跨全批次共享）。预算截断后剩余预算清零——该文件尚未读完，模型本就
    // 需要续读，后续文件不再占用本次响应。
    let mut remaining = MAX_RESPONSE_BYTES;
    let mut files: Vec<serde_json::Value> = Vec::with_capacity(args.paths.len());
    let mut truncated_files: Vec<String> = Vec::new();

    for path in &args.paths {
        if remaining == 0 {
            files.push(json!({
                "path": path,
                "ok": false,
                "truncated": false,
                "error": {
                    "code": "E_INVALID_ARG",
                    "message": "批响应预算（512KB）已耗尽，本文件未读取：请分批调用或减少单批文件数",
                },
            }));
            continue;
        }
        let entry = match ctx.guard().and_then(|g| g.resolve(path)) {
            Ok(resolved) => {
                match read_text_window(&resolved, path, 1, DEFAULT_LINE_LIMIT, remaining) {
                    Ok(w) => {
                        let truncated = w.truncated;
                        if w.byte_truncated {
                            remaining = 0;
                        } else {
                            remaining = remaining.saturating_sub(w.content.len());
                        }
                        if truncated {
                            truncated_files.push(path.clone());
                        }
                        json!({
                            "path": path,
                            "ok": true,
                            "encoding": w.encoding,
                            "totalLines": w.total_lines,
                            "returnedLines": w.returned_lines,
                            "content": w.content,
                            "truncated": truncated,
                            "nextOffset": w.next_offset,
                        })
                    }
                    Err(e) => json!({
                        "path": path,
                        "ok": false,
                        "truncated": false,
                        "error": { "code": e.code.to_string(), "message": e.message },
                    }),
                }
            }
            // 路径越界（E_PATH_OUTSIDE_ROOTS）等守卫失败同样只标记该文件。
            Err(e) => json!({
                "path": path,
                "ok": false,
                "truncated": false,
                "error": { "code": e.code.to_string(), "message": e.message },
            }),
        };
        files.push(entry);
    }

    let ok_count = files.iter().filter(|f| f["ok"] == json!(true)).count();
    let fail_count = files.len() - ok_count;
    let mut summary = format!(
        "批量读取 {} 个文件：{} 成功、{} 失败",
        files.len(),
        ok_count,
        fail_count
    );
    if !truncated_files.is_empty() {
        summary.push_str(&format!(
            "；{} 个文件内容截断（按 nextOffset 续读）",
            truncated_files.len()
        ));
    }

    ToolOutcome::success(
        json!({
            "files": files,
            "truncatedFiles": truncated_files,
        }),
        summary,
    )
}

/// batch_read 工具规格（能力 ReadOnly，全档放行）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "batch_read".to_string(),
        description: "一次读取多个文本文件的开头窗口（每文件默认 2000 行，与 read 同款编码探测）。\
             单批文件数有上限，且全部文件内容共享 512KB 响应预算——预算截断的文件带 truncated 与续读 \
             nextOffset；单个文件失败不影响其余文件。大范围调研时用它代替逐个 read。"
            .to_string(),
        capability: Capability::ReadOnly,
        params_schema: json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "要读取的文件路径列表（工作区内相对路径或绝对路径），按源序读取"
                }
            },
            "required": ["paths"],
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
#[path = "batch_read_test.rs"]
mod batch_read_test;
