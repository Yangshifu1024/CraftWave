/**
 * IPC 契约镜像（§2.4 ipc/types.ts：与 Rust 契约逐一镜像，D14 命名纪律）。
 *
 * - 存储条目/WirePart 镜像 src-tauri/src/storage/entries.rs；
 * - 会话树命令负载镜像 src-tauri/src/host/commands/session_tree.rs；
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

// ---- D14 统一错误形状 ----

export type IpcError = {
  code: string;
  message: string;
  detail?: unknown;
};
