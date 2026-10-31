//! toolres 边车：`sessions/<sessionId>.toolres/<callId>.json`。
//!
//! 术语解释：「边车（sidecar）」指与主会话日志放在一起、只存某一类附属数据的
//! 独立文件；这里每个工具调用结果（`ToolOutcome` 全量 JSON，含 detail 明细；
//! warnings 只走 UI 实时通道不落盘）单独存一个 JSON 文件，按调用编号 callId
//! 命名。为什么单独放边车：工具结果可能很大，混进主日志会让会话文件迅速膨胀，
//! 分开后主日志保持轻量、工具卡恢复时按 callId 批量拉取即可
//!（见 `load_tool_outcomes`）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::tools::ToolOutcome;
use crate::util::atomic::write_atomic;

/// 单文件上限（5MB，超限拒入，改写占位引用）。
pub const TOOLRES_MAX_BYTES: usize = 5 * 1024 * 1024;

/// 边车目录（布局：数据根/sessions/<sessionId>.toolres/）。
pub fn toolres_dir(data_root: &Path, session_id: &str) -> PathBuf {
    data_root
        .join("sessions")
        .join(format!("{session_id}.toolres"))
}

/// callId → 文件名（防御性：桥接 id 本身为安全字符集，异常字符替换防路径逃逸）。
fn call_file(dir: &Path, call_id: &str) -> PathBuf {
    let safe: String = call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{safe}.json"))
}

/// 写一个工具结果边车（工具执行完成点调用，原子写 = fsync，逐结果落）。
/// 超限写占位（`{ok, uiSummary, truncated:true}`——spill 全量细节已由
/// command 工具自身落盘；其余工具超限时 UI 降级用帧面 summary。
pub fn write_outcome(data_root: &Path, session_id: &str, call_id: &str, outcome: &ToolOutcome) {
    let dir = toolres_dir(data_root, session_id);
    let path = call_file(&dir, call_id);
    let payload = match serde_json::to_vec(outcome) {
        Ok(bytes) if bytes.len() <= TOOLRES_MAX_BYTES => bytes,
        _ => serde_json::to_vec(&ToolOutcome {
            ok: outcome.ok,
            data: None,
            error: None,
            ui_summary: outcome.ui_summary.clone(),
            detail: None,
            truncated: Some(true),
        })
        .unwrap_or_default(),
    };
    // 边车属尽力而为通道：失败记日志不中断执行（帧面 summary 仍可用）
    if let Err(e) = write_atomic(&path, &payload) {
        #[cfg(debug_assertions)]
        eprintln!("[craftwave] toolres 边车写入失败（{call_id}）：{e}");
        #[cfg(not(debug_assertions))]
        let _ = e;
    }
}

/// 批量读取（`load_tool_outcomes`；缺文件的 callId 不出现在返回表中，前端降级）。
pub fn load_outcomes(
    data_root: &Path,
    session_id: &str,
    call_ids: &[String],
) -> HashMap<String, Value> {
    let dir = toolres_dir(data_root, session_id);
    let mut out = HashMap::with_capacity(call_ids.len());
    for call_id in call_ids {
        if let Ok(raw) = fs::read_to_string(call_file(&dir, call_id))
            && let Ok(value) = serde_json::from_str(&raw)
        {
            out.insert(call_id.clone(), value);
        }
    }
    out
}
