/**
 * 正式工具卡片（§10.3 规格，14 号票；17 号后按参考样式改版）：
 * toolkit 条目的 render 组件，统一为「单行摘要 + 点击展开详情」的工作流
 * 风格——摘要行 = 动作图标 + 主参数（路径/命令/模式）+ diff 统计徽标 +
 * 状态（运行中文案扫光 / 错误感叹 / 静默完成）；详情区（点击展开）承载参数
 * 全文、结果摘要与 toolres 边车（§7.5）的完整数据。错误态默认展开。
 *
 * 数据源：part props（args/result/isError）+ useToolArgsStatus（S1 钉死）。
 * 状态机：参数准备中（半截 JSON）→ 运行中 → 完成/错误（result 直写驱动）。
 * 15 号票：终态卡片按 callId 拉 toolres 边车，折叠展示 detail/data 全量。
 */

import { type ToolCallMessagePartProps, useToolArgsStatus } from "@assistant-ui/react";
import {
  ChevronRight,
  CircleAlert,
  ClipboardList,
  FilePenLine,
  FolderTree,
  MessageCircleQuestion,
  Search,
  Terminal,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useSessionScope } from "@/chat/session-scope";
import { Button } from "@/components/ui/button";
import * as client from "@/ipc/client";
import type { ToolOutcomeJson } from "@/ipc/types";

/** 卡片通用状态判定（与 mirror 原型同口径，供测试直接断言）。 */
export function cardPhase(
  statusType: "running" | "complete" | "incomplete" | "requires-action",
  hasResult: boolean,
  argsComplete: boolean,
): "preparing" | "running" | "done" | "cancelled" | "awaiting" {
  if (hasResult) return "done";
  if (statusType === "requires-action") return "awaiting";
  if (statusType !== "running") return "cancelled";
  if (statusType === "running" && !argsComplete) return "preparing";
  return "running";
}

/** 卡片读取工具参数用的宽泛类型（args 是 JSON 解析结果）。 */
type CardArgs = Record<string, unknown>;

/** 卡片通用 props：assistant-ui 传入的工具调用 part 数据 + 摘要行内容。 */
interface CardProps extends ToolCallMessagePartProps<CardArgs, unknown> {
  /** 与侧栏一致的单色动作图标。 */
  icon: React.ReactNode;
  label: string;
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

/** 展开状态全局记忆（ZCode 同款，模块级 Map）：切换会话/重挂载后保持，
 * 长会话缓慢增长可接受（ZCode 亦不清理）。导出供测试隔离清理。 */
export const toolCardExpandedMemory = new Map<string, boolean>();

/** 单行摘要壳：整行可点击展开/收起；详情区含参数全文、结果与边车数据。 */
function ToolCardShell({
  icon,
  label,
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
  const [open, setOpen] = useState(
    () => toolCardExpandedMemory.get(toolCallId) ?? phase === "error",
  );
  useEffect(() => {
    if (phase === "error" && !toolCardExpandedMemory.has(toolCallId)) setOpen(true);
  }, [phase, toolCallId]);
  const toggle = () =>
    setOpen((v) => {
      toolCardExpandedMemory.set(toolCallId, !v);
      return !v;
    });
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const [copyState, setCopyState] = useState("");
  const detailJson = outcome
    ? JSON.stringify(outcome.detail ?? outcome.data ?? null, null, 2)
    : null;

  const resultSummary =
    outcome?.uiSummary ||
    (typeof result === "string"
      ? result
      : result !== undefined && typeof result === "object" && result !== null
        ? String((result as { summary?: string }).summary ?? "")
        : "") ||
    "";

  return (
    <div
      data-testid={`tool-card-${toolCallId}`}
      data-phase={phase}
      className="min-w-0 text-ui-base"
    >
      {/* 摘要行：图标 + 主参数 + 徽标 + 状态（点击展开详情） */}
      <button
        type="button"
        data-phase-label={phase}
        onClick={toggle}
        className="group/tool flex max-w-full items-center gap-2 rounded-md px-1 py-1 text-left hover:bg-[var(--accent)]/60"
        aria-expanded={open}
        title={summary}
      >
        <span aria-hidden className="shrink-0 text-[var(--muted-foreground)]">
          {icon}
        </span>
        <span
          className={`shrink-0 font-medium ${phase === "running" ? "cw-shimmer-text" : "text-[var(--muted-foreground)]"}`}
        >
          {label}
        </span>
        <span className="min-w-0 truncate text-[var(--muted-foreground)]">
          {summary || "（等待参数…）"}
        </span>
        {badges}
        {/* 运行态：细进度条扫光（ZCode 同款，不用转圈 spinner） */}
        {phase === "running" && (
          <span data-testid="tool-spinner" className="cw-progress h-1 w-16 shrink-0 rounded-full" />
        )}
        {phase === "preparing" && <span className="shrink-0 text-[var(--cw-dim)]">…</span>}
        {phase === "cancelled" && (
          <span className="shrink-0 text-ui-sm text-[var(--muted-foreground)]">已中断</span>
        )}
        {phase === "awaiting" && (
          <span className="shrink-0 text-ui-sm text-[var(--muted-foreground)]">等待确认</span>
        )}
        {phase === "error" && (
          <span
            title={resultSummary || argsText}
            className="flex shrink-0 cursor-help items-center gap-1 font-medium text-[var(--destructive)] underline decoration-dotted underline-offset-4"
          >
            <CircleAlert size={12} aria-hidden className="shrink-0" />
            失败
          </span>
        )}
        <ChevronRight
          size={12}
          aria-hidden
          className={`shrink-0 text-[var(--cw-dim)] transition-[transform,opacity] group-hover/tool:opacity-100 group-focus-visible/tool:opacity-100 ${open ? "rotate-90 opacity-100" : "opacity-0"}`}
        />
      </button>
      {/* 详情区：参数原文 + 卡片专属排版 + 结果摘要 + 边车全量数据 */}
      {open && (
        <div className="mt-2 ml-2 min-w-0 space-y-2 border-l border-[var(--border)] pl-3.5 text-ui-caption leading-relaxed [overflow-wrap:anywhere]">
          {children}
          <details open={phase === "preparing"}>
            <summary className="cursor-pointer text-ui-sm text-[var(--muted-foreground)]">
              调用参数
            </summary>
            <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap font-mono text-ui-sm text-[var(--muted-foreground)]">
              {argsText || "（等待参数…）"}
            </pre>
          </details>
          {hasResult && (
            <p
              className={`max-h-60 overflow-auto whitespace-pre-wrap ${isError ? "text-[var(--destructive)]" : "text-[var(--muted-foreground)]"}`}
            >
              {resultSummary}
            </p>
          )}
          {phase === "error" && resultSummary && (
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="ghost"
                size="sm"
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(resultSummary);
                    setCopyState("已复制");
                  } catch {
                    setCopyState("复制失败，请选择错误文本复制");
                  }
                }}
              >
                复制错误
              </Button>
              <span role="status" className="text-[var(--muted-foreground)]">
                {copyState}
              </span>
            </div>
          )}
          {detailJson !== null && detailJson !== "null" && (
            <details>
              <summary className="cursor-default select-none text-[var(--muted-foreground)]">
                详细数据
              </summary>
              <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap font-mono text-ui-sm text-[var(--muted-foreground)]">
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
    <ToolCardShell {...props} icon={<Search size={14} />} label="读取" summary={str(args, "path")}>
      {range && <p className="text-[var(--muted-foreground)]">{range}</p>}
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
      icon={<FilePenLine size={14} />}
      label="写入"
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
      icon={<FilePenLine size={14} />}
      label="编辑"
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
  return (
    <ToolCardShell {...props} icon={<FolderTree size={14} />} label="列出文件" summary={target} />
  );
}

/** grep：正则模式 + glob 过滤 + 搜索起点。 */
export function GrepCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const pattern = str(args, "pattern");
  const suffix = [str(args, "glob"), str(args, "path") ? `@ ${str(args, "path")}` : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <ToolCardShell
      {...props}
      icon={<Search size={14} />}
      label="搜索"
      summary={`/${pattern}/ ${suffix}`.trim()}
    >
      {suffix === "" && <p className="text-[var(--muted-foreground)]">（全项目搜索）</p>}
    </ToolCardShell>
  );
}

/** command：命令行 monospace（摘要即命令文本）。 */
export function CommandCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell
      {...props}
      icon={<Terminal size={14} />}
      label="执行"
      summary={str(args, "command")}
    />
  );
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
    <ToolCardShell
      {...props}
      icon={<ClipboardList size={14} />}
      label="计划"
      summary={`任务计划（${done}/${items.length}）`}
    >
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
    <ToolCardShell
      {...props}
      icon={<MessageCircleQuestion size={14} />}
      label="询问"
      summary={str(args, "title")}
    >
      <p className="text-[var(--muted-foreground)]">回答在下方询问面板中进行</p>
    </ToolCardShell>
  );
}
