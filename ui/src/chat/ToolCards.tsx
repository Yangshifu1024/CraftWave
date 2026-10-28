/**
 * 正式工具卡片（§10.3 规格，14 号票；17 号后按参考样式改版）：
 * toolkit 条目的 render 组件，统一为「单行摘要 + 点击展开详情」的工作流
 * 风格——摘要行 = 动作图标 + 主参数（路径/命令/模式）+ diff 统计徽标 +
 * 状态（运行中转圈 / 错误感叹 / 静默完成）；详情区（点击展开）承载参数
 * 全文、结果摘要与 toolres 边车（§7.5）的完整数据。错误态默认展开。
 *
 * 数据源：part props（args/result/isError）+ useToolArgsStatus（S1 钉死）。
 * 状态机：参数准备中（半截 JSON）→ 运行中 → 完成/错误（result 直写驱动）。
 * 15 号票：终态卡片按 callId 拉 toolres 边车，折叠展示 detail/data 全量。
 */

import { type ToolCallMessagePartProps, useToolArgsStatus } from "@assistant-ui/react";
import { useEffect, useState } from "react";
import { useSessionScope } from "@/chat/session-scope";
import * as client from "@/ipc/client";
import type { ToolOutcomeJson } from "@/ipc/types";

/** 卡片通用状态判定（与 mirror 原型同口径，供测试直接断言）。 */
export function cardPhase(
  statusType: "running" | "complete" | "incomplete" | "requires-action",
  hasResult: boolean,
  argsComplete: boolean,
): "preparing" | "running" | "done" | "error" {
  if (hasResult && statusType === "running") return "done";
  if (hasResult) return "done";
  if (statusType === "running" && !argsComplete) return "preparing";
  return "running";
}

/** 卡片读取工具参数用的宽泛类型（args 是 JSON 解析结果）。 */
type CardArgs = Record<string, unknown>;

/** 旋转指示（lucide loader-circle 造型，运行中状态；与 Thread 处理中同款）。 */
function SpinnerIcon() {
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
      className="block animate-spin"
    >
      <path d="M21 12a9 9 0 1 1-6.219-8.56" />
    </svg>
  );
}

/** 告警图标（lucide circle-alert 造型，错误态前缀；与 Thread 错误块同款）。 */
function AlertIconSmall() {
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

/** 卡片通用 props：assistant-ui 传入的工具调用 part 数据 + 摘要行内容。 */
interface CardProps extends ToolCallMessagePartProps<CardArgs, unknown> {
  /** 摘要行的动作图标（emoji，与参考界面口径一致）。 */
  icon: string;
  /** 摘要行主文本（路径 / 命令 / 模式等，等宽字体截断）。 */
  summary: string;
  /** 摘要行尾部徽标（如 +N/−N 统计，可选）。 */
  badges?: React.ReactNode;
}

/** 终态卡片的边车 detail（§7.5）：按 callId 拉一次，缺文件静默（summary 降级）。 */
function useToolOutcomeDetail(toolCallId: string, settled: boolean): ToolOutcomeJson | null {
  const sessionId = useSessionScope();
  const [outcome, setOutcome] = useState<ToolOutcomeJson | null>(null);
  useEffect(() => {
    if (!settled) return;
    let cancelled = false;
    void client
      .loadToolOutcomes(sessionId, [toolCallId])
      .then((map) => {
        if (!cancelled) setOutcome(map[toolCallId] ?? null);
      })
      .catch(() => {
        // IPC 失败（如纯浏览器预览）：summary 降级，不打扰
      });
    return () => {
      cancelled = true;
    };
  }, [toolCallId, settled, sessionId]);
  return outcome;
}

/** 单行摘要壳：整行可点击展开/收起；详情区含参数全文、结果与边车数据。 */
function ToolCardShell({
  icon,
  summary,
  badges,
  children,
  ...props
}: CardProps & { children?: React.ReactNode }) {
  const { toolCallId, result, isError, argsText } = props;
  const { status, argsStatus } = useToolArgsStatus();
  const argsDone = argsStatus === "complete";
  const hasResult = result !== undefined;
  const phase = isError ? "error" : cardPhase(status, hasResult, argsDone);
  const settled = phase === "done" || phase === "error";
  // 错误态默认展开（失败原因一眼可见）；其余默认收起为单行摘要
  const [open, setOpen] = useState(phase === "error");
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const detailJson = outcome
    ? JSON.stringify(outcome.detail ?? outcome.data ?? null, null, 2)
    : null;

  const resultSummary =
    outcome?.uiSummary ||
    (result !== undefined && typeof result === "object" && result !== null
      ? String((result as { summary?: string }).summary ?? "")
      : "") ||
    "";

  return (
    <div data-testid={`tool-card-${toolCallId}`} data-phase={phase} className="my-0.5 text-[13px]">
      {/* 摘要行：图标 + 主参数 + 徽标 + 状态（点击展开详情） */}
      <button
        type="button"
        data-phase-label={phase}
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 rounded px-1 py-0.5 text-left hover:bg-[var(--accent)]/60"
        aria-expanded={open}
      >
        <span aria-hidden className="shrink-0">
          {icon}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[var(--foreground)]">
          {summary || "（等待参数…）"}
        </span>
        {badges}
        {phase === "running" && (
          <span data-testid="tool-spinner" className="shrink-0 text-[var(--muted-foreground)]">
            <SpinnerIcon />
          </span>
        )}
        {phase === "preparing" && <span className="shrink-0 text-[var(--cw-dim)]">…</span>}
        {phase === "error" && (
          <span className="flex shrink-0 items-center gap-1 font-medium text-[var(--destructive)]">
            <AlertIconSmall />
            失败
          </span>
        )}
        <span aria-hidden className="shrink-0 text-[var(--cw-dim)]">
          {open ? "▾" : "▸"}
        </span>
      </button>
      {/* 详情区：参数原文 + 卡片专属排版 + 结果摘要 + 边车全量数据 */}
      {open && (
        <div className="ml-6 mt-1 space-y-1 rounded-lg border border-[var(--border)] bg-[var(--card)] px-3 py-2">
          {children}
          {phase === "preparing" && (
            <pre className="max-h-24 overflow-hidden whitespace-pre-wrap break-all text-[var(--muted-foreground)]">
              {argsText || "（等待参数…）"}
            </pre>
          )}
          {hasResult && (
            <p className={isError ? "text-[var(--destructive)]" : "text-[var(--muted-foreground)]"}>
              {resultSummary}
            </p>
          )}
          {detailJson !== null && detailJson !== "null" && (
            <details className="rounded border border-[var(--border)] bg-[var(--background)]/60 px-2 py-1">
              <summary className="cursor-default select-none text-[var(--muted-foreground)]">
                详细数据
              </summary>
              <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-all text-[var(--muted-foreground)]">
                {outcome?.truncated ? "（结果超过 5MB，已截断——全量见 spill 文件）" : detailJson}
              </pre>
            </details>
          )}
        </div>
      )}
    </div>
  );
}

/** 从 args 取字符串字段的辅助（类型不符返回空串）。 */
const str = (args: CardArgs, key: string): string =>
  typeof args[key] === "string" ? (args[key] as string) : "";
/** 从 args 取数字字段的辅助（类型不符返回 undefined）。 */
const num = (args: CardArgs, key: string): number | undefined =>
  typeof args[key] === "number" ? (args[key] as number) : undefined;
/** 文本行数计数（空文本算 0 行），用于 +N/−N 徽标。 */
const lineCount = (text: string): number => (text.length > 0 ? text.split("\n").length : 0);

/** 增加行数徽标（绿底 +N）。 */
const PlusBadge = ({ n }: { n: number }) => (
  <span className="shrink-0 rounded bg-emerald-500/20 px-1 font-mono text-emerald-600 dark:text-emerald-300">
    +{n}
  </span>
);
/** 删除行数徽标（红底 −N）。 */
const MinusBadge = ({ n }: { n: number }) => (
  <span className="shrink-0 rounded bg-red-500/20 px-1 font-mono text-red-600 dark:text-red-300">
    −{n}
  </span>
);

/** read：路径 + 行区间（详情含区间说明）。 */
export function ReadCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const offset = num(args, "offset");
  const limit = num(args, "limit");
  const range =
    offset !== undefined || limit !== undefined
      ? `（${offset ?? 1} 起${limit !== undefined ? ` · ${limit} 行` : ""}）`
      : "";
  return (
    <ToolCardShell {...props} icon="🔍" summary={str(args, "path")}>
      <p className="text-[var(--muted-foreground)]">{range}</p>
    </ToolCardShell>
  );
}

/** write：新文件 + +N 徽标（content 行数）。 */
export function WriteCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "content"));
  return (
    <ToolCardShell
      {...props}
      icon="✏️"
      summary={str(args, "path")}
      badges={<PlusBadge n={added} />}
    />
  );
}

/** edit：路径 + +N/−N 徽标（newText/oldText 行数）。 */
export function EditCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "newText"));
  const removed = lineCount(str(args, "oldText"));
  return (
    <ToolCardShell
      {...props}
      icon="✏️"
      summary={str(args, "path")}
      badges={
        <>
          <PlusBadge n={added} />
          <MinusBadge n={removed} />
        </>
      }
    />
  );
}

/** list_files：目录树（摘要 = 目标目录）。 */
export function ListFilesCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const target = str(args, "path") || "（项目根）";
  return <ToolCardShell {...props} icon="📂" summary={target} />;
}

/** grep：正则模式 + glob 过滤 + 搜索起点。 */
export function GrepCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const pattern = str(args, "pattern");
  const suffix = [str(args, "glob"), str(args, "path") ? `@ ${str(args, "path")}` : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <ToolCardShell {...props} icon="🔍" summary={`/${pattern}/ ${suffix}`.trim()}>
      {suffix === "" && <p className="text-[var(--muted-foreground)]">（全项目搜索）</p>}
    </ToolCardShell>
  );
}

/** command：命令行 monospace（摘要即命令文本）。 */
export function CommandCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return <ToolCardShell {...props} icon="▣" summary={str(args, "command")} />;
}

/** plan 工具 items 参数里单个条目的形状。 */
interface PlanItemArg {
  id: string;
  text: string;
  status: string;
}

/** plan：计划卡（FR-06-01 消息内呈现；摘要 = 完成数统计）。 */
export function PlanCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const items = Array.isArray(args.items) ? (args.items as PlanItemArg[]) : [];
  const done = items.filter((i) => i.status === "completed").length;
  const statusLabel: Record<string, string> = {
    pending: "待办",
    in_progress: "进行中",
    completed: "已完成",
  };
  return (
    <ToolCardShell {...props} icon="📋" summary={`任务计划（${done}/${items.length}）`}>
      <ul className="space-y-0.5">
        {items.map((item) => (
          <li key={item.id} className="flex items-start gap-1.5">
            <span
              className={
                item.status === "completed"
                  ? "text-emerald-600 dark:text-emerald-400"
                  : "text-[var(--muted-foreground)]"
              }
            >
              {item.status === "completed" ? "●" : item.status === "in_progress" ? "◐" : "○"}
            </span>
            <span className="text-[var(--foreground)]">
              {item.text}
              <span className="ml-1 text-[var(--muted-foreground)]">
                {statusLabel[item.status] ?? item.status}
              </span>
            </span>
          </li>
        ))}
      </ul>
    </ToolCardShell>
  );
}

/** ask：极简状态卡（问题本体在 AskPanel 呈现，§10.3 的 M1 落地形态）。 */
export function AskCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} icon="❓" summary={str(args, "title")}>
      <p className="text-[var(--muted-foreground)]">回答在下方询问面板中进行</p>
    </ToolCardShell>
  );
}
