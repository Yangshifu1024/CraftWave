//! 【S2+S3 Spike】测试公共模块：mock 服务与 rig 测试装配。

// 各集成测试二进制独立链接本模块，未用到的导出属预期。
#![allow(dead_code)]

pub mod mock_openai;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use craftwave_lib::spike::frames::{ChatFrame, DeltaThrottler, FrameSink, SequencedFrame};
use craftwave_lib::spike::turn::{drive_manual_tool_loop, ToolExecutor, TurnDriver};
use rig::completion::request::ToolDefinition;
use rig::completion::Message;
use rig::message::ToolName;
use rig::providers::openai::wire::OpenAIConfig;
use rig::providers::openai::OpenAI;
use rig::run::spec::RunSpec;
use rig::run::AgentRun;

/// mock 服务里声明的工具名。
pub const TOOL_READ: &str = "read";

/// 构造指向 mock 的 rig OpenAI 客户端与 Chat wire 模型。
pub fn mock_model(
    mock: &mock_openai::MockOpenAI,
) -> rig::driver::Model<rig::providers::openai::wire::Chat> {
    let config = OpenAIConfig::new("mock-key".to_owned()).with_base_url(mock.base_url.clone());
    let client: OpenAI = config.client();
    client.chat("gpt-4o-mock")
}

/// spike 用的单工具集（read），与 mock 脚本的工具名一致。
pub fn read_tool() -> Arc<ToolDefinition> {
    Arc::new(ToolDefinition::new(
        ToolName::new(TOOL_READ.to_owned()).expect("tool name"),
        "读取工作区内文件的一段文本",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "offset": {"type": "integer"},
                "limit": {"type": "integer"}
            },
            "required": ["path"]
        }),
    ))
}

/// 组装 TurnDriver。
pub fn turn_driver(mock: &mock_openai::MockOpenAI) -> TurnDriver {
    TurnDriver::new(mock_model(mock), RunSpec::new(), vec![read_tool()])
}

/// 组装一个手动循环的 run（预算 4 个模型调用）。
pub fn new_run() -> AgentRun {
    AgentRun::new(Message::user("帮我读配置文件")).max_turns(4)
}

/// 假工具执行器：返回固定文本（含收到的参数，供断言参数完整性）。
pub struct FakeExecutor;

impl ToolExecutor for FakeExecutor {
    fn execute(&self, name: &str, args_json: &str) -> String {
        format!("[{name}] 共 12 行：port = 8087（参数 {args_json}）")
    }
}

/// 带到达时刻的帧记录（端到端延迟埋点用）。
#[derive(Debug, Clone)]
pub struct TimedFrame {
    pub at: Instant,
    pub frame: SequencedFrame,
}

/// FrameSink 实现：收进共享 Vec 并打时刻（出口处递增 seq，对应 RunHandle 语义）。
#[derive(Default, Clone)]
pub struct TimedSink(pub Arc<Mutex<Vec<TimedFrame>>>);

impl FrameSink for TimedSink {
    fn send(&mut self, frame: ChatFrame) {
        let mut frames = self.0.lock().unwrap();
        let seq = frames.len() as u64;
        frames.push(TimedFrame {
            at: Instant::now(),
            frame: SequencedFrame { seq, frame },
        });
    }
}

/// 直通驱动：流事件逐个转帧直接进 TimedSink（保真断言用）。
pub async fn drive_direct(
    driver: &TurnDriver,
    run: AgentRun,
    executor: Arc<dyn ToolExecutor>,
    mut sink: TimedSink,
) -> Result<rig::run::response::PromptResponse, rig::run::PromptError> {
    let mut on_frame = move |frame: ChatFrame| sink.send(frame);
    drive_manual_tool_loop(driver, run, executor, &mut on_frame).await
}

/// 节流驱动：增量先进 64ms 合并器。首增量立即出帧（TTFT 优先），其后每次
/// push 时检查节流窗口（事件驱动，无外部 tick 任务）；循环结束后调用方须做
/// 收尾 flush（把仍在窗口里的增量放行）。
pub async fn drive_throttled(
    driver: &TurnDriver,
    run: AgentRun,
    executor: Arc<dyn ToolExecutor>,
    throttler: Arc<Mutex<DeltaThrottler>>,
    mut sink: TimedSink,
) -> Result<rig::run::response::PromptResponse, rig::run::PromptError> {
    let mut on_frame = move |frame: ChatFrame| {
        let mut t = throttler.lock().unwrap();
        match &frame {
            ChatFrame::TextDelta { turn, delta } => t.push_text(*turn, delta),
            ChatFrame::ReasoningDelta { turn, delta } => t.push_reasoning(*turn, delta),
            ChatFrame::ToolArgs { call_id, chunk } => t.push_tool_args(call_id, chunk),
            // 非增量帧直通（保证 toolStarted/toolResult 顺序不被节流扰动）
            other => {
                sink.send(other.clone());
                return;
            }
        }
        if t.should_flush() {
            t.flush(&mut sink);
        }
    };
    drive_manual_tool_loop(driver, run, executor, &mut on_frame).await
}
