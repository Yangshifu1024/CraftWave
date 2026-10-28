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

/** 问卷表单：逐题点选选项 / 填自由文本，全部作答后可提交（resolveAsk）。 */
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
      <h3 className="text-ui-base font-medium text-[var(--foreground)]">{payload.title}</h3>
      {payload.questions.map((q) => (
        <div key={q.id} data-testid={`ask-question-${q.id}`} className="space-y-1">
          <p className="text-ui-sm text-[var(--foreground)]">{q.text}</p>
          <div className="flex flex-wrap gap-1.5">
            {q.options?.map((opt) => (
              <button
                key={opt.id}
                type="button"
                data-selected={answers[q.id] === opt.id}
                onClick={() => setAnswers((prev) => ({ ...prev, [q.id]: opt.id }))}
                className={`rounded-full border px-2.5 py-1 text-ui-sm ${
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
                className="min-w-32 flex-1 rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-ui-sm text-[var(--foreground)]"
              />
            )}
          </div>
        </div>
      ))}
      <button
        type="button"
        disabled={!complete}
        onClick={submit}
        className="rounded-full bg-[var(--primary)] px-4 py-1.5 text-ui-sm text-[var(--primary-foreground)] disabled:opacity-40"
      >
        提交回答
      </button>
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
  const submit = (optionId: string) => {
    void resolveAsk(askId, { kind: "approval", optionId });
  };
  return (
    <div className="space-y-3">
      <h3 className="text-ui-base font-medium text-[var(--foreground)]">{payload.title}</h3>
      {payload.command && (
        <pre className="overflow-x-auto whitespace-pre-wrap break-all rounded bg-[var(--background)] px-2 py-1 font-mono text-ui-sm text-[var(--foreground)]">
          {payload.command}
        </pre>
      )}
      {payload.fenceReason && (
        <p className="flex items-center gap-1.5 text-ui-sm text-[var(--warning)]">
          <AlertIcon />
          {payload.fenceReason}
        </p>
      )}
      {payload.diff && <DiffView diff={payload.diff} />}
      <div className="flex justify-end gap-2">
        {payload.options.map((opt) => (
          <button
            key={opt.id}
            type="button"
            data-testid={`ask-option-${opt.id}`}
            onClick={() => submit(opt.id)}
            className={`rounded-full px-4 py-1.5 text-ui-sm ${
              opt.mode === "deny"
                ? "border border-[var(--border)] text-[var(--foreground)] hover:border-[var(--destructive)]"
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
      <p className="border-b border-[var(--border)] px-2 py-1 font-mono text-ui-sm text-[var(--foreground)]">
        {diff.path}
      </p>
      <div className="max-h-64 overflow-auto">
        {diff.hunks.map((hunk) => (
          <div key={hunk.header}>
            <p className="px-2 py-0.5 font-mono text-ui-sm text-[var(--muted-foreground)]">
              {hunk.header}
            </p>
            {hunk.lines.map((line) => (
              <pre
                key={`${line.kind}:${line.text}`}
                className={`px-2 font-mono text-ui-sm ${diffLineClass[line.kind]}`}
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

/** 告警图标（lucide circle-alert 造型，围栏原因提示前缀；与 Thread/ToolCards 同款）。 */
function AlertIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="shrink-0"
    >
      <circle cx="12" cy="12" r="10" />
      <path d="M12 8v4" />
      <path d="M12 16h.01" />
    </svg>
  );
}
