/**
 * IPC 契约镜像（§2.4 ipc/types.ts：与 Rust 契约逐一镜像，D14 命名纪律）。
 *
 * 本文件承接自 S4 spike（07 号票）的 §6.1 存储条目类型；契约一致性由
 * §12.3 manifest + 跨语言 fixture 校验（ui/src/ipc/contract.test.ts）。
 * parts 采用 §3.2 WirePart 中立格式（camelCase：toolCall/toolResult）。
 */

import type { MessageStatus } from "@assistant-ui/react";

// ---- §3.2 WirePart（M1 子集；image 属 M2）----

export type WirePart =
  | { type: "text"; text: string }
  | { type: "reasoning"; text: string }
  | { type: "toolCall"; callId: string; name: string; argsJson: string }
  | { type: "toolResult"; callId: string; ok: boolean; summary: string };

// ---- §6.1 存储条目（serde tag=kind 的 TS 镜像）----

export type StoredMessageEntry = {
  kind: "message";
  id: string;
  parentId?: string;
  role: "user" | "assistant" | "system";
  parts: WirePart[];
  createdAt: number;
  runId?: string;
  /** S4 增补（§6.1）：仅 assistant，保留导出时的 MessageStatus（M1 只落 complete） */
  status?: MessageStatus;
};

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
