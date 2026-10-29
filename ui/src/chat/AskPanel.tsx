/**
 * AskPanel（§3.2 AskPayload 两形态 / §10.1 ask store，14 号票）：
 * - question：逐题选项点选 + 自由文本输入 → resolveAsk(kind=question)；
 * - approval：diff 预览 / 命令行 + 围栏原因 → 允许/拒绝 → resolveAsk(kind=approval)。
 *
 * 事件驱动：ask:opened 置 store.current、ask:closed 清除（App.tsx 事件桥）。
 * 后端默认无限等待，autoConfirm 300s 由 Rust 侧兜底（前端无需计时）。
 */

import { useState } from "react";
import { useSessionScope } from "@/chat/session-scope";
import { resolveAsk } from "@/ipc/client";
import type { AskPayload, DiffLine } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";

/** 询问面板：本会话有未决 ask 时浮层渲染对应表单，否则不渲染任何内容。 */
export function AskPanel() {
  const sessionId = useSessionScope();
  const current = useAskStore((s) => s.bySession[sessionId]);
  if (!current) return null;
  return (
    <div
      data-testid="ask-panel"
      data-kind={current.payload.kind}
      className="fixed inset-x-4 bottom-4 z-50 mx-auto max-w-xl rounded-2xl border border-popover-border bg-popover p-4 shadow-xl"
    >
      {current.payload.kind === "question" ? (
        <QuestionForm askId={current.askId} payload={current.payload} />
      ) : (
        <ApprovalForm askId={current.askId} payload={current.payload} />
      )}
    </div>
  );
}

/** 应答提交的共享错误面：resolveAsk 失败（槽失效/IPC 异常）时在卡片内
 *  原位显示错误——点击无反馈会让审批永远卡死，静默失败不可接受。 */
function useSubmitError(): [string | null, (err: unknown) => void, () => void] {
  const [error, setError] = useState<string | null>(null);
  const report = (err: unknown) => {
    const message = err instanceof Error ? err.message : String(err);
    console.error("[ask] 应答提交失败", err);
    setError(message || "应答提交失败");
  };
  return [error, report, () => setError(null)];
}

/** 问卷表单：逐题点选选项 / 填自由文本，全部作答后可提交（resolveAsk）。 */
function QuestionForm({
  askId,
  payload,
}: {
  askId: string;
  payload: Extract<AskPayload, { kind: "question" }>;
}) {
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [error, reportError, clearError] = useSubmitError();
  const complete = payload.questions.every(
    (q) => answers[q.id] !== undefined && answers[q.id] !== "",
  );

  const submit = () => {
    clearError();
    resolveAsk(askId, { kind: "question", answers }).catch(reportError);
  };

  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium text-foreground">{payload.title}</h3>
      {payload.questions.map((q) => (
        <div key={q.id} data-testid={`ask-question-${q.id}`} className="space-y-1">
          <p className="text-xs text-foreground">{q.text}</p>
          <div className="flex flex-wrap gap-1.5">
            {q.options?.map((opt) => (
              <button
                key={opt.id}
                type="button"
                data-selected={answers[q.id] === opt.id}
                onClick={() => setAnswers((prev) => ({ ...prev, [q.id]: opt.id }))}
                className={`rounded-full border px-2.5 py-1 text-xs ${
                  answers[q.id] === opt.id
                    ? "border-[var(--ring)] bg-[var(--accent)] text-[var(--foreground)]"
                    : "border-[var(--border)] text-[var(--foreground)] hover:border-[var(--border)]"
                }`}
              >
                {opt.label}
              </button>
            ))}
            {q.allowFreeText && (
              <input
                type="text"
                placeholder="自由输入…"
                value={answers[q.id] ?? ""}
                onChange={(e) => setAnswers((prev) => ({ ...prev, [q.id]: e.target.value }))}
                className="min-w-32 flex-1 rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-xs text-[var(--foreground)]"
              />
            )}
          </div>
        </div>
      ))}
      <button
        type="button"
        disabled={!complete}
        onClick={submit}
        className="rounded-full bg-primary px-4 py-1.5 text-xs text-primary-foreground disabled:opacity-40"
      >
        提交回答
      </button>
      {error !== null && (
        <p
          data-testid="ask-error"
          className="rounded bg-destructive-surface px-2 py-1 text-xs text-destructive-surface-foreground"
        >
          应答提交失败：{error}（可重试）
        </p>
      )}
    </div>
  );
}

/** 审批表单：命令行与 diff 预览 + 允许/拒绝按钮（点击即提交所选选项）。 */
function ApprovalForm({
  askId,
  payload,
}: {
  askId: string;
  payload: Extract<AskPayload, { kind: "approval" }>;
}) {
  const [error, reportError, clearError] = useSubmitError();
  const submit = (optionId: string) => {
    clearError();
    resolveAsk(askId, { kind: "approval", optionId }).catch(reportError);
  };
  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium text-foreground">{payload.title}</h3>
      {error !== null && (
        <p
          data-testid="ask-error"
          className="rounded bg-destructive-surface px-2 py-1 text-xs text-destructive-surface-foreground"
        >
          应答提交失败：{error}（可重试；若反复失败请停止该任务）
        </p>
      )}
      {payload.command && (
        <pre className="overflow-x-auto whitespace-pre-wrap break-all rounded bg-surface px-2 py-1 font-mono text-xs text-foreground">
          {payload.command}
        </pre>
      )}
      {payload.fenceReason && <p className="text-xs text-amber-400">⚠ {payload.fenceReason}</p>}
      {payload.diff && <DiffView diff={payload.diff} />}
      <div className="flex justify-end gap-2">
        {payload.options.map((opt) => (
          <button
            key={opt.id}
            type="button"
            data-testid={`ask-option-${opt.id}`}
            onClick={() => submit(opt.id)}
            className={`rounded-full px-4 py-1.5 text-xs ${
              opt.mode === "deny"
                ? "border border-[var(--border)] text-[var(--foreground)] hover:border-red-500"
                : "bg-[var(--primary)] text-[var(--primary-foreground)]"
            }`}
          >
            {opt.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/** diff 行配色：新增绿底、删除红底、上下文行弱化。 */
const diffLineClass: Record<DiffLine["kind"], string> = {
  add: "bg-emerald-950/60 text-emerald-300",
  del: "bg-red-950/60 text-red-300",
  ctx: "text-[var(--muted-foreground)]",
};

/** diff 预览：文件名标题 + 按 hunk（差异块）分组的增删行列表。 */
function DiffView({
  diff,
}: {
  diff: { path: string; hunks: { header: string; lines: DiffLine[] }[] };
}) {
  return (
    <div data-testid="ask-diff" className="rounded border border-[var(--border)]">
      <p className="border-b border-[var(--border)] px-2 py-1 font-mono text-xs text-[var(--foreground)]">
        {diff.path}
      </p>
      <div className="max-h-64 overflow-auto">
        {diff.hunks.map((hunk) => (
          <div key={hunk.header}>
            <p className="px-2 py-0.5 font-mono text-[10px] text-[var(--muted-foreground)]">
              {hunk.header}
            </p>
            {hunk.lines.map((line) => (
              <pre
                key={`${line.kind}:${line.text}`}
                className={`px-2 font-mono text-xs ${diffLineClass[line.kind]}`}
              >
                {line.kind === "add" ? "+" : line.kind === "del" ? "−" : " "} {line.text}
              </pre>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
