/**
 * IPC 契约镜像（§2.4 ipc/types.ts）：与 Rust 侧契约类型"逐字段照抄"的 TS 副本
 * （D14 命名纪律：两侧类型同名同结构，方便逐个对照；wire 指线上传输格式，
 * 即数据跨进程传输时的形态）。
 *
 * - 存储条目/WirePart 镜像 src-tauri/src/storage/entries.rs；
 * - 会话树命令负载镜像 src-tauri/src/host/commands/session_tree.rs；
 * - provider/settings 域（CW schema v2）镜像 src-tauri/src/providers/types.rs；
 * - 契约一致性由 §12.3 manifest + 跨语言 fixture 校验（ui/src/ipc/contract.test.ts；
 *   fixture 指测试用的固定样例数据文件，由 cargo test 产出，运行前端测试前
 *   先跑 cargo test）。
 */

import type { MessageStatus } from "@assistant-ui/react";

// ---- §3.2 WirePart（M1 子集；image 属 M2）----

/** 消息角色：用户 / 助手 / 系统。 */
export type MessageRole = "user" | "assistant" | "system";

/** 计划条目：模型执行计划中的一步，带状态（待办/进行中/已完成）。 */
export type PlanItem = {
  id: string;
  text: string;
  status: "pending" | "in_progress" | "completed";
};

/** 消息内容分片（线上传输形态）：一段文本/思考/工具调用/工具结果/计划快照/run 标记，
 *  一条消息由若干分片按顺序组成。 */
export type WirePart =
  | { type: "text"; text: string }
  | { type: "reasoning"; text: string }
  | { type: "toolCall"; callId: string; name: string; argsJson: string }
  | { type: "toolResult"; callId: string; ok: boolean; summary: string }
  | { type: "planSnapshot"; items: PlanItem[] }
  | {
      type: "runMarker";
      runId: string;
      startedAt: number;
      endedAt?: number;
      durationMs?: number;
      toolCount?: number;
    };

// ---- §6.1 存储条目（serde tag=kind 的 TS 镜像）----

/** message 条目载荷：同时是 save_session_tree 的 messages 元素形态（§6.3，parentId null→省略） */
export type MessageEntry = {
  id: string;
  parentId?: string;
  role: MessageRole;
  parts: WirePart[];
  createdAt: number;
  runId?: string;
  /** S4 增补（§6.1）：仅 assistant，保留导出时的 MessageStatus（M1 只落 complete） */
  status?: MessageStatus;
};

/** 落盘后的 message 条目：MessageEntry 加上 kind 判别字段（serde tag=kind）。 */
export type StoredMessageEntry = MessageEntry & { kind: "message" };

/** 落盘后的 meta 条目：会话级元信息（活跃叶子节点、标题、版本号）。 */
export type StoredMetaEntry = {
  kind: "meta";
  activeLeafId?: string;
  title?: string;
  revision: number;
};

/** 落盘后的 session 条目：会话头（结构版本 v=1、会话 id、创建时间）。 */
export type StoredSessionEntry = {
  kind: "session";
  v: 1;
  id: string;
  createdAt: number;
};

/** 存储条目全集：会话 JSONL 文件的每一行是其中一种（判别字段 kind）。 */
export type StoredEntry = StoredSessionEntry | StoredMessageEntry | StoredMetaEntry;

// ---- 会话树命令（§3.3 #26 / §6.3 / §6.4）----

/** save_session_tree 参数（命令形参平铺，本结构为文档命名锚点） */
export type SaveSessionTreeArgs = {
  sessionId: string;
  messages: MessageEntry[];
  activeLeafId?: string;
  baseRevision: number;
};

/** save_session_tree 返回 */
export type SaveSessionTreeResult = { revision: number };

/** load_session_tree 返回（§6.1 定稿结构：headId=activeLeafId + messages 含 status?） */
export type SessionTreeData = {
  sessionId: string;
  messages: MessageEntry[];
  activeLeafId?: string;
  revision: number;
};

// ---- 会话索引与 toolres 边车（§3.3 #14/#29，15 号票）----

/** §3.2 SessionMeta（index.json 元素与 list_sessions 返回） */
export interface SessionMeta {
  id: string;
  projectId?: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  revision: number;
  messageCount: number;
  totalTokens: number;
}

// ---- 项目注册表与 workspace 只读（§3.3 #10–12/#27/#28，16 号票）----

/** §3.2 ProjectMeta（directories.json 元素与 list_projects 返回） */
export interface ProjectMeta {
  id: string;
  name: string;
  homeDir: string;
  createdAt: number;
}

/** §3.3 #11 save_project 参数（新建/更新） */
export type SaveProjectArgs = { project: { id?: string; name: string; homeDir: string } };

/** §3.3 #27 search_workspace_paths 命中条目 */
export type WorkspacePathHit = { path: string; kind: "file" | "dir" };

/** §3.3 #28 read_workspace_file 行区间 */
export type WorkspaceReadRange = { offset: number; limit: number };

/** §3.3 #28 read_workspace_file 返回 */
export type ReadWorkspaceResult = { content: string; truncated: boolean };

/** load_tool_outcomes 返回（边车 ToolOutcome 全量 JSON；缺文件调用不在表中） */
export type ToolOutcomeJson = {
  ok: boolean;
  data?: unknown;
  error?: { code: string; message: string };
  uiSummary: string;
  detail?: unknown;
  truncated?: boolean;
};

// ---- 供应商（CW schema v2，实现 FR-02-01/06/07）----

/** API 格式（请求协议）：决定请求体怎么拼、响应怎么解析，对应不同服务商的接口风格。 */
export type ApiFormat = "openai_chat" | "anthropic_messages" | "openai_responses";

/** 推理力度档位：控制模型思考的深浅；未设置时用服务端默认。 */
export type ReasoningEffort = "default" | "low" | "medium" | "high" | "max";

/** 供应商名下的一个模型定义（前端手工配置，无内置目录）。 */
export interface ProviderModel {
  /** uuid，activeModelId 指向它 */
  id: string;
  /** wire id，即显示名 */
  model: string;
  /** default 32768 */
  maxTokens: number;
  /** default 128000 */
  contextWindow: number;
  reasoningEffort?: ReasoningEffort;
  vision?: boolean;
}

/** 一个模型供应商的完整配置：端点 + 凭据 + 模型列表。 */
export interface ProviderConfig {
  /** uuid */
  id: string;
  name: string;
  apiFormat: ApiFormat;
  /** 缺省按 apiFormat 用官方缺省地址；自定义 OpenAI 兼容端点在此覆写 */
  baseUrl?: string;
  /** API Key 明文（用户决策废除 keyring：存 config.json，文件权限 0600） */
  apiKey?: string;
  models: ProviderModel[];
  headers?: Record<string, string>;
}

// ---- 应用配置（实现 FR-14）----

/** 代理模式：不走代理 / 跟随系统设置 / 手动指定地址。 */
export type ProxyMode = "none" | "system" | "manual";

/** 网络代理配置（manual 模式下 url 必填）。 */
export interface ProxyConfig {
  mode: ProxyMode;
  url?: string;
}

/** 审批策略：敏感操作（写文件、执行命令等）是否先询问用户再执行。 */
export interface ApprovalConfig {
  enabled: boolean;
  autoConfirm: boolean;
  commandAllowlist: string[];
}

/** 主题三态：浅色 / 深色 / 跟随系统（与 theme.ts 的 ThemeMode 同口径）。 */
export type ThemeName = "light" | "dark" | "system";

/** 界面显示语言。 */
export type Language = "zh-CN" | "en-US";

/** 界面偏好（显示语言、主题、AI 回复语言）。 */
export interface UiConfig {
  language: Language;
  theme: ThemeName;
  aiLanguage?: string;
  fontSize?: number;
}

/** 日志级别（由低到高：debug < info < warn < error，高级别包含低级别）。 */
export type LogLevel = "debug" | "info" | "warn" | "error";

/** 日志配置。 */
export interface LogConfig {
  level: LogLevel;
}

/** 系统通知偏好（FR-13-06 设置项，17 号票；Rust 侧 NotificationConfig 镜像）。 */
export interface NotificationConfig {
  enabled: boolean;
  interventionOnly: boolean;
}

/** 通知授权态（§9.6，19 号票；notification_permission_status 返回镜像）。
 *  granted 可送达 / denied 被拒 / prompt 未决定 / unknown 查不出（dev 裸进程）。 */
export type NotificationPermission = "granted" | "denied" | "prompt" | "unknown";

/** notification_permission_status 返回：授权态 + 平台是否有系统设置入口。 */
export interface NotificationPermissionStatus {
  status: NotificationPermission;
  hasSettingsEntry: boolean;
}

/** 应用配置根：config.json 的完整结构（schemaVersion=2）。 */
export interface AppConfig {
  schemaVersion: 2;
  providers: ProviderConfig[];
  activeModelId?: string;
  proxy?: ProxyConfig;
  approval: ApprovalConfig;
  ui: UiConfig;
  log: LogConfig;
  notifications?: NotificationConfig;
}

// ---- provider/config 命令锚点（§3.3 #6–9，镜像 host/commands/{config,provider}.rs）----

/** save_config 参数（命令形参平铺，本结构为文档命名锚点） */
export type SaveConfigArgs = { config: AppConfig };

/** save_provider 参数（newKey 有值时走 keyring 提交协议 §9.4） */
export type SaveProviderArgs = { provider: ProviderConfig; newKey?: string };

/** delete_provider 参数 */
export type DeleteProviderArgs = { providerId: string };

// ---- 运行配置与用量（§3.2；start_chat，13 号票）----

/** 权限四档（§8.5 矩阵；serde camelCase 口径，与 Rust safety/tier.rs 一致） */
export type PermissionTier = "plan" | "confirmEach" | "autoEdit" | "fullAccess";

/** §3.2 RunConfig */
export interface RunConfig {
  /** 会话级覆盖；缺省全局 active */
  modelId?: string;
  tier: PermissionTier;
  /** default 24 */
  maxTurns: number;
  /** 本次发送是否为 steer 注入（M1 前端恒 false，§5.5） */
  steer: boolean;
}

/** §3.2 UsageTotal（usage 帧/事件载荷）：token 用量汇总（输入/输出/缓存读写）。 */
export interface UsageTotal {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

/** start_chat 参数（命令形参平铺，本结构为文档命名锚点） */
export type StartChatArgs = {
  sessionId: string;
  messages: MessageEntry[];
  runConfig: RunConfig;
  /** tauri ipc Channel<ChatFrame>（@tauri-apps/api/core 的 Channel 实例） */
  channel: unknown;
};

/** start_chat 返回 */
export type StartChatResult = { runId: string };

// ---- §3.4 Channel 帧（P0 全集 9 帧；Rust 镜像 src-tauri/src/backend/frames.rs）----

/** 帧基座：所有帧带 Rust 端递增的 seq；ts = Rust 出口 epoch ms（可选，S2 实证）。 */
type Sequenced = {
  seq: number;
  ts?: number;
};

/** run 期间 Channel 推送的全部帧型（判别字段 type，P0 全集 9 帧）。 */
export type ChatFrame =
  | (Sequenced & { type: "runStarted"; runId: string; model: string })
  | (Sequenced & { type: "stepStarted"; turn: number })
  | (Sequenced & { type: "textDelta"; turn: number; delta: string })
  | (Sequenced & { type: "reasoningDelta"; turn: number; delta: string })
  | (Sequenced & { type: "toolStarted"; callId: string; name: string })
  | (Sequenced & { type: "toolArgs"; callId: string; chunk: string })
  | (Sequenced & {
      type: "toolResult";
      callId: string;
      ok: boolean;
      uiSummary: string;
      error?: { code: string; message: string };
    })
  | (Sequenced & { type: "usage"; usage: UsageTotal; rate?: number })
  | (Sequenced & {
      type: "runEnded";
      reason: "done" | "error" | "cancelled";
      durationMs: number;
      toolCount: number;
      turn: number;
      error?: { code: string; message: string };
    });

// ---- D14 统一错误形状（§3.1：命令失败统一 reject {code, message, detail?}）----

/** 统一错误负载（D14）：命令失败一律 reject 此结构，code 见 §3.1 错误码表。 */
export type IpcError = {
  code: string;
  message: string;
  detail?: unknown;
};

// ---- 询问/审批（§3.2 AskPayload/AskAnswer，14 号票；镜像 backend/ask.rs）----

/** 询问问题的一个候选选项。 */
export type AskChoice = { id: string; label: string };

/** 询问面板的一问：可带候选选项，可允许自由输入文本。 */
export type AskQuestion = {
  id: string;
  text: string;
  options?: AskChoice[];
  allowFreeText: boolean;
};

/** diff（差异对比）中的一行：新增 / 删除 / 上下文（未变的对照行）。 */
export type DiffLine = { kind: "add" | "del" | "ctx"; text: string };

/** diff 的一个 hunk（连续变更块）：头行说明位置，后跟各行。 */
export type DiffHunk = { header: string; lines: DiffLine[] };

/** 审批载荷携带的文件 diff：目标路径 + 变更块列表。 */
export type ApprovalDiff = { path: string; hunks: DiffHunk[] };

/** 审批选项：可带推荐标记与授权模式（本次允许 / 本会话总允许 / 拒绝）。 */
export type AskOption = {
  id: string;
  label: string;
  recommended?: boolean;
  mode?: "allow_once" | "always_session" | "deny";
};

/** §3.2 AskPayload（M1 实装 question/approval 两形态） */
export type AskPayload =
  | { kind: "question"; title: string; questions: AskQuestion[] }
  | {
      kind: "approval";
      title: string;
      diff?: ApprovalDiff;
      command?: string;
      fenceReason?: string;
      options: AskOption[];
    };

/** §3.2 AskAnswer */
export type AskAnswer =
  | { kind: "question"; answers: Record<string, string> }
  | { kind: "approval"; optionId: string; editedCommand?: string };

/** resolve_ask 参数（命令形参平铺，本结构为文档命名锚点） */
export type ResolveAskArgs = { askId: string; answer: AskAnswer };

/** approve_plan 参数（§3.3 #26c，14 号票增补） */
export type ApprovePlanArgs = { sessionId: string };

// ---- §3.5 事件载荷（14 号票实装 5 键；镜像 util/events.rs 与 host 发射面）----

/** ask:opened 事件载荷：后端打开了询问/审批，前端应弹出对应面板。 */
export type AskOpenedEvent = {
  sessionId: string;
  askId: string;
  payload: AskPayload;
};

/** ask:closed 事件载荷：询问被关闭（answered = 是否已有应答）。 */
export type AskClosedEvent = {
  askId: string;
  answered: boolean;
};

/** plan:update 事件载荷：计划条目状态变化（计划卡实时刷新）。 */
export type PlanUpdateEvent = {
  sessionId: string;
  items: PlanItem[];
};

/** run:suggestions 事件载荷：run 结束后给出的建议提示词（输入框 ghost 预填）。 */
export type RunSuggestionsEvent = {
  sessionId: string;
  runId: string;
  text: string;
};

/** tokens:update 事件载荷：会话累计 token 用量的权威校正（覆盖前端累计值）。 */
export type TokensUpdateEvent = {
  sessionId: string;
  usage: UsageTotal;
};
