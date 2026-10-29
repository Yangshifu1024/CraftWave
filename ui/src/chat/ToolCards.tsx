/**
 * 正式工具卡片（§10.3 规格，14 号票；界面像素级复刻 ZCode 的单行工具摘要行
 * ——对照其 ToolCallBlocks/ToolSummaryRow：动作图标 + 类型词（medium）+
 * 主参数（subtlest）+ 状态 + hover 显现的展开箭头）；详情区（点击展开）
 * 承载参数全文、结果摘要与 toolres 边车（§7.5）的完整数据，左侧竖导线
 * 缩进（ml-2 border-l pl-3.5）。错误态默认展开。
 *
 * 数据源：part props（args/result/isError）+ useToolArgsStatus（S1 钉死）。
 * 状态机：参数准备中（半截 JSON）→ 运行中 → 完成/错误（result 直写驱动）。
 * 15 号票：终态卡片按 callId 拉 toolres 边车，折叠展示 detail/data 全量。
 */

import { type ToolCallMessagePartProps, useToolArgsStatus } from "@assistant-ui/react";
import {
  ChevronRight,
  FileText,
  FolderOpen,
  ListTodo,
  Loader,
  MessageCircleQuestion,
  PenLine,
  Search,
  SquareTerminal,
} from "lucide-react";
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

/** 卡片通用 props：assistant-ui 传入的工具调用 part 数据 + 摘要行内容。 */
interface CardProps extends ToolCallMessagePartProps<CardArgs, unknown> {
  /** 摘要行的类型词（读取/写入/执行…，ZCode tool-summary-kind-label 口径）。 */
  kind: string;
  /** 摘要行的动作图标（lucide 线性图标，size-4）。 */
  icon: React.ReactNode;
  /** 摘要行主文本（路径 / 命令 / 模式等）。 */
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

/** 单行摘要壳（ZCode ToolSummaryRow）：整行可点击展开/收起；类型词运行中
 *  换扫光，箭头 hover 显现、展开旋转 90°；详情区左侧竖导线缩进。 */
function ToolCardShell({
  kind,
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
    <div
      data-testid={`tool-card-${toolCallId}`}
      data-phase={phase}
      className="w-full text-ui-base"
      data-zcode-tool-stream-animate="true"
    >
      {/* 摘要行：图标 + 类型词 + 主参数 + 徽标 + 状态（点击展开详情） */}
      <button
        type="button"
        data-phase-label={phase}
        onClick={() => setOpen((v) => !v)}
        className="group/tool-summary inline-flex max-w-full cursor-pointer items-center gap-2 self-start rounded py-0.5 text-left text-ui-base transition-colors"
        aria-expanded={open}
      >
        <span aria-hidden className="shrink-0 text-foreground-subtlest">
          {icon}
        </span>
        <span
          className={`shrink-0 whitespace-nowrap font-medium ${
            phase === "running" ? "animated-gradient-text" : "text-foreground-subtlest"
          }`}
        >
          {kind}
        </span>
        <span className="flex min-w-0 max-w-full items-center gap-2 text-foreground-subtlest">
          <span className="min-w-0 truncate font-mono">{summary || "（等待参数…）"}</span>
        </span>
        {badges}
        {phase === "running" && (
          <span
            data-testid="tool-spinner"
            className="shrink-0 animate-spin text-foreground-subtlest"
          >
            <Loader className="size-4" />
          </span>
        )}
        {phase === "preparing" && <span className="shrink-0 text-foreground-subtlest">…</span>}
        {phase === "error" && <span className="shrink-0 font-medium text-destructive">失败</span>}
        <ChevronRight
          aria-hidden
          className={`size-4 shrink-0 text-foreground-subtlest transition-all duration-200 ease-out ${
            open
              ? "rotate-90 opacity-100"
              : "rotate-0 opacity-0 group-hover/tool-summary:opacity-100"
          }`}
        />
      </button>
      {/* 详情区：参数原文 + 卡片专属排版 + 结果摘要 + 边车全量数据（竖导线缩进） */}
      {open && (
        <div className="ml-2 mt-1 space-y-1 border-l border-border pl-3.5 pt-1 text-ui-sm">
          {children}
          {phase === "preparing" && (
            <pre className="max-h-24 overflow-hidden whitespace-pre-wrap break-all text-foreground-subtle">
              {argsText || "（等待参数…）"}
            </pre>
          )}
          {hasResult && (
            <p className={isError ? "text-destructive" : "text-foreground-subtle"}>
              {resultSummary}
            </p>
          )}
          {detailJson !== null && detailJson !== "null" && (
            <details className="rounded border border-border bg-surface px-2 py-1">
              <summary className="cursor-default select-none text-foreground-subtle">
                详细数据
              </summary>
              <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-all text-foreground-subtle">
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

/** 增加行数徽标（diff-added 色 +N，ZCode text-diff-added 口径）。 */
const PlusBadge = ({ n }: { n: number }) => (
  <span className="shrink-0 font-mono text-diff-added">+{n}</span>
);
/** 删除行数徽标（diff-removed 色 −N）。 */
const MinusBadge = ({ n }: { n: number }) => (
  <span className="shrink-0 font-mono text-diff-removed">−{n}</span>
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
    <ToolCardShell
      {...props}
      kind="读取"
      icon={<FileText className="size-4" />}
      summary={str(args, "path")}
    >
      <p className="text-foreground-subtle">{range}</p>
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
      kind="写入"
      icon={<PenLine className="size-4" />}
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
      kind="编辑"
      icon={<PenLine className="size-4" />}
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
    <ToolCardShell
      {...props}
      kind="列目录"
      icon={<FolderOpen className="size-4" />}
      summary={target}
    />
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
      kind="搜索"
      icon={<Search className="size-4" />}
      summary={`/${pattern}/ ${suffix}`.trim()}
    >
      {suffix === "" && <p className="text-foreground-subtle">（全项目搜索）</p>}
    </ToolCardShell>
  );
}

/** command：命令行 monospace（摘要即命令文本）。 */
export function CommandCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell
      {...props}
      kind="执行"
      icon={<SquareTerminal className="size-4" />}
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
      kind="计划"
      icon={<ListTodo className="size-4" />}
      summary={`任务计划（${done}/${items.length}）`}
    >
      <ul className="space-y-0.5">
        {items.map((item) => (
          <li key={item.id} className="flex items-start gap-1.5">
            <span
              className={item.status === "completed" ? "text-success" : "text-foreground-subtle"}
            >
              {item.status === "completed" ? "●" : item.status === "in_progress" ? "◐" : "○"}
            </span>
            <span className="text-foreground">
              {item.text}
              <span className="ml-1 text-foreground-subtle">
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
      kind="提问"
      icon={<MessageCircleQuestion className="size-4" />}
      summary={str(args, "title")}
    >
      <p className="text-foreground-subtle">回答在下方询问面板中进行</p>
    </ToolCardShell>
  );
}
