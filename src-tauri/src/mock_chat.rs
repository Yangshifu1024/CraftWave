//! 【S1 spike 原型，随时可弃】dev-only mock 帧源：按剧本发 ToolStart / ToolProgress / ToolResult。
//!
//! 不接真 rig、不需要 API key。验证点：前端镜像工具（ui/src/chat/mirror/）能被 Channel 帧驱动出
//! 完整状态机；参数增量以半截 JSON 分片下发，前端以「参数准备中」展示。
//! 仅 debug 构建注册（`#[cfg(debug_assertions)]`），release 不进 invoke_handler。
//! 剧本与 ui/src/chat/mirror/mockSource.ts 的 MOCK_SCRIPT 同构（原型期人工对齐），
//! 一致性由 mock_chat_test.rs 断言。

use serde::Serialize;
use tauri::ipc::Channel;

/// 单条 Channel 帧。tag 用 kebab-case 对齐前端判别值（"text-delta" 等），
/// 字段名 camelCase 对齐 D14（负载字段 camelCase）。
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum MockFrame {
    /// 文本增量。
    TextDelta { seq: u32, delta: String },
    /// 工具开始。
    ToolStart {
        seq: u32,
        tool_call_id: String,
        tool_name: String,
    },
    /// 参数增量（半截 JSON 文本分片）。
    ToolProgress {
        seq: u32,
        tool_call_id: String,
        args_text_delta: String,
    },
    /// 工具结果。
    ToolResult {
        seq: u32,
        tool_call_id: String,
        result: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    /// run 结束。
    RunEnded { seq: u32 },
}

/// 剧本步骤：自上一步起等待毫秒数 + 帧。
pub(crate) struct ScriptStep {
    /// 发出本帧前先等待的毫秒数（模拟真实流式的节奏间隔）。
    pub delay_ms: u64,
    /// 要发出的帧。
    pub frame: MockFrame,
}

/// 验证剧本（覆盖票面全部判定点）：正文 → 工具1（半截 JSON 参数增量 ×2 → 参数齐 → 完成）
/// → 工具2（完整参数 → isError 结果）→ 收尾正文 → run-ended。
pub(crate) fn mock_script() -> Vec<ScriptStep> {
    use MockFrame::*;
    vec![
        ScriptStep {
            delay_ms: 30,
            frame: TextDelta {
                seq: 1,
                delta: "我来查看一下项目文件。".into(),
            },
        },
        ScriptStep {
            delay_ms: 60,
            frame: ToolStart {
                seq: 2,
                tool_call_id: "call-1".into(),
                tool_name: "read_workspace_file".into(),
            },
        },
        // 半截 JSON 增量 ×2（参数准备中）
        ScriptStep {
            delay_ms: 60,
            frame: ToolProgress {
                seq: 3,
                tool_call_id: "call-1".into(),
                args_text_delta: r#"{"path": "src/"#.into(),
            },
        },
        ScriptStep {
            delay_ms: 60,
            frame: ToolProgress {
                seq: 4,
                tool_call_id: "call-1".into(),
                args_text_delta: r#"main.rs", "#.into(),
            },
        },
        // 参数到齐（运行中）
        ScriptStep {
            delay_ms: 60,
            frame: ToolProgress {
                seq: 5,
                tool_call_id: "call-1".into(),
                args_text_delta: r#""startLine": 1}"#.into(),
            },
        },
        ScriptStep {
            delay_ms: 120,
            frame: ToolResult {
                seq: 6,
                tool_call_id: "call-1".into(),
                result: serde_json::json!({ "content": "fn main() {}" }),
                is_error: None,
            },
        },
        ScriptStep {
            delay_ms: 60,
            frame: ToolStart {
                seq: 7,
                tool_call_id: "call-2".into(),
                tool_name: "run_command".into(),
            },
        },
        ScriptStep {
            delay_ms: 60,
            frame: ToolProgress {
                seq: 8,
                tool_call_id: "call-2".into(),
                args_text_delta: r#"{"command": "cargo test"}"#.into(),
            },
        },
        ScriptStep {
            delay_ms: 120,
            frame: ToolResult {
                seq: 9,
                tool_call_id: "call-2".into(),
                result: serde_json::json!({ "exitCode": 101, "stderr": "编译失败" }),
                is_error: Some(true),
            },
        },
        ScriptStep {
            delay_ms: 60,
            frame: TextDelta {
                seq: 10,
                delta: "读取完成，但命令执行失败了。".into(),
            },
        },
        ScriptStep {
            delay_ms: 30,
            frame: RunEnded { seq: 11 },
        },
    ]
}

/// dev-only：按剧本向前端 Channel 顺序发帧。命令体直到剧本播完才返回，
/// 前端以 invoke resolve 为「流结束」信号（@tauri-apps/api 2.12.4 的 Channel 无 onclose）。
#[tauri::command]
pub(crate) async fn mock_chat_run(channel: Channel<MockFrame>) -> Result<(), String> {
    for step in mock_script() {
        tokio::time::sleep(std::time::Duration::from_millis(step.delay_ms)).await;
        let _ = channel.send(step.frame);
    }
    Ok(())
}
