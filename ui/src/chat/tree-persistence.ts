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
  // runMarker 条目（system + custom 载荷）：parts 还原为 RunMarker part
  //（链上节点，wire 重建与渲染均跳过其内容，§6.2）
  const runMarker = readRunMarker(message);
  if (runMarker !== null) {
    const entry: StoredMessageEntry = {
      kind: "message",
      id: message.id,
      role: "system",
      parts: [{ type: "runMarker", ...runMarker }],
      createdAt: message.createdAt.getTime(),
    };
    if (parentId !== null) entry.parentId = parentId;
    return entry;
  }
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

/** 树消息的 runMarker 载荷（entryToMessageLike 写入的 custom metadata；无则 null）。
 *  toWire（runtime.ts）共用——三点转换（import/export/wire）单一判定口径。 */
export function readRunMarker(
  message: Pick<ThreadMessage, "metadata">,
): Omit<Extract<WirePart, { type: "runMarker" }>, "type"> | null {
  const raw = (message.metadata as { custom?: Record<string, unknown> } | undefined)?.custom
    ?.runMarker;
  if (raw === null || typeof raw !== "object") return null;
  const m = raw as Record<string, unknown>;
  if (typeof m.runId !== "string" || typeof m.startedAt !== "number") return null;
  const out: {
    runId: string;
    startedAt: number;
    endedAt?: number;
    durationMs?: number;
    toolCount?: number;
  } = {
    runId: m.runId,
    startedAt: m.startedAt,
  };
  if (typeof m.endedAt === "number") out.endedAt = m.endedAt;
  if (typeof m.durationMs === "number") out.durationMs = m.durationMs;
  if (typeof m.toolCount === "number") out.toolCount = m.toolCount;
  return out;
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
  // runMarker 条目：assistant-ui 无对应 part 类型——空 system 消息 +
  // metadata.custom.runMarker 载荷保真（messageToEntry 侧对称还原）；
  // 不保真会在 save 全树同步时同 id 内容漂移 → E_SESSION_REV
  const marker = entry.parts.find(
    (p): p is Extract<WirePart, { type: "runMarker" }> => p.type === "runMarker",
  );
  if (marker !== undefined) {
    const { type: _type, ...payload } = marker;
    return {
      id: entry.id,
      role: "system",
      // assistant-ui 约束 system 消息必须恰好一个 text part；占位符不进
      // 导出/wire 面（marker 分支优先），Thread 渲染层过滤 system 消息
      content: [{ type: "text", text: "·" }],
      createdAt: new Date(entry.createdAt),
      metadata: { custom: { runMarker: payload } },
    };
  }
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

/** 单个 run 的收口摘要（折叠分组摘要行与崩溃中断态的数据面，17 号票）。 */
export interface RunSummary {
  runId: string;
  /** 正常收口的结束时刻；undefined = 该 run 无 marker 收口（崩溃恢复补的
   *  interrupted 修复行 endedAt 为空）——渲染层据此标「已中断」。 */
  endedAt?: number;
  durationMs?: number;
  toolCount?: number;
}

/** 从会话树条目推导 run 收口摘要表（runId → 摘要）。
 *  依据 system 条目里的 runMarker part（§6.1）；崩溃修复行 endedAt 为空，
 *  恰好承载「未收口」语义（15 号票 shortcut 的渲染层落地，17 号票）。 */
export function deriveRunSummaries(messages: MessageEntryLike[]): Map<string, RunSummary> {
  const summaries = new Map<string, RunSummary>();
  for (const entry of messages) {
    if (entry.role !== "system") continue;
    const marker = entry.parts.find(
      (p): p is Extract<WirePart, { type: "runMarker" }> => p.type === "runMarker",
    );
    if (!marker) continue;
    summaries.set(marker.runId, {
      runId: marker.runId,
      endedAt: marker.endedAt,
      durationMs: marker.durationMs,
      toolCount: marker.toolCount,
    });
  }
  return summaries;
}

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
