//! 工具错误与 IPC 错误（错误码注册表）。
//!
//! 放 util 而非 tools：safety（safe_join/围栏）与 tools 都要返回 `ToolError`；
//! `IpcError` 是 IPC 命令统一 reject 负载，host 与 providers 共用——util 是被
//! 所有层依赖的最底层，两个类型下沉于此，避免 providers 反向依赖 host 造成环。

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 工具/安全层错误的统一载体：`code` 必须取自 错误码注册表。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
}

impl ToolError {
    /// 以注册表错误码 + 人读消息直接构造（优先用下面的语义化快捷方法）。
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// E_INVALID_ARG：参数校验失败（通用）。
    pub fn invalid_arg(message: impl Into<String>) -> Self {
        Self::new("E_INVALID_ARG", message)
    }

    /// E_NOT_FOUND：目标不存在（通用）。
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("E_NOT_FOUND", message)
    }

    /// E_PATH_OUTSIDE_ROOTS：路径越界。
    pub fn path_outside_roots(message: impl Into<String>) -> Self {
        Self::new("E_PATH_OUTSIDE_ROOTS", message)
    }

    /// E_FENCE_BLOCKED：命令围栏 Block。
    pub fn fence_blocked(message: impl Into<String>) -> Self {
        Self::new("E_FENCE_BLOCKED", message)
    }

    /// E_INTERNAL_STATE：试图写宿主内部状态。
    pub fn internal_state(message: impl Into<String>) -> Self {
        Self::new("E_INTERNAL_STATE", message)
    }

    /// E_TOOL_BLOCKED：权限档排除。
    pub fn tool_blocked(message: impl Into<String>) -> Self {
        Self::new("E_TOOL_BLOCKED", message)
    }

    /// E_EDIT_NO_MATCH / E_EDIT_AMBIGUOUS / E_EDIT_VERSION_STALE。
    pub fn edit_no_match(message: impl Into<String>) -> Self {
        Self::new("E_EDIT_NO_MATCH", message.into())
    }

    /// E_EDIT_AMBIGUOUS：编辑目标在文件里匹配到多处。
    pub fn edit_ambiguous(message: impl Into<String>) -> Self {
        Self::new("E_EDIT_AMBIGUOUS", message.into())
    }

    /// E_EDIT_VERSION_STALE：编辑所基于的文件版本已过期（他人已改过）。
    pub fn edit_version_stale(message: impl Into<String>) -> Self {
        Self::new("E_EDIT_VERSION_STALE", message.into())
    }

    /// E_PLAN_STALE：已批准计划过期仍写。
    pub fn plan_stale(message: impl Into<String>) -> Self {
        Self::new("E_PLAN_STALE", message.into())
    }

    /// E_PLAN_REQUIRED：严格硬门开启且无已批准计划时写
    /// （approval.requirePlan 默认关）。
    pub fn plan_required(message: impl Into<String>) -> Self {
        Self::new("E_PLAN_REQUIRED", message.into())
    }

    /// E_COMMAND_TIMEOUT：命令执行超时（补录）。
    pub fn command_timeout(message: impl Into<String>) -> Self {
        Self::new("E_COMMAND_TIMEOUT", message)
    }

    /// E_SSRF_BLOCKED：SSRF 防护拦截（实装）——message 带命中
    /// 类别（私网/回环/链路本地/CGNAT/ULA/保留段/重定向降级等）供模型纠正。
    pub fn ssrf_blocked(message: impl Into<String>) -> Self {
        Self::new("E_SSRF_BLOCKED", message)
    }
}

/// 展示为 "code: message"，日志与调试可读。
impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// 空 impl：Display 已带出全部信息，这里只为让它能当标准错误类型传。
impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    /// io 错误映射：NotFound → E_NOT_FOUND；其余归 E_INTERNAL_STATE 兜底。
    /// 注册表无通用 IO 码，映射决策见 Answer。
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => {
                Self::not_found(format!("file or directory not found: {e}"))
            }
            _ => Self::new(
                "E_INTERNAL_STATE",
                format!("filesystem operation failed: {e}"),
            ),
        }
    }
}

// ---- 错误码常量（注册表内已落地的键面）----
// 命名与字符串字面量同形，供手写 code 的调用处以常量引用、避免拼写漂移。

/// 参数校验失败。
pub const E_INVALID_ARG: &str = "E_INVALID_ARG";
/// 目标（文件/会话等）不存在。
pub const E_NOT_FOUND: &str = "E_NOT_FOUND";
/// 会话已有活跃 run，拒绝再次启动。
pub const E_SESSION_RUNNING: &str = "E_SESSION_RUNNING";
/// 会话树同步基线过期（前端版本落后于落盘版本）。
pub const E_SESSION_REV: &str = "E_SESSION_REV";
/// 磁盘 IO 失败。
pub const E_STORAGE_IO: &str = "E_STORAGE_IO";
/// OS 层文件与目录操作失败（open_dir 等命令的 io::Error 归一面）。
pub const E_IO: &str = "E_IO";
/// 内部状态兜底（未分类的意外错误）。
pub const E_INTERNAL: &str = "E_INTERNAL";
/// 命令执行超时。
pub const E_COMMAND_TIMEOUT: &str = "E_COMMAND_TIMEOUT";
/// 已批准的计划已过期仍尝试写入。
pub const E_PLAN_STALE: &str = "E_PLAN_STALE";
/// 严格计划硬门：开关开启且无已批准计划时的写入
/// （approval.requirePlan 默认关）。
pub const E_PLAN_REQUIRED: &str = "E_PLAN_REQUIRED";
/// 命令围栏拦截（Block 档）。
pub const E_FENCE_BLOCKED: &str = "E_FENCE_BLOCKED";
/// 灰区命令模型审计拒绝（实装）：审计模型判断高风险，
/// 操作不执行，run 以 error 终态收场——区别于 E_FENCE_BLOCKED 确定性硬拒，
/// 本结论来自单次模型审计（灰区风险定性），message 附简短依据。
pub const E_AUDIT_BLOCKED: &str = "E_AUDIT_BLOCKED";
/// 试图写宿主内部状态（实装禁区硬拒——write/edit
/// 路径与 command 写目标命中 config/凭据/MCP 配置/索引/锁面等禁区，
/// 任何权限档与白名单不豁免）。
pub const E_INTERNAL_STATE: &str = "E_INTERNAL_STATE";
/// 当前权限档排除了该工具。
pub const E_TOOL_BLOCKED: &str = "E_TOOL_BLOCKED";
/// 路径越界（safe_join 与 open_dir 白名单共用）。
pub const E_PATH_OUTSIDE_ROOTS: &str = "E_PATH_OUTSIDE_ROOTS";
/// 上下文超预算且压缩后仍放不下——暂停执行并保留输入。
pub const E_CONTEXT_OVERFLOW: &str = "E_CONTEXT_OVERFLOW";
/// 同批 ask 互斥或同文件写冲突时未执行的调用——工具级失败回喂，模型据说明
/// 重新决定或纠正（步骤 4）。
pub const E_TOOL_SKIPPED: &str = "E_TOOL_SKIPPED";
/// 流停滞看门狗终止：模型流停滞超过阈值或重复失败/空转越界。
pub const E_RUN_STALLED: &str = "E_RUN_STALLED";
/// 未信任的项目资源访问/注入（首次实装：未信任技能的
/// `skill` 工具调用按本码拒绝回喂，模型须先经用户信任确认）。
pub const E_PROJECT_UNTRUSTED: &str = "E_PROJECT_UNTRUSTED";
/// 子代理步数/预算耗尽：以工具级失败回喂父 run，
/// 子代理终态归 outcome=partial。
pub const E_SUB_BUDGET: &str = "E_SUB_BUDGET";
/// SSRF 防护拦截：目标 URL/解析地址命中私网/回环/链路本地/CGNAT/ULA 等保留段，
/// 或重定向 HTTPS 降级/超跳——message 带命中类别。
pub const E_SSRF_BLOCKED: &str = "E_SSRF_BLOCKED";
/// 网络搜索全部通道失败：web_search 按钉死优先级（SearXNG → 内置抓取 →
/// Tavily）逐层回落仍无可用通道——message 带逐层失败原因与配置引导，
/// detail.attempts 带结构化逐层记录。
pub const E_SEARCH_UNAVAILABLE: &str = "E_SEARCH_UNAVAILABLE";

/// IPC 命令统一错误负载：`{ code, message, detail? }`，camelCase。
///
/// 命令层一律返回 `Result<T, IpcError>`；Tauri 把 Serialize 错误直接序列化给
/// 前端 reject。与 `ToolError` 的分工：IpcError 走 IPC 边界（可带 `detail`、
/// 需要 Deserialize 供测试/工具回放），ToolError 承载于 ToolOutcome。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub detail: Option<Value>,
}

impl IpcError {
    /// 以错误码 + 人读消息构造（detail 默认为无）。
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            detail: None,
        }
    }

    /// 附带结构化 detail（如失败原因的 JSON 面板数据），链式调用。
    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }

    /// E_INVALID_ARG：参数校验失败。
    pub fn invalid_arg(message: impl Into<String>) -> Self {
        Self::new(E_INVALID_ARG, message)
    }

    /// E_NOT_FOUND：目标不存在。
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(E_NOT_FOUND, message)
    }

    /// E_SESSION_REV：树同步基线过期。
    pub fn session_rev(message: impl Into<String>) -> Self {
        Self::new(E_SESSION_REV, message)
    }

    /// E_SESSION_RUNNING：会话已有活跃 run。
    pub fn session_running(message: impl Into<String>) -> Self {
        Self::new(E_SESSION_RUNNING, message)
    }

    /// E_STORAGE_IO：磁盘 IO 失败（补录）。
    pub fn storage_io(message: impl Into<String>) -> Self {
        Self::new(E_STORAGE_IO, message)
    }

    /// E_IO：OS 层文件与目录操作失败（补行、实装）。
    pub fn io(message: impl Into<String>) -> Self {
        Self::new(E_IO, message)
    }

    /// E_PATH_OUTSIDE_ROOTS：路径越界。
    pub fn path_outside_roots(message: impl Into<String>) -> Self {
        Self::new(E_PATH_OUTSIDE_ROOTS, message)
    }

    /// E_INTERNAL：内部状态兜底。
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(E_INTERNAL, message)
    }
}

/// 展示为 "[code] message"，日志与调试可读。
impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// 空 impl：Display 已带出全部信息，这里只为让它能当标准错误类型传。
impl std::error::Error for IpcError {}

#[cfg(test)]
#[path = "errors_test.rs"]
mod errors_test;
