//! 工具系统：注册表 + 各工具实现 + 批次调度（§8.1）。
//!
//! 工具（tool）是模型可调用的能力单元（读文件、执行命令等）；工具注册表
//! （registry，即 [`build_registry`] 返回的清单）在启动时集中登记全部工具
//! 规格供模型选择。
//!
//! M1 第二切片（14 号票）：在五文件类工具（11 号票）之上接线
//! command/plan/ask 三工具与 suggest 后台小调用（非模型工具，不进注册表，
//! 由 host 在 run 收尾发起）。审批门与工具卡片 UI 接线同票落地。
//!
//! 双通道结果（§8.2）：工具返回 [`ToolOutcome`]，模型通道 compact 文本经
//! [`compact_text`] 生成（成功头 4KB + 尾 8KB 截断；失败 error code+message）。

pub mod ask;
pub mod batch_read;
pub mod calculate;
pub mod command;
// 文档读取管线（16 号票，§8.9 读侧）：read 工具按扩展名分派 docx/xlsx/pdf
// 解析；不设独立工具（§8.3 read 行），注册表清单不变。
pub mod doc;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod http_request;
pub mod list_files;
pub mod plan;
pub mod postcheck;
pub mod process;
pub mod read;
pub mod render_html;
pub mod web_fetch;
pub mod web_search;
pub mod write;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::providers::types::ProxyConfig;
use crate::safety::safe_join::PathGuard;
use crate::safety::ssrf::{HostThrottle, SsrfPolicy};
use crate::safety::tier::{Capability, GateDecision, PermissionTier, decide_gate};
use crate::util::errors::ToolError;

/// 工具执行上下文：roots（项目主目录 + 数据目录）由调用方构造。
/// M1 无 extra_roots（§9.1）；24 号票起可追加会话授权的外部目录
/// （§9.9 会话 roots 快照）。
#[derive(Debug, Clone)]
pub struct ToolsContext {
    /// 项目主目录（canonical），相对路径的解析基准与首个 root。
    pub project_root: PathBuf,
    /// 数据目录（canonical）。
    pub data_root: PathBuf,
    /// 会话已授权的外部目录（§9.9，24 号票；canonical 绝对路径）。
    /// 空 = 无授权（缺省）。
    pub extra_roots: Vec<PathBuf>,
}

/// 网络工具执行环境（12 号票，web_fetch/http_request 共用；13 号票扩
/// web_search）：SSRF 策略、代理配置与进程级节流器，外加搜索三通道的
/// 配置装配面（[`web_search::SearchEnv`]）。由宿主按 `AppConfig` 装配注入
/// 执行器（save_config 后下一次 start_chat 生效——与 watchdog/tools 组同
/// 语义）；`throttle` 为进程级共享句柄（Arc）：跨会话/跨 run 对同一 host
/// 的访问共同参与 1s 节流（§9.2）。
#[derive(Clone)]
pub struct NetworkEnv {
    /// SSRF 策略（`network.allowPrivateNetwork` 的运行面，缺省拒绝私网）。
    pub policy: SsrfPolicy,
    /// 代理配置（`AppConfig.proxy`；None = 未配置。19 号票 client 重建
    /// 路径落地前按本配置直建，§9.2 收敛点注记）。
    pub proxy: Option<ProxyConfig>,
    /// 进程级每 host 节流器（跨会话共享）。
    pub throttle: Arc<HostThrottle>,
    /// web_search 三通道装配（13 号票，§8.3/C-12）：SearXNG 启用配置 +
    /// Tavily Key + 测试注入端点；随本环境按 AppConfig 装配。
    pub search: web_search::SearchEnv,
}

// NetworkEnv 的构造。
impl NetworkEnv {
    /// 按应用配置装配（policy 取 network 组，proxy 取代理组，search 取
    /// search 组——三通道的启用面一次配齐；节流器与环境共用同一句柄）。
    pub fn from_config(config: &crate::providers::types::AppConfig) -> Self {
        let search = web_search::SearchEnv::from_config(config);
        let throttle = search.throttle.clone();
        Self {
            policy: SsrfPolicy {
                allow_private_network: config.network.allow_private_network,
            },
            proxy: config.proxy.clone(),
            throttle,
            search,
        }
    }
}

// ToolsContext 的构造与路径守卫方法。
impl ToolsContext {
    /// 构造工具执行上下文（两个根目录应传 canonical 后的绝对路径）。
    /// `data_root` 语义为「第二根」：生产路径传
    /// `resolve_session_roots` 收窄后的 `<data>/workspace/` 子目录
    /// （C-8/21 号票），测试直接传临时目录。
    pub fn new(project_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            project_root,
            data_root,
            extra_roots: Vec::new(),
        }
    }

    /// 追加会话授权的外部目录（§9.9，24 号票；run 装配时从 SessionMeta
    /// 读取——授权跨 run 生效、跨会话不延续）。
    pub fn with_extra_roots(mut self, extra_roots: Vec<PathBuf>) -> Self {
        self.extra_roots = extra_roots;
        self
    }

    /// 构造路径守卫（roots canonicalize 失败时返回参数错误；授权目录
    /// canonicalize 失败的条目跳过——不存在的授权根不阻塞其余根）。
    pub fn guard(&self) -> Result<PathGuard, ToolError> {
        let mut roots = vec![self.project_root.clone(), self.data_root.clone()];
        for extra in &self.extra_roots {
            // PathGuard::new 对任一根 canonicalize 失败都会整体报错；
            // 授权目录可能已被删除（跨 run 后），跳过即可（下次访问该
            // 目录重新走授权询问）。
            if std::fs::canonicalize(extra).is_ok() {
                roots.push(extra.clone());
            }
        }
        PathGuard::new(roots)
    }
}

/// 工具注解（镜像 MCP annotations 的 P0 子集）：
/// 给宿主/客户端的行为提示，不影响本端的权限判定。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    /// 提示该工具不修改状态（只读）。
    pub read_only_hint: bool,
    /// 提示该工具可能破坏性修改数据。
    pub destructive_hint: bool,
    /// 提示该工具重复执行结果一致（幂等）。
    pub idempotent_hint: bool,
    /// 提示该工具会与外部世界交互（网络等）。
    pub open_world_hint: bool,
}

/// 工具来源（08 号票，§8.1「MCP 动态工具并入 ToolServer 模式」）：
/// 注册表分静态段（编译期内置 8 个）与动态段（MCP 服务器运行时注册）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolOrigin {
    /// 编译期内置工具（build_registry 的静态段）。
    Static,
    /// MCP 服务器动态注册的工具（归一名，mcp/tools.rs 维护）。
    Mcp,
}

/// 工具规格（§8.1）：注册表里的一个条目，描述工具名、能力分级与参数约束。
/// `params_schema` 为 JSON Schema 对象，约束 `additionalProperties: false`。
/// 08 号票：`name` 从 `&'static str` 放宽为 `String`（动态段归一名运行时
/// 才定，票面定案走 String；静态段构造点均为常量字符串，无额外成本）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    /// 合法字符 [a-z_][a-z0-9_]{0,63}（§8.1）。
    pub name: String,
    /// 面向模型，≤800 字符。
    pub description: String,
    /// 能力分级（决定该工具在权限矩阵中的判定路径）。
    pub capability: Capability,
    /// 参数的 JSON Schema（模型据此构造调用参数）。
    pub params_schema: serde_json::Value,
    /// 行为提示注解。
    pub annotations: ToolAnnotations,
    /// 来源段（静态/动态；plan 档过滤规则与卡片兜底渲染据此分流）。
    pub origin: ToolOrigin,
}

// ToolSpec 的构造捷径（静态段各工具与动态段共用）。
impl ToolSpec {
    /// 组装一份规格（字段逐项给出，origin 显式传入）。
    pub fn build(
        name: impl Into<String>,
        description: impl Into<String>,
        capability: Capability,
        params_schema: serde_json::Value,
        annotations: ToolAnnotations,
        origin: ToolOrigin,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            capability,
            params_schema,
            annotations,
            origin,
        }
    }
}

/// 工具结果（§3.2 ToolOutcomeJson 镜像）：一次工具调用的返回，
/// 同时承载模型通道（data/error）与 UI 通道（ui_summary/detail）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcome {
    /// 是否成功（false 时 error 有值）。
    pub ok: bool,
    /// 成功时的结构化数据（进模型文本与边车）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub data: Option<serde_json::Value>,
    /// 失败时的错误（错误码 + 人话消息）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub error: Option<ToolError>,
    /// 一行人话（UI 通道，不进模型文本）。
    pub ui_summary: String,
    /// 全量细节（toolres 边车；warnings 仅 UI 显示不进模型文本，§8.2）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub detail: Option<serde_json::Value>,
    /// 截断标记（02 号票收编：toolres 超限占位载荷此前是绕过本结构体的
    /// 手拼 JSON，字段收编后边车文件形状单一来源；正常结果恒 None 不序列化，
    /// 线上字节不变）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub truncated: Option<bool>,
}

// ToolOutcome 的构造方法：成功/失败/附带 detail。
impl ToolOutcome {
    /// 构造成功结果（data + UI 摘要）。
    pub fn success(data: serde_json::Value, ui_summary: impl Into<String>) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            ui_summary: ui_summary.into(),
            detail: None,
            truncated: None,
        }
    }

    /// 构造失败结果（错误 + UI 摘要）。
    pub fn failure(err: ToolError, ui_summary: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(err),
            ui_summary: ui_summary.into(),
            detail: None,
            truncated: None,
        }
    }

    /// 附带 detail（全量信息进边车）。
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

// 工具错误到工具结果的兜底转换。
impl From<ToolError> for ToolOutcome {
    /// 无更具体 UI 话术时的兜底转换（错误码即摘要）。
    fn from(err: ToolError) -> Self {
        let summary = if err.message.is_empty() {
            err.code.to_string()
        } else {
            format!("{}：{}", err.code, err.message)
        };
        Self::failure(err, summary)
    }
}

/// 模型通道 compact 文本（§8.2）：成功序列化 data（头 4KB + 尾 8KB 截断）；
/// 失败输出 error code+message。warnings/detail 不进模型文本。
pub fn compact_text(outcome: &ToolOutcome) -> String {
    if !outcome.ok {
        let Some(err) = &outcome.error else {
            return "unknown error".to_string();
        };
        return format!("{} {}", err.code, err.message);
    }
    let body = outcome
        .data
        .as_ref()
        .map(|d| serde_json::to_string(d).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_default();
    truncate_middle(&body, COMPACT_HEAD_BYTES, COMPACT_TAIL_BYTES)
}

/// 模型通道截断的头/尾保留字节（§8.2：头 4KB + 尾 8KB）。
/// 12 号票抽常量：web_fetch raw 模式的 HTML 截断复用同一口径。
pub const COMPACT_HEAD_BYTES: usize = 4 * 1024;
pub const COMPACT_TAIL_BYTES: usize = 8 * 1024;

/// 头 `head` + 尾 `tail` 字节截断，中间以省略标记连接。按 UTF-8 字符边界切。
/// `pub(crate)`：backend 执行器（exec.rs）的 skill 工具分支复用同一 §8.2
/// 截断口径（09 号票）。
pub(crate) fn truncate_middle(s: &str, head: usize, tail: usize) -> String {
    if s.len() <= head + tail {
        return s.to_string();
    }
    let head_cut = floor_char_boundary(s, head);
    let tail_cut_start = floor_char_boundary(s, s.len() - tail);
    format!(
        "{}…[截断 {} 字节]…{}",
        &s[..head_cut],
        s.len() - head_cut - (s.len() - tail_cut_start),
        &s[tail_cut_start..]
    )
}

/// 向下取最近 UTF-8 字符边界。
fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// 静态段工具名称与顺序钉死（§8.1：集成测试防漂移）。M1 注册表 8 个
/// （§8.3 P0 9 个中的模型可见子集；suggest 仅 run 尾调用不开放模型）。
/// 08 号票改名 STATIC_TOOL_NAMES：注册表分「静态段 + 动态段（MCP）」两段，
/// 动态段运行时可变，钉死测试改为两断言（见 mod 的 compact_test 与
/// ipc_manifest_test）。09 号票增 skill（§8.3 M2 表 Meta 行；description
/// 动态列出启用技能名清单——build_registry 每次现读技能注册表，
/// toggle/reload 后下一 run 生效）；10 号票增 subagent（§8.3 M2 表 Meta 行，
/// 子代理委派入口）：静态 10 个。12 号票增 web_fetch/http_request
/// （§8.3 M2 表 Network 两行，网页抓取与通用 HTTP）：静态 12 个。
/// 13 号票增 web_search（§8.3 M2 表 Network 行，本地优先三通道搜索）：
/// 静态 13 个。14 号票增 batch_read/glob/calculate/render_html（§8.3 M2
/// 表四行：批量读、文件名模式检索、精确算术、沙箱 HTML 预览产出）：
/// 静态 17 个。
pub const STATIC_TOOL_NAMES: [&str; 17] = [
    "read",
    "write",
    "edit",
    "list_files",
    "grep",
    "command",
    "plan",
    "ask",
    "skill",
    "subagent",
    "web_fetch",
    "http_request",
    "web_search",
    "batch_read",
    "glob",
    "calculate",
    "render_html",
];

/// 静态注册表（§8.1：启动时构建）。顺序与 [`STATIC_TOOL_NAMES`] 一致。
pub fn build_registry() -> Vec<ToolSpec> {
    vec![
        read::spec(),
        write::spec(),
        edit::spec(),
        list_files::spec(),
        grep::spec(),
        command::spec(),
        plan::spec(),
        ask::spec(),
        crate::knowledge::skills::skill_spec(),
        crate::agent::roles::subagent_spec(),
        web_fetch::spec(),
        http_request::spec(),
        web_search::spec(),
        batch_read::spec(),
        glob::spec(),
        calculate::spec(),
        render_html::spec(),
    ]
}

/// 全表查询（§8.1「三处同源过滤」的查找面，08 号票）：静态段 + MCP 动态段
/// （进程级 [`crate::mcp::tools::McpToolRegistry`] 单例快照）。动态段未装配
/// 时等价于静态表。主循环策略门（is_interactive）、看门狗只读判定、审批门
/// 与执行器 dispatch 都经本函数，避免各自直连 MCP 模块。
pub fn lookup_spec(name: &str) -> Option<ToolSpec> {
    if let Some(spec) = build_registry().into_iter().find(|s| s.name == name) {
        return Some(spec);
    }
    crate::mcp::tools::McpToolRegistry::global()
        .snapshot()
        .into_iter()
        .find(|s| s.name == name)
}

/// 三处同源过滤的单一实现（§8.1，FR-05-01）：按档位过滤工具表。
/// 08 号票增 MCP 规则（§8.5 注）：plan 档 MCP 动态工具全过滤（不出现在
/// 工具表）；其余档位保留，能力矩阵照旧——MCP 未声明 read_only 的审批在
/// 门控执行（§9.7），不影响是否暴露给模型。
pub fn filter_for_tier(specs: Vec<ToolSpec>, tier: PermissionTier) -> Vec<ToolSpec> {
    specs
        .into_iter()
        .filter(|spec| {
            if tier == PermissionTier::Plan && spec.origin == ToolOrigin::Mcp {
                return false;
            }
            decide_gate(spec.capability, tier) != GateDecision::Filtered
        })
        .collect()
}

#[cfg(test)]
#[path = "ask_test.rs"]
mod ask_test;

#[cfg(test)]
#[path = "command_test.rs"]
mod command_test;

#[cfg(test)]
#[path = "compact_test.rs"]
mod compact_test;

#[cfg(test)]
#[path = "http_request_test.rs"]
mod http_request_test;

#[cfg(test)]
#[path = "plan_test.rs"]
mod plan_test;

#[cfg(test)]
#[path = "web_fetch_test.rs"]
mod web_fetch_test;

#[cfg(test)]
#[path = "web_search_test.rs"]
mod web_search_test;
