//! 请求 token 估算器（每请求预算检查的估算面）。
//!
//! 名词解释：token（词元）是模型处理文本的最小单位；上下文窗口（context
//! window）是单次请求能装下的 token 总量上限。每次向模型发请求前，后端要
//! 判断"这次要发的内容 + 模型的回复空间"是否塞得进窗口——判断依据就是本
//! 模块的估算值。
//!
//! 为什么用启发式而不是精确分词器：精确 tokenizer（tiktoken 等）体积大、
//! 按供应商分化，而预算检查的语义是"宁可高估提前压缩，不可低估导致超长
//! 报错"——启发式只要系统性偏高就是安全的。口径：
//! - CJK 字符（中日韩统一表意/扩展/兼容区 + 全角标点）每字按 1.5 token 计
//!   （主流分词器对中文约 0.75–1.5 token/字，取上沿）；
//! - 其他字符（拉丁字母、空白、JSON 结构等）每 4 字符按 1 token 计；
//! - 两组分别上取整后求和。
//!
//! 最近 usage 帧仅作校准参考（不能以无 usage 为由跳过检查——估算
//! 面完全本地，首轮请求即可判定）。

use crate::storage::{MessageEntry, WirePart};

/// 单段文本的 token 估算。
pub fn estimate_text_tokens(text: &str) -> u64 {
    let mut cjk: u64 = 0;
    let mut other: u64 = 0;
    for ch in text.chars() {
        if is_cjk(ch) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    // CJK ×1.5（×3/2 上取整）+ 其他 ÷4 上取整
    (cjk * 3).div_ceil(2) + other.div_ceil(4)
}

/// 是否 CJK 字符（中日韩表意区/扩展 A–F/兼容表意/兼容形式/全角符号）。
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x2E80..=0x9FFF      // CJK 部首补充/符号标点/注音 → 中日韩统一表意文字（含 3000–303F 标点区）
        | 0xF900..=0xFAFF    // CJK 兼容表意文字
        | 0xFF00..=0xFFEF    // 全角/半角形式（全角标点与字母）
        | 0x20000..=0x2FA1F  // 表意文字扩展 B–F 区
    )
}

/// wire part 的 token 估算（按文本表示面估算，与 wire 重建后的模型输入同构）。
fn estimate_parts_tokens(parts: &[WirePart]) -> u64 {
    let mut total = 0u64;
    for part in parts {
        match part {
            WirePart::Text { text } => total += estimate_text_tokens(text),
            WirePart::Reasoning { text } => {
                // reasoning 进模型请求面（assistant 消息的一部分）
                total += estimate_text_tokens(text);
            }
            WirePart::ToolCall {
                call_id,
                name,
                args_json,
            } => {
                // 工具调用以 JSON 文本进入请求
                total += estimate_text_tokens(&format!(
                    r#"{{"id":"{call_id}","name":"{name}","args":{args_json}}}"#
                ));
            }
            WirePart::ToolResult {
                call_id,
                ok,
                summary,
            } => {
                total += estimate_text_tokens(&format!(
                    r#"{{"id":"{call_id}","ok":{ok},"content":"{summary}"}}"#
                ));
            }
            WirePart::PlanSnapshot { .. } | WirePart::RunMarker { .. } => {
                // wire 重建跳过：不占模型请求面
            }
            WirePart::Image { blob_hash, .. } => {
                // 图片：视觉 token 随供应商与分辨率分化，无法从
                // 字节数估算；按宁高勿低给常数 2048 token/张（对应主流
                // 视觉模型一张中分辨率图的实际消耗上沿），加占位文本面
                //（非 vision 模型收到的占位与摘要块都含哈希文本）。
                total += 2048 + estimate_text_tokens(blob_hash);
            }
        }
    }
    total
}

/// 消息序列（活动路径条目）的 token 估算。
pub fn estimate_entries_tokens(entries: &[MessageEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| 4u64 + estimate_parts_tokens(&entry.parts)) // 每条消息 4 token 结构开销
        .sum()
}

/// 工具定义表（名称 + 描述 + 参数 schema 文本）的 token 估算。
/// 工具表每次请求都随请求广播给模型（预算覆盖面）。
pub fn estimate_tool_tokens(tools: &[(String, String, String)]) -> u64 {
    tools
        .iter()
        .map(|(name, description, schema)| {
            estimate_text_tokens(&format!("{name} {description} {schema}")) + 8
        })
        .sum()
}

/// 一次请求的完整预算估算：
/// 系统前缀（customPrompt/摘要）+ history + prompt + 工具表 + 回复预留。
pub fn estimate_request_tokens(
    system_prefixes: &[&str],
    history_entries: &[MessageEntry],
    prompt_text: &str,
    tools: &[(String, String, String)],
    reply_reserve: u64,
) -> u64 {
    let prefix: u64 = system_prefixes
        .iter()
        .map(|t| estimate_text_tokens(t) + 4)
        .sum();
    prefix
        + estimate_entries_tokens(history_entries)
        + estimate_text_tokens(prompt_text)
        + 4
        + estimate_tool_tokens(tools)
        + reply_reserve
}

/// 自动压缩判定（占用 ≥ 窗口 × threshold）。
pub fn should_auto_compact(estimated_tokens: u64, context_window: u64, threshold: f64) -> bool {
    if context_window == 0 {
        return false;
    }
    let budget = (context_window as f64 * threshold).floor() as u64;
    estimated_tokens >= budget
}

/// 硬预算判定（wire 预算超限线：占用 > 窗口 × 0.9）。
pub fn is_over_wire_budget(estimated_tokens: u64, context_window: u64) -> bool {
    if context_window == 0 {
        return false;
    }
    estimated_tokens > (context_window as f64 * 0.9) as u64
}

#[cfg(test)]
#[path = "estimator_test.rs"]
mod estimator_test;
