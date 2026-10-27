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

export function AskPanel() {
  const sessionId = useSessionScope();
  const current = useAskStore((s) => s.bySession[sessionId]);
  if (!current) return null;
  return (
    <div
      data-testid="ask-panel"
      data-kind={current.payload.kind}
      className="fixed inset-x-4 bottom-4 z-50 mx-auto max-w-xl rounded-xl border border-[var(--border)] bg-[var(--card)] p-4 shadow-2xl"
    >
      {current.payload.kind === "question" ? (
        <QuestionForm askId={current.askId} payload={current.payload} />
      ) : (
        <ApprovalForm askId={current.askId} payload={current.payload} />
      )}
    </div>
  );
}

function QuestionForm({
  askId,
  payload,
}: {
  askId: string;
  payload: Extract<AskPayload, { kind: "question" }>;
}) {
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const complete = payload.questions.every(
    (q) => answers[q.id] !== undefined && answers[q.id] !== "",
  );

  const submit = () => {
    void resolveAsk(askId, { kind: "question", answers });
  };

  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium text-[var(--foreground)]">{payload.title}</h3>
      {payload.questions.map((q) => (
        <div key={q.id} data-testid={`ask-question-${q.id}`} className="space-y-1">
          <p className="text-xs text-[var(--foreground)]">{q.text}</p>
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
        className="rounded-full bg-[var(--primary)] px-4 py-1.5 text-xs text-[var(--primary-foreground)] disabled:opacity-40"
      >
        提交回答
      </button>
    </div>
  );
}

function ApprovalForm({
  askId,
  payload,
}: {
  askId: string;
  payload: Extract<AskPayload, { kind: "approval" }>;
}) {
  const submit = (optionId: string) => {
    void resolveAsk(askId, { kind: "approval", optionId });
  };
  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium text-[var(--foreground)]">{payload.title}</h3>
      {payload.command && (
        <pre className="overflow-x-auto whitespace-pre-wrap break-all rounded bg-[var(--background)] px-2 py-1 font-mono text-xs text-[var(--foreground)]">
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

const diffLineClass: Record<DiffLine["kind"], string> = {
  add: "bg-emerald-950/60 text-emerald-300",
  del: "bg-red-950/60 text-red-300",
  ctx: "text-[var(--muted-foreground)]",
};

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
