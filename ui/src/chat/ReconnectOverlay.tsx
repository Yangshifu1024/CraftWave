/**
 * 重连观察态覆盖层（§5.8/§10.7，05 号票）：WebView 重载后 Rust run 仍存活
 * 时的只读实时视图。挂在会话 Tab 内容之上（absolute inset-0）——天然禁用
 * 输入（防 start_chat 撞 E_SESSION_RUNNING），与正常流式视觉区分（顶部细条
 * 「已重连到运行中的任务」）。
 *
 * 内容渲染：重放帧与续流帧经独立 FrameBuffer 累积（chat/run-reconnect.ts
 * 驱动），此处按快照段列表增量呈现（思考行 / Markdown 正文 / 工具行）——
 * 不注入 assistant-ui LocalRuntime，不触碰运行态节点记账（§6.3 双写旁路）。
 * runEnded 后覆盖层消失，canonical 树重载由收口信号驱动（SessionTab）。
 */

import { LoaderCircle, Square } from "lucide-react";
import type { SnapshotSegment, ToolCallState } from "@/chat/frames";
import { Markdown } from "@/chat/Markdown";
import { cancelObservedRun, useRunReconnectStore } from "@/chat/run-reconnect";

/** 重连观察态覆盖层：本会话处于观察态时渲染，否则不渲染任何内容。 */
export function ReconnectOverlay({ sessionId }: { sessionId: string }) {
  const phase = useRunReconnectStore((s) => s.scopes[sessionId]?.phase ?? "idle");
  const snapshot = useRunReconnectStore((s) => s.scopes[sessionId]?.snapshot ?? null);
  const cancel = () =>
    void cancelObservedRun(sessionId).catch((err) => {
      console.error("[reconnect] 取消运行中任务失败", err);
    });
  if (phase !== "observing" || snapshot === null) return null;
  const toolById = new Map(snapshot.toolCalls.map((tool) => [tool.callId, tool]));
  // 快照段列表的 key：tool 段用 callId（同调用跨帧稳定）；text/reasoning 段
  // 用类别+序号——快照段是纯追加式时间线（FrameBuffer 只追加不重排不删除），
  // index key 无重渲染风险
  const rows = snapshot.segments.map((segment, index) => {
    let key = `tool:${segment.callId}`;
    if (segment.kind !== "tool") {
      key = `${segment.kind}:${index}`;
    }
    return (
      <SnapshotRow
        key={key}
        segment={segment}
        tool={segment.kind === "tool" ? toolById.get(segment.callId) : undefined}
      />
    );
  });
  return (
    <div
      data-testid="reconnect-overlay"
      className="absolute inset-0 z-40 flex min-h-0 flex-col bg-[var(--cw-bg-main)]"
    >
      {/* 顶部细条（§5.8：观察态视觉与正常流式区分）+ 取消按钮 */}
      <div
        data-testid="reconnect-banner"
        className="flex h-9 shrink-0 items-center gap-2 border-b border-[var(--border)] bg-[var(--card)] px-4 text-ui-sm text-[var(--muted-foreground)]"
      >
        <LoaderCircle size={12} aria-hidden className="shrink-0 animate-spin" />
        <span>已重连到运行中的任务（实时只读）</span>
        <button
          type="button"
          data-testid="reconnect-cancel"
          onClick={cancel}
          className="ml-auto flex h-6 shrink-0 items-center gap-1 rounded-lg border border-[var(--border)] px-2 text-ui-sm text-[var(--foreground)] hover:border-[var(--destructive)] hover:text-[var(--destructive)]"
        >
          <Square size={10} aria-hidden fill="currentColor" />
          取消任务
        </button>
      </div>
      {/* 只读实时内容区：文本/工具卡照常增量呈现 */}
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-6">
        <div className="mx-auto w-full max-w-4xl space-y-4 2xl:max-w-6xl">
          {snapshot.run && (
            <p className="text-ui-sm text-[var(--cw-dim)]">运行模型：{snapshot.run.model}</p>
          )}
          {rows}
          {!snapshot.ended && (
            <div className="flex items-center gap-2 px-1 py-1 text-ui-sm text-[var(--muted-foreground)]">
              <LoaderCircle size={12} aria-hidden className="shrink-0 animate-spin" />
              处理中…
            </div>
          )}
          {snapshot.ended && (
            <p className="px-1 py-1 text-ui-sm text-[var(--cw-dim)]">任务已结束，正在恢复会话…</p>
          )}
        </div>
      </div>
    </div>
  );
}

/** 快照段渲染：思考行 / Markdown 正文 / 工具行（观察态简化形态）。 */
function SnapshotRow({ segment, tool }: { segment: SnapshotSegment; tool?: ToolCallState }) {
  if (segment.kind === "reasoning") {
    return (
      <div
        data-testid="reconnect-reasoning"
        className="rounded-lg border border-[var(--border)]/60 bg-[var(--muted)]/40 px-3 py-2 text-ui-sm whitespace-pre-wrap text-[var(--muted-foreground)]"
      >
        {segment.text}
      </div>
    );
  }
  if (segment.kind === "text") {
    return (
      <div data-testid="reconnect-text" className="text-ui-base leading-6 text-[var(--foreground)]">
        <Markdown streaming={true}>{segment.text}</Markdown>
      </div>
    );
  }
  // 工具行（镜像卡简化形态：名称 + 参数准备中/结果摘要）
  return (
    <div
      data-testid="reconnect-tool"
      className="flex items-center gap-2 rounded-lg border border-[var(--border)] px-3 py-2 text-ui-sm text-[var(--foreground)]"
    >
      <span className="shrink-0 rounded border border-[var(--border)] px-1.5 font-mono text-ui-sm leading-4">
        {tool?.name ?? "?"}
      </span>
      <span className="min-w-0 flex-1 truncate text-[var(--muted-foreground)]">
        {tool === undefined ? "准备中…" : tool.result ? tool.result.uiSummary : "运行中…"}
      </span>
      {tool?.result && (
        <span
          className={`shrink-0 ${tool.result.ok ? "text-[var(--cw-dim)]" : "text-[var(--destructive)]"}`}
        >
          {tool.result.ok ? "完成" : "失败"}
        </span>
      )}
    </div>
  );
}
