/**
 * IPC 契约镜像（§2.4 ipc/types.ts：与 Rust 契约逐一镜像，D14 命名纪律）。
 *
 * - 存储条目/WirePart 镜像 src-tauri/src/storage/entries.rs；
 * - 会话树命令负载镜像 src-tauri/src/host/commands/session_tree.rs；
 * - provider/settings 域（CW schema v2）镜像 src-tauri/src/providers/types.rs；
 * - 契约一致性由 §12.3 manifest + 跨语言 fixture 校验（ui/src/ipc/contract.test.ts，
 *   fixture 由 cargo test 产出，运行前端测试前先跑 cargo test）。
 */

import type { MessageStatus } from "@assistant-ui/react";

// ---- §3.2 WirePart（M1 子集；image 属 M2）----

export type MessageRole = "user" | "assistant" | "system";

export type PlanItem = {
  id: string;
  text: string;
  status: "pending" | "in_progress" | "completed";
};

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

export type StoredMessageEntry = MessageEntry & { kind: "message" };

export type StoredMetaEntry = {
  kind: "meta";
  activeLeafId?: string;
  title?: string;
  revision: number;
};

export type StoredSessionEntry = {
  kind: "session";
  v: 1;
  id: string;
  createdAt: number;
};

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

// ---- 供应商（CW schema v2，实现 FR-02-01/06/07）----

export type ApiFormat = "openai_chat" | "anthropic_messages" | "openai_responses";

export type ReasoningEffort = "default" | "low" | "medium" | "high" | "max";

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

export type ProxyMode = "none" | "system" | "manual";

export interface ProxyConfig {
  mode: ProxyMode;
  url?: string;
}

export interface ApprovalConfig {
  enabled: boolean;
  autoConfirm: boolean;
  commandAllowlist: string[];
}

export type ThemeName = "light" | "dark" | "system";

export type Language = "zh-CN" | "en-US";

export interface UiConfig {
  language: Language;
  theme: ThemeName;
  aiLanguage?: string;
}

export type LogLevel = "debug" | "info" | "warn" | "error";

export interface LogConfig {
  level: LogLevel;
}

export interface AppConfig {
  schemaVersion: 2;
  providers: ProviderConfig[];
  activeModelId?: string;
  proxy?: ProxyConfig;
  approval: ApprovalConfig;
  ui: UiConfig;
  log: LogConfig;
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

/** §3.2 UsageTotal（usage 帧载荷） */
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

export type IpcError = {
  code: string;
  message: string;
  detail?: unknown;
};

// ---- 询问/审批（§3.2 AskPayload/AskAnswer，14 号票；镜像 backend/ask.rs）----

export type AskChoice = { id: string; label: string };

export type AskQuestion = {
  id: string;
  text: string;
  options?: AskChoice[];
  allowFreeText: boolean;
};

export type DiffLine = { kind: "add" | "del" | "ctx"; text: string };

export type DiffHunk = { header: string; lines: DiffLine[] };

export type ApprovalDiff = { path: string; hunks: DiffHunk[] };

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

export type AskOpenedEvent = {
  sessionId: string;
  askId: string;
  payload: AskPayload;
};

export type AskClosedEvent = {
  askId: string;
  answered: boolean;
};

export type PlanUpdateEvent = {
  sessionId: string;
  items: PlanItem[];
};

export type RunSuggestionsEvent = {
  sessionId: string;
  runId: string;
  text: string;
};

export type TokensUpdateEvent = {
  sessionId: string;
  usage: UsageTotal;
};
