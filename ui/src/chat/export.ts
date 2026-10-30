/**
 * 会话导出（27 号票，FR-01-12；§10.7 形态定稿）：纯前端序列化，无新命令。
 *
 * 数据源（与历史恢复同源）：
 * - `loadSessionTree`（活动路径 messages，§6.3——只导活动路径/当前分支，
 *   全树导出不做，PRD「分支当前路径」口径）；
 * - `loadToolOutcomes`（工具 uiSummary，历史工具卡恢复同源，§7.5）；
 *   工具行明细缺文件时按帧面 summary 降级（#29 语义）。
 *
 * 两种形态（§10.7 27 号票定稿）：
 * - Markdown（人读归档）：`# <标题>` + 导出时间行 → 逐条 `## user /
 *   assistant`（createdAt + 文本）→ 工具调用以列表行
 *   `- ⚙ name — uiSummary（ok/error）` 附在产生它的 assistant 段内；
 *   reasoning 不导出（归档/分享面，票面边界）。
 * - JSONL（机器可读）：活动路径消息逐行导出（含 toolCall/toolResult
 *   parts），仅剔除 reasoning part；图片 part 只含 blobHash 引用不含本体
 *   （§6.1 image part 本就不携带字节，不宣称跨机器完整回放）。
 *
 * 下载：Blob + `URL.createObjectURL` + `a[download]`（Tauri WebView 同
 * 浏览器下载语义），文件名 `<标题>-<YYYYMMDD>.md|jsonl`（非法文件名字符
 * 替换为 `-`）。
 */

import { loadSessionTree, loadToolOutcomes } from "@/ipc/client";
import type { MessageEntry, SessionMeta, ToolOutcome, WirePart } from "@/ipc/types";

/** 导出格式。 */
export type ExportFormat = "markdown" | "jsonl";

/** 工具记录行（Markdown 工具调用列表行的数据面）。 */
export interface ToolLine {
  name: string;
  uiSummary: string;
  ok: boolean;
}

/** 提取消息正文（Text parts 顺序拼接）。 */
export function textOf(parts: WirePart[]): string {
  return parts
    .filter((p): p is Extract<WirePart, { type: "text" }> => p.type === "text")
    .map((p) => p.text)
    .join("");
}

/** 是否对话正文条目（runMarker 等 system 内部条目不进 Markdown 归档）。 */
export function isConversationEntry(entry: MessageEntry): boolean {
  return entry.role === "user" || entry.role === "assistant";
}

/** 单条消息内的工具调用列表行（callId 配对同条目内的 toolResult）。 */
export function toolLinesOf(
  entry: MessageEntry,
  outcomes: Record<string, ToolOutcome>,
): ToolLine[] {
  const resultByCall = new Map<string, Extract<WirePart, { type: "toolResult" }>>();
  for (const part of entry.parts) {
    if (part.type === "toolResult") resultByCall.set(part.callId, part);
  }
  const lines: ToolLine[] = [];
  for (const part of entry.parts) {
    if (part.type !== "toolCall") continue;
    // 边车优先（全量结果面）；缺文件按帧面 summary/ok 降级（#29 语义）
    const sidecar = outcomes[part.callId];
    const result = resultByCall.get(part.callId);
    const uiSummary = sidecar?.uiSummary || result?.summary || part.name;
    const ok = sidecar?.ok ?? result?.ok ?? true;
    lines.push({ name: part.name, uiSummary, ok });
  }
  return lines;
}

/** Unix 毫秒 → 本地 `YYYY-MM-DD HH:mm`（导出归档的可读时间面）。 */
export function formatTimestamp(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Unix 毫秒 → `YYYYMMDD`（导出文件名的日期段）。 */
export function formatFileDate(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}`;
}

/** Markdown 单条消息段（`## user / ## assistant` + 时间行 + 正文 + 工具行）。 */
export function messageToMarkdown(
  entry: MessageEntry,
  outcomes: Record<string, ToolOutcome>,
): string {
  const role = entry.role === "user" ? "user" : "assistant";
  const lines: string[] = [`## ${role}`, "", `- 时间：${formatTimestamp(entry.createdAt)}`, ""];
  const text = textOf(entry.parts).trim();
  if (text) {
    lines.push(text, "");
  }
  const tools = toolLinesOf(entry, outcomes);
  for (const tool of tools) {
    lines.push(`- ⚙ ${tool.name} — ${tool.uiSummary}（${tool.ok ? "ok" : "error"}）`);
  }
  if (tools.length > 0) {
    lines.push("");
  }
  return lines.join("\n");
}

/** 序列化为 Markdown（人读归档，§10.7 形态）。reasoning/planSnapshot/
 * runMarker 等内部条目不导出（票面边界）。 */
export function toMarkdown(
  meta: Pick<SessionMeta, "title">,
  messages: MessageEntry[],
  outcomes: Record<string, ToolOutcome>,
  exportedAt = Date.now(),
): string {
  const header = [`# ${meta.title}`, "", `- 导出时间：${formatTimestamp(exportedAt)}`, ""].join(
    "\n",
  );
  const body = messages
    .filter(isConversationEntry)
    .map((entry) => messageToMarkdown(entry, outcomes));
  return `${[header, ...body].join("\n").trimEnd()}\n`;
}

/** 序列化为 JSONL（机器可读）：活动路径逐行 `JSON.stringify(MessageEntry)`，
 *  仅剔除 reasoning part（含 toolCall/toolResult parts；图片 part 只含
 *  blobHash 引用不含本体）。 */
export function toJsonl(messages: MessageEntry[]): string {
  const lines = messages
    .map((entry) => ({
      ...entry,
      parts: entry.parts.filter((p) => p.type !== "reasoning"),
    }))
    .map((entry) => JSON.stringify(entry));
  return `${lines.join("\n")}\n`;
}

/** 导出文件名：`<标题>-<YYYYMMDD>.<ext>`；标题清洗三步——① 跨平台非法
 *  文件名字符 `/ \ : * ? " < > |` 与控制字符替换为 `-`；② 连续 `-` 折叠为
 *  单个（避免 `///` 之类标题产生一串减号）；③ 去首尾空白与 `-.`（Windows
 *  结尾点非法、首尾分隔符无意义），清空后兜底「会话」。 */
export function exportFileName(title: string, ext: "md" | "jsonl", now = Date.now()): string {
  const cleaned =
    title
      // \p{Cc} = Unicode 全部控制字符（C0/C1；字面区间写法触发 lint 禁令）
      .replace(/\p{Cc}/gu, "-")
      .replace(/[/\\:*?"<>|]/g, "-")
      .replace(/-+/g, "-")
      .trim()
      .replace(/^[-.]+|[-.]+$/g, "") || "会话";
  return `${cleaned}-${formatFileDate(now)}.${ext}`;
}

/** 触发浏览器下载（Tauri WebView 同浏览器语义）：Blob URL + 隐形链接点击，
 *  用后 revoke 防泄漏。 */
export function downloadText(filename: string, content: string, mime: string): void {
  const blob = new Blob([content], { type: mime });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

/** 导出编排（SessionWorkspace context-menu 入口调用）：加载活动路径树 →
 *  批量拉工具边车（失败降级空表——工具行按帧面 summary 呈现）→ 序列化 →
 *  下载。返回实际下载的文件名（测试/调用方可提示）。 */
export async function exportSession(
  meta: Pick<SessionMeta, "id" | "title">,
  format: ExportFormat,
  now = Date.now(),
): Promise<string> {
  const tree = await loadSessionTree(meta.id);
  const callIds = tree.messages.flatMap((entry) =>
    entry.parts
      .filter((p): p is Extract<WirePart, { type: "toolCall" }> => p.type === "toolCall")
      .map((p) => p.callId),
  );
  const outcomes = await loadToolOutcomes(meta.id, callIds).catch(() => ({}));
  if (format === "markdown") {
    const filename = exportFileName(meta.title, "md", now);
    downloadText(
      filename,
      toMarkdown(meta, tree.messages, outcomes, now),
      "text/markdown;charset=utf-8",
    );
    return filename;
  }
  const filename = exportFileName(meta.title, "jsonl", now);
  downloadText(filename, toJsonl(tree.messages), "application/x-ndjson");
  return filename;
}
