/**
 * 正式工具卡片（§10.3 规格，14 号票）：toolkit 条目的 render 组件。
 *
 * 数据源：part props（args/result/isError）+ useToolArgsStatus（S1 钉死）。
 * 状态机：参数准备中（半截 JSON）→ 运行中 → 完成/错误（result 直写驱动）。
 * 15 号票：终态卡片按 callId 拉 toolres 边车（§7.5），折叠展示 detail/data
 * 全量 JSON——历史恢复的数据面；各卡片专属排版（read 预览高亮等）随桌面
 * 体验票打磨。
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

/** 卡片通用 props：assistant-ui 传入的工具调用 part 数据 + 卡片标题。 */
interface CardProps extends ToolCallMessagePartProps<CardArgs, unknown> {
  title: string;
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

/** 通用壳：标题 + 状态徽标 + result 摘要行。 */
function ToolCardShell({ title, children, ...props }: CardProps & { children?: React.ReactNode }) {
  const { toolCallId, result, isError, argsText } = props;
  const { status, argsStatus } = useToolArgsStatus();
  const argsDone = argsStatus === "complete";
  const hasResult = result !== undefined;
  const phase = isError ? "error" : cardPhase(status, hasResult, argsDone);
  const phaseLabel = {
    preparing: "参数准备中",
    running: "运行中",
    error: "错误",
    done: "完成",
  }[phase];
  const settled = phase === "done" || phase === "error";
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const detailJson = outcome
    ? JSON.stringify(outcome.detail ?? outcome.data ?? null, null, 2)
    : null;

  const summary =
    outcome?.uiSummary ||
    (result !== undefined && typeof result === "object" && result !== null
      ? String((result as { summary?: string }).summary ?? "")
      : "") ||
    "";

  return (
    <div
      data-testid={`tool-card-${toolCallId}`}
      data-phase={phase}
      className="my-1 rounded-lg border border-[var(--border)] bg-[var(--card)] px-3 py-2 text-xs"
    >
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium text-[var(--foreground)]">{title}</span>
        <span
          data-phase-label={phaseLabel}
          className={
            phase === "error"
              ? "rounded bg-red-900/60 px-1.5 py-0.5 text-red-200"
              : phase === "done"
                ? "rounded bg-emerald-900/60 px-1.5 py-0.5 text-emerald-200"
                : "rounded bg-[var(--accent)] px-1.5 py-0.5 text-[var(--foreground)]"
          }
        >
          {phaseLabel}
        </span>
      </div>
      <div className="mt-1 space-y-1">{children}</div>
      {phase === "preparing" && (
        <pre className="max-h-24 overflow-hidden whitespace-pre-wrap break-all text-[var(--muted-foreground)]">
          {argsText || "（等待参数…）"}
        </pre>
      )}
      {hasResult && (
        <p className={isError ? "text-red-300" : "text-[var(--muted-foreground)]"}>{summary}</p>
      )}
      {detailJson !== null && detailJson !== "null" && (
        <details className="mt-1 rounded border border-[var(--border)] bg-[var(--background)]/60 px-2 py-1">
          <summary className="cursor-default select-none text-[var(--muted-foreground)]">
            详细数据
          </summary>
          <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-all text-[var(--muted-foreground)]">
            {outcome?.truncated ? "（结果超过 5MB，已截断——全量见 spill 文件）" : detailJson}
          </pre>
        </details>
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

/** read：路径 + 行区间。 */
export function ReadCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const offset = num(args, "offset");
  const limit = num(args, "limit");
  const range =
    offset !== undefined || limit !== undefined
      ? `（${offset ?? 1} 起${limit !== undefined ? ` · ${limit} 行` : ""}）`
      : "";
  return (
    <ToolCardShell {...props} title="读取文件">
      <p className="font-mono text-[var(--foreground)]">
        {str(args, "path")}
        <span className="text-[var(--muted-foreground)]">{range}</span>
      </p>
    </ToolCardShell>
  );
}

/** write：新文件 + +N 徽标（content 行数）。 */
export function WriteCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "content"));
  return (
    <ToolCardShell {...props} title="写入新文件">
      <p className="font-mono text-[var(--foreground)]">
        {str(args, "path")}{" "}
        <span className="rounded bg-emerald-900/60 px-1 text-emerald-200">+{added}</span>
      </p>
    </ToolCardShell>
  );
}

/** edit：路径 + +N/−N 徽标（newText/oldText 行数）。 */
export function EditCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "newText"));
  const removed = lineCount(str(args, "oldText"));
  return (
    <ToolCardShell {...props} title="编辑文件">
      <p className="font-mono text-[var(--foreground)]">
        {str(args, "path")}{" "}
        <span className="rounded bg-emerald-900/60 px-1 text-emerald-200">+{added}</span>{" "}
        <span className="rounded bg-red-900/60 px-1 text-red-200">−{removed}</span>
      </p>
    </ToolCardShell>
  );
}

/** list_files / grep：目标 + 计数摘要（命中数在 result.summary）。 */
export function ListFilesCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} title="列目录">
      <p className="font-mono text-[var(--foreground)]">{str(args, "path") || "（项目根）"}</p>
    </ToolCardShell>
  );
}

/** grep：正则模式 + glob 过滤 + 搜索起点。 */
export function GrepCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} title="搜索">
      <p className="font-mono text-[var(--foreground)]">
        <span className="text-[var(--muted-foreground)]">/{str(args, "pattern")}/</span>{" "}
        {str(args, "glob") || ""}
        {str(args, "path") ? ` @ ${str(args, "path")}` : ""}
      </p>
    </ToolCardShell>
  );
}

/** command：命令行 monospace（输出折叠/退出码徽标随 toolres 边车票）。 */
export function CommandCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} title="执行命令">
      <pre className="overflow-x-auto whitespace-pre-wrap break-all rounded bg-[var(--background)] px-2 py-1 font-mono text-[var(--foreground)]">
        {str(args, "command") || "（等待参数…）"}
      </pre>
    </ToolCardShell>
  );
}

/** plan：计划卡（FR-06-01 消息内呈现 + 批准按钮冻结快照 §8.6）。 */
export function PlanCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const items = Array.isArray(args.items) ? (args.items as PlanItemArg[]) : [];
  const statusLabel: Record<string, string> = {
    pending: "待办",
    in_progress: "进行中",
    completed: "已完成",
  };
  return (
    <ToolCardShell {...props} title="任务计划">
      <ul className="space-y-0.5">
        {items.map((item) => (
          <li key={item.id} className="flex items-start gap-1.5">
            <span
              className={
                item.status === "completed"
                  ? "text-emerald-400"
                  : item.status === "in_progress"
                    ? "text-[var(--muted-foreground)]"
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

/** plan 工具 items 参数里单个条目的形状。 */
interface PlanItemArg {
  id: string;
  text: string;
  status: string;
}

/** ask：极简状态卡（问题本体在 AskPanel 呈现，§10.3 的 M1 落地形态）。 */
export function AskCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} title="向用户询问">
      <p className="text-[var(--foreground)]">{str(args, "title")}</p>
      <p className="text-[var(--muted-foreground)]">回答在下方询问面板中进行</p>
    </ToolCardShell>
  );
}
