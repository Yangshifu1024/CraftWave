/**
 * errors 域字典（31 号票，§3.6 错误面口径定案）：错误码 → 用户可见本地化
 * 文案。后端 `E_*` 错误码机器面不变、message 归一为英文技术串；前端展示
 * 层按错误码经 tError(code, fallbackMessage) 取本域映射——映射缺失时
 * 回退原始 message 不空白（unknown code / 后端新增码的兜底）。
 *
 * 键名 = §3.6 错误码注册表的码值原样（provider 七类子码逐个列出）。
 */

const zh = {
  // 通用
  E_INVALID_ARG: "参数不合法",
  E_NOT_FOUND: "目标不存在",
  E_INTERNAL: "内部错误",
  E_IO: "文件或目录操作失败",
  // 会话
  E_SESSION_RUNNING: "会话正在运行中",
  E_SESSION_REV: "会话内容已在别处更新，请刷新后重试",
  E_CANCELLED: "操作已取消",
  E_CONTEXT_OVERFLOW: "上下文超出模型窗口且压缩后仍放不下，已暂停并保留输入",
  E_STORAGE_IO: "本地存储写入失败",
  // 供应商（FR-03-04 七类）
  E_PROVIDER_AUTH: "鉴权失败：请检查 API Key",
  E_PROVIDER_BILLING: "账户额度或计费问题",
  E_PROVIDER_RATE_LIMITED: "请求过于频繁，已触发限流",
  E_PROVIDER_BAD_REQUEST: "请求参数不被供应商接受",
  E_PROVIDER_SERVER: "供应商服务端错误",
  E_PROVIDER_PROTOCOL: "供应商响应格式异常",
  E_PROVIDER_NETWORK: "网络连接失败",
  // 安全
  E_INTERNAL_STATE: "该目标是应用内部状态，禁止写入",
  E_PATH_OUTSIDE_ROOTS: "路径超出授权范围",
  E_DELETE_GUARDED: "删除被守卫拦截",
  E_PROJECT_UNTRUSTED: "项目资源尚未信任",
  E_SSRF_BLOCKED: "目标地址被网络安全防护拦截",
  // 计划门
  E_PLAN_STALE: "已批准的计划已过期，请更新计划后再执行",
  E_PLAN_REQUIRED: "严格计划模式已开启：请先制定并批准计划",
  // 工具
  E_TOOL_BLOCKED: "当前权限档不允许该工具",
  E_FENCE_BLOCKED: "命令被安全围栏拦截",
  E_AUDIT_BLOCKED: "命令经安全审计判定为高风险，已拒绝执行",
  E_EDIT_NO_MATCH: "编辑目标内容未在文件中匹配到",
  E_EDIT_AMBIGUOUS: "编辑目标在文件中匹配到多处",
  E_EDIT_VERSION_STALE: "文件已被修改，编辑基于的内容已过期",
  E_COMMAND_TIMEOUT: "命令执行超时",
  E_TOOL_SKIPPED: "该调用未执行（同批存在待回答询问或写冲突）",
  E_SEARCH_UNAVAILABLE: "网络搜索全部通道不可用",
  // 运行时
  E_RUN_STALLED: "模型长时间无响应，运行已终止",
  // MCP
  E_MCP_DISCONNECTED: "MCP 服务器未连接",
  E_MCP_RESULT_UNKNOWN: "MCP 调用结果未知（连接中断），请先核查实际结果",
  // 子代理
  E_SUB_BUDGET: "子代理步数预算已耗尽",
};

/** en 全量翻译：键集合编译期同构。 */
const en: Record<keyof typeof zh, string> = {
  E_INVALID_ARG: "Invalid argument",
  E_NOT_FOUND: "Not found",
  E_INTERNAL: "Internal error",
  E_IO: "File or directory operation failed",
  E_SESSION_RUNNING: "Session is already running",
  E_SESSION_REV: "Session was updated elsewhere; refresh and retry",
  E_CANCELLED: "Operation cancelled",
  E_CONTEXT_OVERFLOW:
    "Context exceeded the model window and does not fit even after compaction; paused with input preserved",
  E_STORAGE_IO: "Failed to write local storage",
  E_PROVIDER_AUTH: "Authentication failed: check your API key",
  E_PROVIDER_BILLING: "Account quota or billing issue",
  E_PROVIDER_RATE_LIMITED: "Rate limited by the provider",
  E_PROVIDER_BAD_REQUEST: "Request rejected by the provider",
  E_PROVIDER_SERVER: "Provider server error",
  E_PROVIDER_PROTOCOL: "Unexpected provider response format",
  E_PROVIDER_NETWORK: "Network connection failed",
  E_INTERNAL_STATE: "This target is application-internal state and cannot be written",
  E_PATH_OUTSIDE_ROOTS: "Path is outside the authorized scope",
  E_DELETE_GUARDED: "Deletion blocked by a guard",
  E_PROJECT_UNTRUSTED: "Project resource is not trusted yet",
  E_SSRF_BLOCKED: "Target address blocked by network protection",
  E_PLAN_STALE: "The approved plan has expired; update the plan before writing",
  E_PLAN_REQUIRED: "Strict plan mode is on: create and approve a plan first",
  E_TOOL_BLOCKED: "The current permission tier does not allow this tool",
  E_FENCE_BLOCKED: "Command blocked by the safety fence",
  E_AUDIT_BLOCKED: "Command rejected by the security auditor as high risk",
  E_EDIT_NO_MATCH: "The edit target was not found in the file",
  E_EDIT_AMBIGUOUS: "The edit target matches multiple locations in the file",
  E_EDIT_VERSION_STALE: "The file has been modified; the edit was based on outdated content",
  E_COMMAND_TIMEOUT: "Command timed out",
  E_TOOL_SKIPPED: "Not executed (a pending question or write conflict exists in this batch)",
  E_SEARCH_UNAVAILABLE: "All web search channels are unavailable",
  E_RUN_STALLED: "Model stopped responding; the run was terminated",
  E_MCP_DISCONNECTED: "MCP server is not connected",
  E_MCP_RESULT_UNKNOWN: "MCP call result unknown (connection lost); verify the actual result first",
  E_SUB_BUDGET: "Subagent step budget exhausted",
};

export const errorsDomain = { name: "errors", zh, en };
