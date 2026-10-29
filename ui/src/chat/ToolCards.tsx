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
  FileCode2,
  FileCog,
  FileJson2,
  FilePenLine,
  FileText,
  FolderTree,
  MessageCircleQuestion,
  Search,
  SquareTerminal,
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
  /** 运行中的动作词替换（ZCode 终端行「正在执行」同款；缺省沿用 label）。 */
  runningLabel?: string;
  /** 摘要行主文本（路径 / 命令 / 模式等），同时作悬浮提示。 */
  summary: string;
  /** 摘要行主文本的富渲染（ZCode 蓝文件名 + 灰目录拆分）；缺省灰字 summary。 */
  summaryNode?: React.ReactNode;
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
  runningLabel,
  summary,
  summaryNode,
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
      {/* 摘要行（ZCode ToolSummaryRow 同款）：整行 self-start 收缩宽度、gap-2、
          无 hover 底色——运行指示在动作词流光，悬停只露出 chevron。点击展开详情 */}
      <button
        type="button"
        data-phase-label={phase}
        onClick={toggle}
        className="group/tool inline-flex max-w-full cursor-pointer items-center gap-2 self-start rounded-sm text-left transition-colors focus-visible:ring-2 focus-visible:ring-[var(--ring)]/30 focus-visible:outline-none"
        aria-expanded={open}
        title={summary}
      >
        <span
          aria-hidden
          className="shrink-0 text-[var(--foreground-subtlest)] [&_svg]:text-[var(--foreground-subtlest)]"
        >
          {icon}
        </span>
        {/* 动作词：完成态最淡档 medium（不喧宾夺主），运行中流光扫动表达进行中 */}
        <span
          className={`shrink-0 font-medium whitespace-nowrap ${phase === "running" ? "cw-shimmer-text" : "text-[var(--foreground-subtlest)]"}`}
        >
          {phase === "running" && runningLabel ? runningLabel : label}
        </span>
        {summaryNode ?? (
          <span className="min-w-0 truncate text-[var(--foreground-subtlest)]">
            {summary || "（等待参数…）"}
          </span>
        )}
        {badges}
        {/* 运行态：细进度条扫光（不用转圈 spinner） */}
        {phase === "running" && (
          <span data-testid="tool-spinner" className="cw-progress h-1 w-16 shrink-0 rounded-full" />
        )}
        {phase === "preparing" && <span className="shrink-0 text-[var(--cw-dim)]">…</span>}
        {phase === "cancelled" && (
          <span className="shrink-0 text-ui-sm text-[var(--foreground-subtlest)]">已中断</span>
        )}
        {phase === "awaiting" && (
          <span className="shrink-0 text-ui-sm text-[var(--foreground-subtlest)]">等待确认</span>
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
          size={16}
          aria-hidden
          className={`shrink-0 text-[var(--foreground-subtlest)] opacity-0 transition-[transform,opacity] duration-200 ease-out group-hover/tool:opacity-100 group-focus-visible/tool:opacity-100 ${open ? "rotate-90 opacity-100" : ""}`}
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

/** diff 统计（ZCode renderDiffCount 同款）：等宽纯彩字、零值整项隐藏、
 *  ASCII 减号；+N 增绿 / −N 删红。 */
function DiffCount({ added, removed }: { added: number; removed: number }) {
  if (added <= 0 && removed <= 0) return null;
  return (
    <span className="inline-flex shrink-0 items-center gap-1 font-mono leading-none tabular-nums whitespace-nowrap">
      {added > 0 && <span className="text-[var(--diff-added)]">+{added}</span>}
      {removed > 0 && <span className="text-[var(--diff-removed)]">-{removed}</span>}
    </span>
  );
}

/** 文件类型图标（ZCode FileDisplayIcon 的轻量替代）：按扩展名换图标与色。 */
function FileTypeIcon({ path }: { path: string }) {
  const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
  if (ext === "rs")
    return <FileCog size={16} aria-hidden className="size-4 shrink-0 text-[#deA584]" />;
  if (ext === "ts" || ext === "tsx")
    return <FileCode2 size={16} aria-hidden className="size-4 shrink-0 text-[#3178c6]" />;
  if (ext === "js" || ext === "jsx" || ext === "mjs")
    return <FileCode2 size={16} aria-hidden className="size-4 shrink-0 text-[#b8860b]" />;
  if (ext === "json" || ext === "toml" || ext === "yaml" || ext === "yml")
    return <FileJson2 size={16} aria-hidden className="size-4 shrink-0 text-[#b8860b]" />;
  if (ext === "md")
    return <FileText size={16} aria-hidden className="size-4 shrink-0 text-[#519aba]" />;
  return <FileText size={16} aria-hidden className="size-4 shrink-0 text-[var(--cw-dim)]" />;
}

/** 文件 chip（ZCode ReadFileChip 同款）：彩色类型图标 + 文件名（subtle 中灰，
 *  不是链接蓝）truncate；目录路径单独走次要槽。 */
function FileChip({ path }: { path: string }) {
  const sep = path.lastIndexOf("/");
  const name = sep >= 0 ? path.slice(sep + 1) : path;
  const dir = sep >= 0 ? path.slice(0, sep + 1) : "";
  if (!name) {
    return <span className="min-w-0 truncate text-[var(--foreground-subtlest)]">{path}</span>;
  }
  return (
    <span className="inline-flex max-w-full min-w-0 items-center gap-1.5" title={path}>
      <FileTypeIcon path={name} />
      <span className="min-w-0 truncate text-[var(--foreground-subtle)]">{name}</span>
      {dir && <span className="min-w-0 truncate text-[var(--foreground-subtlest)]">{dir}</span>}
    </span>
  );
}

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
      icon={<Search size={16} />}
      label="读取"
      runningLabel="正在读取"
      summary={str(args, "path")}
      summaryNode={<FileChip path={str(args, "path")} />}
    >
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
      icon={<FilePenLine size={16} />}
      label="写入"
      runningLabel="正在写入"
      summary={str(args, "path")}
      summaryNode={<FileChip path={str(args, "path")} />}
      badges={<DiffCount added={added} removed={0} />}
    />
  );
}

/** edit：路径 + +N/−N 统计（newText/oldText 行数）。 */
export function EditCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "newText"));
  const removed = lineCount(str(args, "oldText"));
  return (
    <ToolCardShell
      {...props}
      icon={<FilePenLine size={16} />}
      label="编辑"
      runningLabel="正在编辑"
      summary={str(args, "path")}
      summaryNode={<FileChip path={str(args, "path")} />}
      badges={<DiffCount added={added} removed={removed} />}
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
      icon={<FolderTree size={16} />}
      label="列出文件"
      runningLabel="正在列出文件"
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
      icon={<Search size={16} />}
      label="搜索"
      runningLabel="正在搜索"
      summary={`/${pattern}/ ${suffix}`.trim()}
    >
      {suffix === "" && <p className="text-[var(--muted-foreground)]">（全项目搜索）</p>}
    </ToolCardShell>
  );
}

/** command：命令行（摘要 = 命令文本 font-sans 截断）；动作词随状态切换
 *  （ZCode 终端行同款：完成态「终端」、运行中「正在执行」）。 */
export function CommandCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell
      {...props}
      icon={<SquareTerminal size={16} />}
      label="终端"
      runningLabel="正在执行"
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
      icon={<ClipboardList size={16} />}
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
      icon={<MessageCircleQuestion size={16} />}
      label="询问"
      summary={str(args, "title")}
    >
      <p className="text-[var(--muted-foreground)]">回答在下方询问面板中进行</p>
    </ToolCardShell>
  );
}
