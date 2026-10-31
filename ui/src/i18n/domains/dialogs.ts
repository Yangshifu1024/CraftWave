/**
 * dialogs 域字典：询问/审批面板（AskPanel 五形态——question/
 * approval/mcpTrust/projectTrust/externalDir）的全部用户可见文案。选项按钮
 * label 来自后端载荷（opt.label，不前端翻译——后端按 ai_language 口径），
 * 前端只承载表单 chrome 文案。
 */

const zh = {
  // ---- 共享 ----
  freeTextPlaceholder: "自由输入…",
  submitAnswer: "提交回答",
  submitFailedRetry: "应答提交失败：{{error}}（可重试）",
  submitFailedRetryStop: "应答提交失败：{{error}}（可重试；若反复失败请停止该任务）",
  submitFailedDefault: "应答提交失败",
  // ---- approval ----
  auditManualNote: "审计不可用，已转人工确认（{{reason}}）",
  auditAskNote: "模型审计建议人工确认：{{reason}}",
  planScopeNote: "计划外路径：{{path}}",
  collapsePreview: "收起预览",
  viewPreview: "查看预览",
  addToAllowlist: "把命令前缀加入白名单（此后同前缀命令全局免审批）",
  // ---- mcpTrust ----
  mcpTrustTitle: "信任并连接 MCP 服务器 {{server}}？",
  mcpTransport: "传输方式：",
  mcpTransportStdio: "stdio 子进程",
  mcpTransportHttp: "streamable HTTP",
  fingerprint: "配置指纹：",
  mcpTrustExplain:
    "首次连接需要确认服务器身份；「记住此版本」后同指纹不再询问，配置被改动会重新确认。",
  // ---- projectTrust ----
  skillTrustTitle: "信任技能 {{name}}？",
  skillSource: "来源：",
  skillSourceGlobal: "全局技能目录",
  skillSourceProject: "项目技能目录",
  versionFingerprint: "版本指纹：",
  skillTrustExplain:
    "技能包含说明与随附脚本、模板等全部文件；「记住此版本」后同版本不再询问，内容变更会重新确认。拒绝后本次运行不加载该技能。",
  // ---- externalDir ----
  externalDirTitle: "允许访问项目外的目录？",
  externalDirExplain:
    "授权粒度：该目录及子目录的读/写/列。授权仅本会话有效（可在 Inspector「授权」页移除）；项目内的目录不需要授权。",
};

/** en 全量翻译：键集合编译期同构。 */
const en: Record<keyof typeof zh, string> = {
  freeTextPlaceholder: "Free text…",
  submitAnswer: "Submit answer",
  submitFailedRetry: "Failed to submit: {{error}} (retryable)",
  submitFailedRetryStop:
    "Failed to submit: {{error}} (retryable; stop the task if this keeps failing)",
  submitFailedDefault: "Failed to submit",
  auditManualNote: "Audit unavailable, escalated to human confirmation ({{reason}})",
  auditAskNote: "Model audit suggests human confirmation: {{reason}}",
  planScopeNote: "Out-of-plan path: {{path}}",
  collapsePreview: "Collapse preview",
  viewPreview: "View preview",
  addToAllowlist:
    "Add the command prefix to the allowlist (future commands with this prefix skip approval globally)",
  mcpTrustTitle: "Trust and connect to MCP server {{server}}?",
  mcpTransport: "Transport: ",
  mcpTransportStdio: "stdio subprocess",
  mcpTransportHttp: "streamable HTTP",
  fingerprint: "Config fingerprint: ",
  mcpTrustExplain:
    'First connections verify the server identity; after "remember this version", the same fingerprint is not asked again. Config changes ask again.',
  skillTrustTitle: "Trust skill {{name}}?",
  skillSource: "Source: ",
  skillSourceGlobal: "global skills directory",
  skillSourceProject: "project skills directory",
  versionFingerprint: "Version fingerprint: ",
  skillTrustExplain:
    'A skill includes its instructions plus all accompanying scripts, templates, and files; after "remember this version", the same version is not asked again. Content changes ask again. If denied, the skill is not loaded for this run.',
  externalDirTitle: "Allow access to a directory outside the project?",
  externalDirExplain:
    'Grant scope: read/write/list of this directory and its subdirectories. The grant lasts for this session only (removable from the Inspector "Grants" tab); directories inside the project need no grant.',
};

export const dialogsDomain = { name: "dialogs", zh, en };
