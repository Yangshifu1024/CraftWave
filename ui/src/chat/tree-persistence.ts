import type { ThreadMessage, ThreadMessageLike } from "@assistant-ui/react";
import { ExportedMessageRepository } from "@assistant-ui/react";
import type { StoredEntry, StoredMessageEntry, StoredMetaEntry, WirePart } from "../ipc/types";

/** ThreadMessageLike 的 content 数组形态（core 未单独导出 part 类型，从此推导） */
type MessageLikeContent = Exclude<ThreadMessageLike["content"], string>;

/**
 * 前端树 ↔ §6.1 存储条目双向转换（D8 载体，§6.3 单一事实源）。
 *
 * 设计对齐：
 * - 存储条目 schema 见技术设计文档 §6.1（M1 子集：session/message/meta；
 *   compaction、branchSummary 属 M2，暂不实现）。
 * - 存储条目与 WirePart 类型是 Rust 契约镜像，单一事实源在 ui/src/ipc/types.ts（§2.4）。
 * - 条目内容不可变：编辑/重生成/分支一律追加新节点，本层是纯函数，无副作用。
 *
 * 落盘由 Rust storage 模块承载（append-only 分段 JSONL）；
 * 本层条目数组逐行 `JSON.stringify` 即为 JSONL 行（serializeSessionJsonl，契约测试用）。
 */

// ---- 导出 → 存储条目 ----

/** 从 assistant 消息单个 part 提取工具结果摘要（toolResult.summary 口径：纯文本拼接） */
function summarizeToolResult(result: unknown): string {
  if (result === undefined) return "";
  if (typeof result === "string") return result;
  if (result === null || typeof result !== "object") return String(result);
  const obj = result as Record<string, unknown>;
  // assistant-ui 镜像工具的常见结果形态 { type: "text", ... } 或数组
  const extract = (value: unknown): string => {
    if (typeof value === "string") return value;
    if (Array.isArray(value)) return value.map(extract).join("");
    if (value && typeof value === "object") {
      const rec = value as Record<string, unknown>;
      if (typeof rec.text === "string") return rec.text;
      if (rec.content !== undefined) return extract(rec.content);
    }
    return "";
  };
  if (obj.content !== undefined) return extract(obj.content);
  return JSON.stringify(result);
}

function partToWire(
  part: ThreadMessage["content"][number],
  role: ThreadMessage["role"],
): WirePart[] {
  switch (part.type) {
    case "text":
      return [{ type: "text", text: part.text }];
    case "reasoning":
      return [{ type: "reasoning", text: part.text }];
    case "tool-call": {
      // assistant-ui 的工具调用+结果在一个 part 里；§6.1 拆为 toolCall/toolResult 两条
      const wires: WirePart[] = [
        {
          type: "toolCall",
          callId: part.toolCallId,
          name: part.toolName,
          argsJson: JSON.stringify(part.args ?? {}),
        },
      ];
      if (part.result !== undefined) {
        wires.push({
          type: "toolResult",
          callId: part.toolCallId,
          ok: !part.isError,
          summary: summarizeToolResult(part.result),
        });
      }
      return wires;
    }
    default:
      // source/file/image/data/generative-ui 等不在 M1 范围：fail fast 防静默丢失
      throw new Error(
        `tree-persistence：role=${role} 的 part 类型 ${part.type} 不在 spike 映射范围`,
      );
  }
}

export function messageToEntry(
  message: ThreadMessage,
  parentId: string | null,
): StoredMessageEntry {
  const entry: StoredMessageEntry = {
    kind: "message",
    id: message.id,
    role: message.role,
    parts: message.content.flatMap((part) => partToWire(part, message.role)),
    createdAt: message.createdAt.getTime(),
  };
  if (parentId !== null) entry.parentId = parentId;
  if (message.role === "assistant") {
    entry.status = message.status;
    if (message.metadata.runId !== undefined) entry.runId = message.metadata.runId;
  }
  return entry;
}

export function exportToEntries(
  repo: ExportedMessageRepository,
  sessionId: string,
  revision: number,
  now = Date.now(),
): StoredEntry[] {
  const entries: StoredEntry[] = [{ kind: "session", v: 1, id: sessionId, createdAt: now }];
  for (const item of repo.messages) {
    entries.push(messageToEntry(item.message, item.parentId));
  }
  const meta: StoredMetaEntry = { kind: "meta", revision };
  if (repo.headId != null) meta.activeLeafId = repo.headId;
  entries.push(meta);
  return entries;
}

// ---- 存储条目 → 导出（还原） ----

function toolResultText(
  parts: WirePart[],
  callId: string,
): { ok: boolean; summary: string } | undefined {
  const found = parts.find(
    (p): p is Extract<WirePart, { type: "toolResult" }> =>
      p.type === "toolResult" && p.callId === callId,
  );
  return found ? { ok: found.ok, summary: found.summary } : undefined;
}

function entryToMessageLike(entry: StoredMessageEntry): ThreadMessageLike {
  // 按 ThreadMessageLike part 形状构造 plain 对象；JSON.parse 的 args 为 unknown，
  // 因此整体出口处一次断言（fromBranchableArray 内部还会再做规范化校验）
  const rawParts: Record<string, unknown>[] = [];
  for (const part of entry.parts) {
    switch (part.type) {
      case "text":
        rawParts.push({ type: "text", text: part.text });
        break;
      case "reasoning":
        rawParts.push({ type: "reasoning", text: part.text });
        break;
      case "toolCall": {
        // toolCall + toolResult 合并回 assistant-ui 单一 tool-call part
        const result = toolResultText(entry.parts, part.callId);
        let args: unknown;
        try {
          args = JSON.parse(part.argsJson);
        } catch {
          args = {};
        }
        rawParts.push({
          type: "tool-call",
          toolCallId: part.callId,
          toolName: part.name,
          args,
          argsText: part.argsJson,
          ...(result !== undefined ? { result: result.summary, isError: !result.ok } : {}),
        });
        break;
      }
      case "toolResult":
        // 已在上面与 toolCall 合并，跳过
        break;
    }
  }

  const createdAt = new Date(entry.createdAt);
  const metadata =
    entry.role === "assistant" && entry.runId !== undefined ? { runId: entry.runId } : undefined;

  return {
    id: entry.id,
    role: entry.role,
    content: rawParts as unknown as MessageLikeContent,
    createdAt,
    ...(entry.role === "assistant" && entry.status !== undefined ? { status: entry.status } : {}),
    ...(metadata !== undefined ? { metadata } : {}),
  };
}

export function entriesToExported(entries: StoredEntry[]): ExportedMessageRepository {
  const meta = entries.find((e): e is StoredMetaEntry => e.kind === "meta");
  const messages = entries.filter((e): e is StoredMessageEntry => e.kind === "message");
  return ExportedMessageRepository.fromBranchableArray(
    messages.map((entry) => ({
      message: entryToMessageLike(entry),
      parentId: entry.parentId ?? null,
    })),
    { headId: meta?.activeLeafId ?? null },
  );
}

/** load_session_tree 返回 → 导入结构（15 号票历史收养入口）。 */
export function loadedTreeToExported(tree: {
  messages: MessageEntryLike[];
  activeLeafId?: string;
  revision: number;
}): ExportedMessageRepository {
  const entries: StoredEntry[] = tree.messages.map((m) => ({ ...m, kind: "message" }));
  const meta: StoredMetaEntry = { kind: "meta", revision: tree.revision };
  if (tree.activeLeafId != null) meta.activeLeafId = tree.activeLeafId;
  entries.push(meta);
  return entriesToExported(entries);
}

/** load_session_tree 的 messages 元素形态（StoredMessageEntry 去掉 kind 判别键）。 */
type MessageEntryLike = Omit<StoredMessageEntry, "kind">;

// ---- 落盘（spike：单 JSON 文件；生产：逐条 JSONL）----

export function serializeSession(entries: StoredEntry[]): string {
  return `${JSON.stringify({ v: 1, entries }, null, 2)}\n`;
}

export function serializeSessionJsonl(entries: StoredEntry[]): string {
  // 生产形态预演：每行一个条目（append-only 分段的行即条目）
  return `${entries.map((entry) => JSON.stringify(entry)).join("\n")}\n`;
}

export function parseSession(text: string): StoredEntry[] {
  const parsed: unknown = JSON.parse(text);
  if (typeof parsed !== "object" || parsed === null || !("entries" in parsed)) {
    throw new Error("parseSession：文件缺少 entries 字段");
  }
  return (parsed as { entries: StoredEntry[] }).entries;
}

export function parseSessionJsonl(text: string): StoredEntry[] {
  return text
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line) as StoredEntry);
}
