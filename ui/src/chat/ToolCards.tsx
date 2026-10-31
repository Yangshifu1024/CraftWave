/**
 * 正式工具卡片（按参考样式改版）：
 * toolkit 条目的 render 组件，统一为「单行摘要 + 点击展开详情」的工作流
 * 风格——摘要行 = 动作图标 + 主参数（路径/命令/模式）+ diff 统计徽标 +
 * 状态（运行中文案扫光 / 错误感叹 / 静默完成）；详情区（点击展开）承载参数
 * 全文、结果摘要与 toolres 边车的完整数据。错误态默认展开。
 *
 * 数据源：part props（args/result/isError）+ useToolArgsStatus（S1 钉死）。
 * 状态机：参数准备中（半截 JSON）→ 运行中 → 完成/错误（result 直写驱动）。
 * 终态卡片按 callId 拉 toolres 边车，折叠展示 detail/data 全量。
 */

import { type ToolCallMessagePartProps, useToolArgsStatus } from "@assistant-ui/react";
import {
  Bot,
  Calculator,
  ChevronRight,
  CircleAlert,
  ClipboardList,
  Eye,
  FileCode2,
  FileCog,
  FileJson2,
  FilePenLine,
  FileStack,
  FileText,
  FolderSearch,
  FolderTree,
  Globe,
  MessageCircleQuestion,
  Plug,
  Search,
  SquareTerminal,
  Zap,
} from "lucide-react";
import { useEffect, useState } from "react";
import { HtmlPreview } from "@/chat/HtmlPreview";
import { Markdown } from "@/chat/Markdown";
import { EMPTY_PLAN_STATE, usePlanStateStore } from "@/chat/plan-state";
import { useSessionScope } from "@/chat/session-scope";
import { Button } from "@/components/ui/button";
import { t, useT } from "@/i18n/core";
import * as client from "@/ipc/client";
import type { ToolOutcome } from "@/ipc/types";
import { auditOf, type ToolAuditInfo, useAuditsStore } from "@/stores/audits";
import { outcomeLabel, useSubagentsStore } from "@/stores/subagents";

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

/** 终态卡片的边车 detail：按 callId 拉一次，缺文件静默（summary 降级）。 */
function useToolOutcomeDetail(toolCallId: string, settled: boolean): ToolOutcome | null {
  const sessionId = useSessionScope();
  const [outcome, setOutcome] = useState<ToolOutcome | null>(null);
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
  const t = useT();
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
            {summary || t("toolcards.awaitingArgs")}
          </span>
        )}
        {badges}
        {/* 运行态：细进度条扫光（不用转圈 spinner） */}
        {phase === "running" && (
          <span data-testid="tool-spinner" className="cw-progress h-1 w-16 shrink-0 rounded-full" />
        )}
        {phase === "preparing" && <span className="shrink-0 text-[var(--cw-dim)]">…</span>}
        {phase === "cancelled" && (
          <span className="shrink-0 text-ui-sm text-[var(--foreground-subtlest)]">
            {t("toolcards.interrupted")}
          </span>
        )}
        {phase === "awaiting" && (
          <span className="shrink-0 text-ui-sm text-[var(--foreground-subtlest)]">
            {t("toolcards.awaitingConfirm")}
          </span>
        )}
        {phase === "error" && (
          <span
            title={resultSummary || argsText}
            className="flex shrink-0 cursor-help items-center gap-1 font-medium text-[var(--destructive)] underline decoration-dotted underline-offset-4"
          >
            <CircleAlert size={12} aria-hidden className="shrink-0" />
            {t("toolcards.failed")}
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
              {t("toolcards.argsTitle")}
            </summary>
            <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap font-mono text-ui-sm text-[var(--muted-foreground)]">
              {argsText || t("toolcards.awaitingArgs")}
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
                    setCopyState(t("toolcards.copied"));
                  } catch {
                    setCopyState(t("toolcards.copyFailed"));
                  }
                }}
              >
                {t("toolcards.copyError")}
              </Button>
              <span role="status" className="text-[var(--muted-foreground)]">
                {copyState}
              </span>
            </div>
          )}
          {detailJson !== null && detailJson !== "null" && (
            <details>
              <summary className="cursor-default select-none text-[var(--muted-foreground)]">
                {t("toolcards.detailTitle")}
              </summary>
              <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap font-mono text-ui-sm text-[var(--muted-foreground)]">
                {outcome?.truncated ? t("toolcards.detailTruncated5mb") : detailJson}
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

/** read 图片态边车 data 的形状（kind=image 结果，含 base64）。 */
interface ImageReadOutcome {
  kind?: unknown;
  path?: unknown;
  mediaType?: unknown;
  base64?: unknown;
}

/** read：路径 + 行区间（详情含区间说明）。.docx/.xlsx/.pdf 等文档
 *  扩展名进入文档态（徽标 + 页数/sheet 摘要 + 内容折叠预览 + warnings 黄条）。
 *  kind="image" 结果进入图片态（缩略图展示，detail 边车取 base64）。 */
export function ReadCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const ext = docExtension(str(args, "path"));
  if (ext) return <DocReadCard {...props} ext={ext} />;
  // 图片态判定走边车 data（结果终态后拉取；kind="image" 的帧面 result 是
  // 模型通道压缩文本，base64 本体只在 detail 边车——恢复链照常可用）
  const { result, isError } = props;
  const settled = result !== undefined && !isError;
  return <ReadImageOrTextCard {...props} settled={settled} />;
}

/** read 的图片/文本双态内层（hook 拉边车后分流；条件渲染不违反 hooks 规则
 *  ——拉取发生在组件顶层）。 */
function ReadImageOrTextCard({
  settled,
  ...props
}: ToolCallMessagePartProps<CardArgs, unknown> & { settled: boolean }) {
  const outcome = useToolOutcomeDetail(props.toolCallId, settled);
  const args = (props.args ?? {}) as CardArgs;
  const data = (outcome?.data ?? null) as ImageReadOutcome | null;
  if (data?.kind === "image") {
    const mediaType = typeof data.mediaType === "string" ? data.mediaType : "image/png";
    const base64 = typeof data.base64 === "string" ? data.base64 : null;
    return (
      <ToolCardShell
        {...props}
        icon={<Search size={16} />}
        label={t("toolcards.read")}
        runningLabel={t("toolcards.reading")}
        summary={str(args, "path")}
        summaryNode={<FileChip path={str(args, "path")} />}
      >
        {base64 !== null ? (
          <img
            data-testid="read-image-thumb"
            src={`data:${mediaType};base64,${base64}`}
            alt={outcome?.uiSummary ?? t("toolcards.readImageAlt")}
            className="max-h-64 max-w-full rounded-lg border border-[var(--border)] object-contain"
          />
        ) : (
          <p className="text-ui-sm text-[var(--muted-foreground)]">
            {t("toolcards.readImageNoThumb")}
          </p>
        )}
        {outcome?.uiSummary && (
          <p className="text-ui-sm text-[var(--muted-foreground)]">{outcome.uiSummary}</p>
        )}
      </ToolCardShell>
    );
  }
  const offset = num(args, "offset");
  const limit = num(args, "limit");
  const range =
    offset !== undefined || limit !== undefined
      ? t("toolcards.readRange", {
          offset: offset ?? 1,
          limit: limit !== undefined ? t("toolcards.readLimitLines", { limit }) : "",
        })
      : "";
  return (
    <ToolCardShell
      {...props}
      icon={<Search size={16} />}
      label={t("toolcards.read")}
      runningLabel={t("toolcards.reading")}
      summary={str(args, "path")}
      summaryNode={<FileChip path={str(args, "path")} />}
    >
      {range && <p className="text-[var(--muted-foreground)]">{range}</p>}
    </ToolCardShell>
  );
}

/** 文档扩展名清单（与后端 tools/doc.rs detect_format 对齐）。 */
const DOC_EXTENSIONS = ["docx", "xlsx", "xlsm", "xlsb", "xls", "ods", "pdf"];

/** 按路径扩展名判定文档格式（大小写不敏感；非文档返回 null）。 */
function docExtension(path: string): string | null {
  const dot = path.lastIndexOf(".");
  if (dot < 0) return null;
  const ext = path.slice(dot + 1).toLowerCase();
  return DOC_EXTENSIONS.includes(ext) ? ext : null;
}

/** 文档态徽标配色：格式缩写（与 FileTypeIcon 的文件色语义一致）。 */
function docBadgeClass(ext: string): string {
  switch (ext) {
    case "pdf":
      return "bg-[var(--destructive)]/10 text-[var(--destructive)]";
    case "docx":
      return "bg-[#3178c6]/10 text-[#3178c6]";
    default:
      return "bg-emerald-600/10 text-emerald-600 dark:text-emerald-400";
  }
}

/** read 文档态边车 data 的形状（文档管线结构化输出，字段随格式可选）。 */
interface DocReadOutcome {
  kind?: unknown;
  content?: unknown;
  truncated?: unknown;
  resumeHint?: unknown;
  paragraphCount?: unknown;
  tableCount?: unknown;
  pageCount?: unknown;
  sheets?: unknown;
}

/** 文档读取卡：扩展名徽标 + 页数/sheet/段落摘要 + 内容 Markdown 折叠预览
 *  （前 80 行）+ truncated 续读提示行（提示模型，非按钮）+ warnings 黄条
 *  （降级解析等仅 UI 提示）。 */
function DocReadCard({
  ext,
  ...props
}: ToolCallMessagePartProps<CardArgs, unknown> & { ext: string }) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const { result, isError } = props;
  // 结构化输出在边车 data（帧面 result 是模型通道压缩文本），失败不拉取。
  const settled = result !== undefined && !isError;
  const outcome = useToolOutcomeDetail(props.toolCallId, settled);
  const data = (outcome?.data ?? null) as DocReadOutcome | null;
  const content = typeof data?.content === "string" ? data.content : null;
  const truncated = data?.truncated === true;
  const resumeHint = typeof data?.resumeHint === "string" ? data.resumeHint : null;
  const pageCount = typeof data?.pageCount === "number" ? data.pageCount : undefined;
  const sheets = Array.isArray(data?.sheets)
    ? (data.sheets as unknown[]).filter((s): s is string => typeof s === "string")
    : [];
  const paragraphCount = typeof data?.paragraphCount === "number" ? data.paragraphCount : undefined;
  const tableCount = typeof data?.tableCount === "number" ? data.tableCount : undefined;
  // warnings（仅 UI 提示）：detail 是 JsonValue，按对象形状安全提取。
  const detailRecord =
    outcome?.detail && typeof outcome.detail === "object" && !Array.isArray(outcome.detail)
      ? (outcome.detail as { warnings?: unknown })
      : null;
  const rawWarnings = detailRecord?.warnings;
  const warnings = Array.isArray(rawWarnings)
    ? (rawWarnings as unknown[]).filter((w): w is string => typeof w === "string")
    : [];
  // 内容预览前 80 行（表格与段落足够看清结构，全量走详细数据）。
  const preview =
    content !== null
      ? content.split("\n").slice(0, 80).join("\n") +
        (content.split("\n").length > 80 ? t("toolcards.previewCut", { n: 80 }) : "")
      : null;
  // 参数区说明：pdf 页码 / xlsx sheet。
  const page = num(args, "page");
  const pages = Array.isArray(args.pages) ? (args.pages as number[]) : null;
  const sheet = str(args, "sheet");
  const rangeText = [
    page !== undefined ? t("toolcards.docPage", { page }) : undefined,
    pages !== null && pages.length === 2
      ? t("toolcards.docPageRange", { from: pages[0], to: pages[1] })
      : undefined,
    sheet ? t("toolcards.docSheet", { name: sheet }) : undefined,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <ToolCardShell
      {...props}
      icon={<Search size={16} />}
      label={t("toolcards.read")}
      runningLabel={t("toolcards.reading")}
      summary={str(args, "path")}
      summaryNode={<FileChip path={str(args, "path")} />}
      badges={
        <span
          data-testid="doc-format-badge"
          className={`shrink-0 rounded px-1.5 py-0.5 text-ui-sm font-medium uppercase ${docBadgeClass(ext)}`}
        >
          {ext}
        </span>
      }
    >
      {rangeText && <p className="text-[var(--muted-foreground)]">（{rangeText}）</p>}
      {/* 页数 / sheet 数 / 段落表格摘要（终态后可见） */}
      {(pageCount !== undefined || sheets.length > 0 || paragraphCount !== undefined) && (
        <p data-testid="doc-summary" className="text-ui-sm text-[var(--muted-foreground)]">
          {pageCount !== undefined && t("toolcards.docPageCount", { count: pageCount })}
          {sheets.length > 0 &&
            `${pageCount !== undefined ? " · " : ""}${t("toolcards.docSheetCount", { count: sheets.length })}`}
          {paragraphCount !== undefined &&
            `${pageCount !== undefined || sheets.length > 0 ? " · " : ""}${t("toolcards.docParaCount", { count: paragraphCount })}${tableCount !== undefined ? ` · ${t("toolcards.docTableCount", { count: tableCount })}` : ""}`}
        </p>
      )}
      {/* warnings 黄条（降级解析等，仅 UI 不进模型文本） */}
      {warnings.length > 0 && (
        <div
          data-testid="doc-warnings"
          className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-2 py-1 text-ui-sm text-amber-700 dark:text-amber-400"
        >
          {warnings.map((w) => (
            <p key={w}>{w}</p>
          ))}
        </div>
      )}
      {/* truncated 续读提示：给模型看的信息行（非按钮），帮助用户理解下一步 */}
      {truncated && (
        <p data-testid="doc-resume-hint" className="text-ui-sm text-[var(--muted-foreground)]">
          {resumeHint ?? t("toolcards.docTruncatedDefault")}
          {t("toolcards.docTruncatedHint")}
        </p>
      )}
      {preview !== null && (
        <details>
          <summary className="cursor-pointer text-[var(--muted-foreground)]">
            {t("toolcards.docPreview")}
          </summary>
          <div className="mt-1 max-h-60 overflow-auto rounded border border-[var(--border)] p-2">
            <Markdown>{preview}</Markdown>
          </div>
        </details>
      )}
    </ToolCardShell>
  );
}

/** write：新文件 + +N 徽标（content 行数）。 */
export function WriteCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "content"));
  return (
    <ToolCardShell
      {...props}
      icon={<FilePenLine size={16} />}
      label={t("toolcards.write")}
      runningLabel={t("toolcards.writing")}
      summary={str(args, "path")}
      summaryNode={<FileChip path={str(args, "path")} />}
      badges={<DiffCount added={added} removed={0} />}
    />
  );
}

/** edit：路径 + +N/−N 统计（newText/oldText 行数）。 */
export function EditCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "newText"));
  const removed = lineCount(str(args, "oldText"));
  return (
    <ToolCardShell
      {...props}
      icon={<FilePenLine size={16} />}
      label={t("toolcards.edit")}
      runningLabel={t("toolcards.editing")}
      summary={str(args, "path")}
      summaryNode={<FileChip path={str(args, "path")} />}
      badges={<DiffCount added={added} removed={removed} />}
    />
  );
}

/** list_files：目录树（摘要 = 目标目录）。 */
export function ListFilesCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const target = str(args, "path") || t("toolcards.projectRoot");
  return (
    <ToolCardShell
      {...props}
      icon={<FolderTree size={16} />}
      label={t("toolcards.listFiles")}
      runningLabel={t("toolcards.listingFiles")}
      summary={target}
    />
  );
}

/** grep：正则模式 + glob 过滤 + 搜索起点。 */
export function GrepCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const pattern = str(args, "pattern");
  const suffix = [str(args, "glob"), str(args, "path") ? `@ ${str(args, "path")}` : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <ToolCardShell
      {...props}
      icon={<Search size={16} />}
      label={t("toolcards.grep")}
      runningLabel={t("toolcards.grepping")}
      summary={`/${pattern}/ ${suffix}`.trim()}
    >
      {suffix === "" && (
        <p className="text-[var(--muted-foreground)]">{t("toolcards.grepFullProject")}</p>
      )}
    </ToolCardShell>
  );
}

/** command：命令行（摘要 = 命令文本 font-sans 截断）；动作词随状态切换
 *  （ZCode 终端行同款：完成态「终端」、运行中「正在执行」）。
 *  审计行：挂起命令的灰区审计经 audits store 按 callId
 *  关联——running 徽标「审计中」+ 终态徽标（放行/拒绝/转人工），展开详情
 *  区显示简短理由/失败原因（结论与审批卡载荷 audit 字段同源）。 */
export function CommandCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const sessionId = useSessionScope();
  const audits = useAuditsStore((s) => s.bySession);
  const audit = auditOf(audits, sessionId, props.toolCallId);
  return (
    <ToolCardShell
      {...props}
      icon={<SquareTerminal size={16} />}
      label={t("toolcards.terminal")}
      runningLabel={t("toolcards.executing")}
      summary={str(args, "command")}
      badges={audit ? <CommandAuditBadge audit={audit} /> : undefined}
    >
      {audit?.reason && (
        <p
          data-testid="command-audit-note"
          className="mb-1 text-ui-sm text-[var(--muted-foreground)]"
        >
          {t("toolcards.auditNote", { reason: audit.reason })}
        </p>
      )}
    </ToolCardShell>
  );
}

/** 审计五态的徽标文案（running 态随 run 流光语义，其余为终态结论词）。 */
function auditBadgeText(audit: ToolAuditInfo): string {
  switch (audit.state) {
    case "running":
      return t("toolcards.auditRunning");
    case "allow":
      return t("toolcards.auditAllow");
    case "block":
      return t("toolcards.auditBlock");
    case "ask":
      return t("toolcards.auditAsk");
    case "manual":
      return t("toolcards.auditManual");
  }
}

/** command 卡的审计状态徽标（五态配色：running 灰、allow 绿、block/manual
 *  红、ask 橙——与 AskPanel 审计注记同色系）。 */
function CommandAuditBadge({ audit }: { audit: ToolAuditInfo }) {
  const color =
    audit.state === "allow"
      ? "text-emerald-600 dark:text-emerald-400"
      : audit.state === "ask"
        ? "text-[var(--warning)]"
        : audit.state === "running"
          ? "text-[var(--muted-foreground)]"
          : "text-[var(--destructive)]";
  return (
    <span
      data-testid={`command-audit-${audit.state}`}
      className={`shrink-0 text-ui-sm font-medium whitespace-nowrap ${color}`}
    >
      {auditBadgeText(audit)}
    </span>
  );
}

/** plan 工具 items 参数里单个条目的形状。 */
interface PlanItemArg {
  id: string;
  text: string;
  status: string;
}

/**
 * plan：计划卡（消息内呈现；摘要 = 完成数统计）。
 * 展示模型声明的写范围（args.paths，批准时冻结为基线）；基线态标记
 * （已批准 + 基线条目数）从 plan-state store 按所属会话读取（plan:update
 * 驱动，内存态——卡片渲染与 Inspector 计划区块同源）。
 */
export function PlanCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const items = Array.isArray(args.items) ? (args.items as PlanItemArg[]) : [];
  const done = items.filter((i) => i.status === "completed").length;
  const paths = Array.isArray(args.paths) ? (args.paths as string[]) : [];
  const sessionId = useSessionScope();
  const planState = usePlanStateStore((s) => s.bySession[sessionId] ?? EMPTY_PLAN_STATE);
  const statusLabel: Record<string, string> = {
    pending: t("toolcards.planPending"),
    in_progress: t("toolcards.planInProgress"),
    completed: t("toolcards.planCompleted"),
  };
  return (
    <ToolCardShell
      {...props}
      icon={<ClipboardList size={16} />}
      label={t("toolcards.plan")}
      summary={t("toolcards.planSummary", { done, total: items.length })}
    >
      {planState.approved && (
        <p data-testid="plan-card-baseline" className="mb-1 text-ui-sm text-[var(--primary)]">
          {t("toolcards.planApproved")}
          {planState.baselinePaths.length > 0
            ? t("toolcards.planBaseline", { count: planState.baselinePaths.length })
            : t("toolcards.planNoBaseline")}
        </p>
      )}
      {paths.length > 0 && (
        <p className="mb-1 break-all font-mono text-ui-sm text-[var(--muted-foreground)]">
          {t("toolcards.planScope", { paths: paths.join("、") })}
        </p>
      )}
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

/** ask：极简状态卡（问题本体在 AskPanel 呈现）。 */
export function AskCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell
      {...props}
      icon={<MessageCircleQuestion size={16} />}
      label={t("toolcards.ask")}
      summary={str(args, "title")}
    >
      <p className="text-[var(--muted-foreground)]">{t("toolcards.askHint")}</p>
    </ToolCardShell>
  );
}

/**
 * skill 工具卡：技能名 + 加载状态 + 正文折叠预览
 * （终态按 callId 拉边车 data.body，Markdown 只读渲染；加载中显示技能名
 * 扫光）。失败态（未信任/未知技能）沿 Shell 的错误展开展示 uiSummary。
 */
export function SkillCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const { toolCallId, result, isError } = props;
  // 正文来源：边车 data.body（帧面 result 是模型通道压缩文本，不含结构化
  // data——与 read 等卡的边车恢复口径一致）。失败路径不写边车
  // （执行器约定），不拉取。
  const settled = result !== undefined && !isError;
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const rawBody = (outcome?.data as { body?: unknown } | undefined)?.body;
  const body = typeof rawBody === "string" ? rawBody : null;
  const optional = typeof args.args === "string" && args.args.length > 0;
  return (
    <ToolCardShell
      {...props}
      icon={<Zap size={16} />}
      label={t("toolcards.skill")}
      runningLabel={t("toolcards.skillLoading")}
      summary={str(args, "name") + (optional ? t("toolcards.skillWithArgs") : "")}
    >
      {body !== null && (
        <details>
          <summary className="cursor-pointer text-[var(--muted-foreground)]">
            {t("toolcards.bodyPreview")}
          </summary>
          <div className="mt-1 max-h-60 overflow-auto rounded border border-[var(--border)] p-2">
            <Markdown>{body}</Markdown>
          </div>
        </details>
      )}
    </ToolCardShell>
  );
}

/**
 * subagent 工具卡：角色徽标 + task 摘要 + 实时
 * 步数/状态（subagents store 按 callId 关联，sub:step/subFrame 驱动）+
 * `<report>` 折叠全文（subReport 后展开可见）；终态徽标按四象限着色。
 */
export function SubagentCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const { toolCallId, result, isError } = props;
  const t = useT();
  const role = str(args, "role") || t("toolcards.subagentDefaultRole");
  const task = str(args, "task");
  // 按 callId 关联 store 记录（subStarted.callId = 委派工具调用的帧面 id）
  const item = useSubagentsStore((s) =>
    Object.values(s.bySession)
      .flat()
      .find((entry) => entry.callId === toolCallId),
  );
  const setOpenSubState = useSubagentsStore((s) => s.openSubId);
  void setOpenSubState;
  const running = item !== undefined && item.state === "running";
  const turn = item?.turn ?? 0;
  const outcome = item !== undefined && item.state !== "running" ? outcomeLabel(item.state) : "";
  return (
    <ToolCardShell
      {...props}
      icon={<Bot size={16} />}
      label={role}
      runningLabel={t("toolcards.subagentRunning", { role, turn })}
      summary={task.length > 60 ? `${task.slice(0, 60)}…` : task || role}
      badges={
        item !== undefined ? (
          <span
            className={`text-ui-sm tabular-nums ${
              running ? "text-blue-600 dark:text-blue-400" : outcomeColorOf(item.state)
            }`}
          >
            {running ? t("toolcards.subagentTurn", { turn }) : outcome}
          </span>
        ) : undefined
      }
    >
      {task.length > 0 && (
        <div className="mb-1">
          <p className="text-ui-sm text-[var(--muted-foreground)]">{t("toolcards.subagentTask")}</p>
          <p className="whitespace-pre-wrap text-ui-sm">{task}</p>
        </div>
      )}
      {item !== undefined && item.report.length > 0 && (
        <details>
          <summary className="cursor-pointer text-[var(--muted-foreground)]">
            {t("toolcards.subagentReport")}
          </summary>
          <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap rounded border border-[var(--border)] p-2 text-ui-sm">
            {item.report}
          </pre>
        </details>
      )}
      {isError && result !== undefined && (
        <p className="text-ui-sm text-red-600 dark:text-red-400">
          {typeof result === "string" ? result : t("toolcards.subagentAbnormal")}
        </p>
      )}
    </ToolCardShell>
  );
}

/** 终态徽标配色（与 Inspector 子代理页同语义）。 */
function outcomeColorOf(state: string): string {
  switch (state) {
    case "report":
      return "text-emerald-600 dark:text-emerald-400";
    case "partial":
      return "text-amber-600 dark:text-amber-400";
    case "cancelled":
      return "text-[var(--muted-foreground)]";
    case "no_report":
      return "text-red-600 dark:text-red-400";
    default:
      return "";
  }
}

/**
 * MCP 动态工具兜底卡：toolkit 只静态声明 8 个内置工具的专用卡，
 * MCP 服务器的动态工具（归一名运行时才定）无法静态枚举——未命中专用卡的
 * 工具调用一律落本卡（Thread.tsx 在 part.toolUI 为 null 时兜底渲染）。
 * 渲染 = 工具名 + 参数 JSON 折叠 + uiSummary + 错误态，结构与专用卡一致
 * （单行摘要 + 点击展开详情，边车 detail/data 全量）。
 */
export function McpFallbackCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  return (
    <ToolCardShell
      {...props}
      icon={<Plug size={16} />}
      label={t("toolcards.mcpTool")}
      runningLabel={t("toolcards.callingMcpTool")}
      summary={props.toolName}
    />
  );
}

/** web_fetch 边车 data 的形状（{ url, title, content, length }）。 */
interface WebFetchOutcome {
  title?: unknown;
  content?: unknown;
  length?: unknown;
  truncated?: unknown;
}

/**
 * web_fetch 工具卡：URL 摘要 + 终态拉
 * 边车 data——标题 + 正文 Markdown 折叠预览（默认收起，展开渲染前 50 行）。
 * 失败态（SSRF 拒绝/超限）沿 Shell 的错误展开展示 uiSummary。
 */
export function WebFetchCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const { toolCallId, result, isError } = props;
  // 正文来源：边车 data（帧面 result 是模型通道压缩文本，不含结构化 data）。
  // 失败路径不写边车（执行器约定），不拉取。
  const settled = result !== undefined && !isError;
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const data = (outcome?.data ?? null) as WebFetchOutcome | null;
  const title = typeof data?.title === "string" ? data.title : null;
  const content = typeof data?.content === "string" ? data.content : null;
  const length = typeof data?.length === "number" ? data.length : undefined;
  const truncated = data?.truncated === true;
  // 正文预览前 50 行（默认收起，展开前 50 行）
  const preview =
    content !== null
      ? content.split("\n").slice(0, 50).join("\n") +
        (content.split("\n").length > 50 ? t("toolcards.previewCut", { n: 50 }) : "")
      : null;
  return (
    <ToolCardShell
      {...props}
      icon={<Globe size={16} />}
      label={t("toolcards.fetch")}
      runningLabel={t("toolcards.fetching")}
      summary={str(args, "url")}
    >
      {title !== null && <p className="text-ui-sm font-medium text-[var(--foreground)]">{title}</p>}
      {length !== undefined && (
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          {t("toolcards.fetchBodyChars", { length })}
          {truncated ? t("toolcards.fetchBodyTruncated") : ""}
        </p>
      )}
      {preview !== null && (
        <details>
          <summary className="cursor-pointer text-[var(--muted-foreground)]">
            {t("toolcards.bodyPreview")}
          </summary>
          <div className="mt-1 max-h-60 overflow-auto rounded border border-[var(--border)] p-2">
            <Markdown>{preview}</Markdown>
          </div>
        </details>
      )}
    </ToolCardShell>
  );
}

/** http_request 边车 data 的形状（{ status, headers, body, spillPath? }）。 */
interface HttpRequestOutcome {
  status?: unknown;
  headers?: unknown;
  body?: unknown;
  spillPath?: unknown;
  bodyTruncated?: unknown;
}

/**
 * http_request 工具卡：method 徽标 + URL
 * 摘要 + 状态码徽标；详情含响应头折叠、body 折叠与 spill 文件路径
 * （对齐 CommandCard 形态：单行摘要 + 点击展开详情）。
 */
export function HttpRequestCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const { toolCallId, result, isError } = props;
  const method = str(args, "method").toUpperCase() || "GET";
  const settled = result !== undefined && !isError;
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const data = (outcome?.data ?? null) as HttpRequestOutcome | null;
  const status = typeof data?.status === "number" ? data.status : undefined;
  const headersJson =
    data?.headers !== undefined && data.headers !== null
      ? JSON.stringify(data.headers, null, 2)
      : null;
  const body = typeof data?.body === "string" ? data.body : null;
  const bodyTruncated = data?.bodyTruncated === true;
  const spillPath = typeof data?.spillPath === "string" ? data.spillPath : null;
  // 2xx 绿 / 4xx 橙 / 5xx 红（状态码徽标配色）
  const statusColor =
    status === undefined
      ? ""
      : status < 300
        ? "text-emerald-600 dark:text-emerald-400"
        : status < 500
          ? "text-amber-600 dark:text-amber-400"
          : "text-red-600 dark:text-red-400";
  return (
    <ToolCardShell
      {...props}
      icon={<SquareTerminal size={16} />}
      label={t("toolcards.http")}
      runningLabel={t("toolcards.sendingRequest")}
      summary={str(args, "url")}
      badges={
        status !== undefined ? (
          <span className={`shrink-0 font-mono text-ui-sm tabular-nums ${statusColor}`}>
            {status}
          </span>
        ) : undefined
      }
    >
      <p className="flex items-center gap-2 text-ui-sm">
        <span className="rounded bg-[var(--muted)] px-1.5 py-0.5 font-mono text-[var(--foreground)]">
          {method}
        </span>
        <span className="min-w-0 truncate text-[var(--muted-foreground)]" title={str(args, "url")}>
          {str(args, "url")}
        </span>
      </p>
      {headersJson !== null && (
        <details>
          <summary className="cursor-pointer text-[var(--muted-foreground)]">
            {t("toolcards.responseHeaders")}
          </summary>
          <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap rounded border border-[var(--border)] p-2 font-mono text-ui-sm text-[var(--muted-foreground)]">
            {headersJson}
          </pre>
        </details>
      )}
      {body !== null && (
        <details>
          <summary className="cursor-pointer text-[var(--muted-foreground)]">
            {t("toolcards.responseBody")}
            {bodyTruncated ? t("toolcards.responseBodyTail") : ""}
          </summary>
          <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap rounded border border-[var(--border)] p-2 font-mono text-ui-sm text-[var(--muted-foreground)]">
            {body}
          </pre>
        </details>
      )}
      {spillPath !== null && (
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          {t("toolcards.fullBodyAt", { path: spillPath })}
        </p>
      )}
    </ToolCardShell>
  );
}

/** web_search 边车 data 的形状（{ query, channel, results, count }）。 */
interface WebSearchOutcome {
  query?: unknown;
  channel?: unknown;
  results?: unknown;
  count?: unknown;
}

/** 边车 detail 的逐层尝试记录（SearchEnv 全败结构化错误 / 成功回落记录）。 */
interface SearchAttempt {
  label?: unknown;
  channel?: unknown;
  tried?: unknown;
  error?: unknown;
}

/** 来源通道徽标文案与配色（标注来源通道——本地/抓取/云）；文案经
 *  查词（模块级 t）。 */
function channelBadge(channel: string): { label: string; className: string } | undefined {
  switch (channel) {
    case "searxng":
      return {
        label: t("toolcards.channelLocal"),
        className: "bg-emerald-600/10 text-emerald-600 dark:text-emerald-400",
      };
    case "ddg":
      return {
        label: t("toolcards.channelFetch"),
        className: "bg-sky-600/10 text-sky-600 dark:text-sky-400",
      };
    case "tavily":
      return {
        label: t("toolcards.channelCloud"),
        className: "bg-violet-600/10 text-violet-600 dark:text-violet-400",
      };
    default:
      return undefined;
  }
}

/**
 * web_search 工具卡：查询词摘要 + 来源
 * 通道徽标（本地/抓取/云）+ 结果列表（标题链接 + 摘要）；失败态在
 * Shell 错误展开之上附逐层通道尝试记录（detail.attempts）。
 */
export function WebSearchCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const { toolCallId, result } = props;
  const query = str(args, "query");
  // 结果来源：边车 data（帧面 result 是模型通道压缩文本，不含结构化 data）。
  // 成功与失败终态都拉边车——web_search 的全败结构化错误（detail.attempts
  // 逐层通道原因）由执行器无条件持久化，失败态卡片要展示它（与 web_fetch
  // 失败不写边车的约定不同，web_search 的结构化错误面走 detail）。
  const settled = result !== undefined;
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const data = (outcome?.data ?? null) as WebSearchOutcome | null;
  const channel = typeof data?.channel === "string" ? data.channel : null;
  const badge = channel !== null ? channelBadge(channel) : undefined;
  const count = typeof data?.count === "number" ? data.count : undefined;
  // 结果列表（宽松取字段：title/url/snippet 任一类型不符按空串处理）
  const results = Array.isArray(data?.results)
    ? (data?.results as { title?: unknown; url?: unknown; snippet?: unknown }[])
    : [];
  // 失败态的逐层通道尝试（detail.attempts；成功时为回落记录一般仅一层）。
  // detail 是 JsonValue 联合类型：先窄化到对象再取数组
  const searchDetail = outcome?.detail;
  const detailAttempts =
    searchDetail !== null && typeof searchDetail === "object" && !Array.isArray(searchDetail)
      ? (searchDetail as { attempts?: unknown }).attempts
      : undefined;
  const attempts = Array.isArray(detailAttempts) ? (detailAttempts as SearchAttempt[]) : [];
  return (
    <ToolCardShell
      {...props}
      icon={<Search size={16} />}
      label={t("toolcards.webSearch")}
      runningLabel={t("toolcards.searching")}
      summary={query}
      badges={
        badge !== undefined ? (
          <span
            data-testid="web-search-channel"
            className={`shrink-0 rounded px-1.5 py-0.5 text-ui-sm font-medium ${badge.className}`}
          >
            {badge.label}
          </span>
        ) : undefined
      }
    >
      {count !== undefined && (
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          {count === 0 ? t("toolcards.noResults") : t("toolcards.resultCount", { count })}
          {channel !== null &&
            `（${t("toolcards.viaChannel", { channel: badge?.label ?? channel })}）`}
        </p>
      )}
      {results.length > 0 && (
        <ul className="mt-1 space-y-2" data-testid="web-search-results">
          {results.map((hit, index) => {
            const title = typeof hit.title === "string" ? hit.title : t("toolcards.untitled");
            const url = typeof hit.url === "string" ? hit.url : "";
            const snippet = typeof hit.snippet === "string" ? hit.snippet : "";
            return (
              <li key={url !== "" ? url : index} className="min-w-0">
                {url !== "" ? (
                  <a
                    href={url}
                    target="_blank"
                    rel="noreferrer noopener"
                    className="block truncate text-ui-sm font-medium text-[var(--foreground)] hover:underline"
                    title={url}
                  >
                    {title}
                  </a>
                ) : (
                  <span className="block truncate text-ui-sm font-medium text-[var(--foreground)]">
                    {title}
                  </span>
                )}
                {url !== "" && (
                  <span className="block truncate font-mono text-ui-sm text-[var(--muted-foreground)]">
                    {url}
                  </span>
                )}
                {snippet !== "" && (
                  <p className="text-ui-sm text-[var(--muted-foreground)]">{snippet}</p>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {/* 逐层通道尝试（全败结构化错误的可读化；成功回落时同理展示过程） */}
      {attempts.length > 0 && (
        <details>
          <summary className="cursor-pointer text-[var(--muted-foreground)]">
            {t("toolcards.channelAttempts")}
          </summary>
          <ul className="mt-1 space-y-1">
            {attempts.map((attempt, index) => {
              // key 用通道标识（每通道在尝试记录中至多一次）；缺标识时退化序号
              const key =
                typeof attempt.channel === "string" ? `${attempt.channel}:${index}` : String(index);
              return (
                <li key={key} className="text-ui-sm text-[var(--muted-foreground)]">
                  <span className="text-[var(--foreground)]">
                    {typeof attempt.label === "string" ? attempt.label : String(attempt.channel)}
                  </span>
                  {attempt.tried === true
                    ? typeof attempt.error === "string"
                      ? t("toolcards.attemptError", { error: attempt.error })
                      : t("toolcards.attemptOk")
                    : t("toolcards.attemptSkipped")}
                </li>
              );
            })}
          </ul>
        </details>
      )}
    </ToolCardShell>
  );
}

// —— 小工具包四卡（batch_read/glob/calculate/render_html）——

/** batch_read 的边车 data.files 元素形状（宽松取字段用）。 */
interface BatchReadFileEntry {
  path?: unknown;
  ok?: unknown;
  truncated?: unknown;
  content?: unknown;
  nextOffset?: unknown;
  error?: { message?: unknown } | null;
}

/**
 * batch_read 工具卡：文件数徽标 + 逐文件成功/失败/截断列表；每个文件的
 * 内容默认折叠（details，限高滚动），截断文件标注续读 nextOffset。
 */
export function BatchReadCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const { toolCallId, result } = props;
  const paths = Array.isArray(args.paths)
    ? (args.paths as unknown[]).filter((p): p is string => typeof p === "string")
    : [];
  // 逐文件状态来自边车 data.files（帧面 result 是模型通道压缩文本）。
  const settled = result !== undefined;
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const data = outcome?.data;
  const rawFiles =
    data !== null && typeof data === "object" && !Array.isArray(data) ? data.files : undefined;
  const files = Array.isArray(rawFiles) ? (rawFiles as BatchReadFileEntry[]) : [];
  const truncatedCount = files.filter((f) => f.truncated === true).length;
  return (
    <ToolCardShell
      {...props}
      icon={<FileStack size={16} />}
      label={t("toolcards.batchRead")}
      runningLabel={t("toolcards.batchReading")}
      summary={paths[0] ?? ""}
      summaryNode={
        paths.length > 0 ? (
          <span className="min-w-0 truncate text-[var(--foreground-subtlest)]">
            {paths.length === 1
              ? paths[0]
              : t("toolcards.moreFiles", { first: paths[0], count: paths.length })}
          </span>
        ) : undefined
      }
      badges={
        paths.length > 0 ? (
          <span
            data-testid="batch-read-count"
            className="shrink-0 rounded bg-[var(--muted)] px-1.5 py-0.5 text-ui-sm font-medium text-[var(--muted-foreground)]"
          >
            {paths.length}
          </span>
        ) : undefined
      }
    >
      {files.length > 0 && (
        <ul className="space-y-1" data-testid="batch-read-files">
          {files.map((file, index) => {
            const path = typeof file.path === "string" ? file.path : `#${index}`;
            const failed = file.ok === false;
            const message =
              file.error !== null &&
              typeof file.error === "object" &&
              typeof file.error?.message === "string"
                ? file.error.message
                : "";
            return (
              // biome-ignore lint/suspicious/noArrayIndexKey: 重复路径是合法输入（同批可重复读同一文件），序号防 key 撞
              <li key={`${path}:${index}`} className="min-w-0">
                <span className={failed ? "text-[var(--destructive)]" : "text-[var(--foreground)]"}>
                  {failed ? "✗" : "✓"}
                </span>{" "}
                <span className="font-mono text-ui-sm text-[var(--foreground)]">{path}</span>
                {file.truncated === true && (
                  <span className="ml-1 text-ui-sm text-[var(--warning)]">
                    {t("toolcards.truncated")}
                    {typeof file.nextOffset === "number"
                      ? t("toolcards.resumeOffset", { offset: file.nextOffset })
                      : ""}
                  </span>
                )}
                {failed && message !== "" && (
                  <span className="ml-1 text-ui-sm text-[var(--destructive)]">{message}</span>
                )}
                {!failed && typeof file.content === "string" && file.content !== "" && (
                  <details className="mt-0.5">
                    <summary className="cursor-pointer text-ui-sm text-[var(--muted-foreground)]">
                      {t("toolcards.content")}
                    </summary>
                    <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap font-mono text-ui-sm text-[var(--muted-foreground)]">
                      {file.content}
                    </pre>
                  </details>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {truncatedCount > 0 && (
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          {t("toolcards.truncatedFilesNote", { count: truncatedCount })}
        </p>
      )}
    </ToolCardShell>
  );
}

/** glob 的边车 data.paths 元素形状。 */
interface GlobPathEntry {
  path?: unknown;
  kind?: unknown;
}

/** glob 工具卡：模式 + 起点摘要；详情 = 命中计数 + 路径列表（上限展示 100
 *  条，对齐 grep 卡的结果量级）。 */
export function GlobCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const { toolCallId, result } = props;
  const pattern = str(args, "pattern");
  const scope = str(args, "path") || t("toolcards.projectRoot");
  // 命中清单来自边车 data（帧面 result 是压缩文本）。
  const settled = result !== undefined;
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const data = outcome?.data as { count?: unknown; truncated?: unknown; paths?: unknown } | null;
  const count = typeof data?.count === "number" ? data.count : undefined;
  const truncated = data?.truncated === true;
  const paths = Array.isArray(data?.paths) ? (data?.paths as GlobPathEntry[]) : [];
  const shown = paths.slice(0, 100);
  return (
    <ToolCardShell
      {...props}
      icon={<FolderSearch size={16} />}
      label={t("toolcards.glob")}
      runningLabel={t("toolcards.globbing")}
      summary={`${pattern} @ ${scope}`}
    >
      {count !== undefined && (
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          {count === 0 ? t("toolcards.noMatch") : t("toolcards.matchCount", { count })}
          {truncated && t("toolcards.truncatedAtLimit")}
        </p>
      )}
      {shown.length > 0 && (
        <ul
          className="max-h-60 space-y-0.5 overflow-auto font-mono text-ui-sm"
          data-testid="glob-paths"
        >
          {shown.map((entry) => (
            <li
              key={typeof entry.path === "string" ? entry.path : ""}
              className="truncate text-[var(--muted-foreground)]"
            >
              {typeof entry.path === "string" ? entry.path : ""}
              {entry.kind === "dir" && "/"}
            </li>
          ))}
          {paths.length > shown.length && (
            <li className="text-[var(--muted-foreground)]">
              {t("toolcards.moreInDetail", { count: paths.length - shown.length })}
            </li>
          )}
        </ul>
      )}
    </ToolCardShell>
  );
}

/** calculate 工具卡：表达式摘要（monospace）+ 结果 monospace 单行（保真
 *  显示取自边车 data.formatted——与 Python repr 同口径的最短往返）。 */
export function CalculateCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const { toolCallId, result } = props;
  const expression = str(args, "expression");
  const settled = result !== undefined;
  const outcome = useToolOutcomeDetail(toolCallId, settled);
  const data = outcome?.data as { formatted?: unknown } | null;
  const formatted = typeof data?.formatted === "string" ? data.formatted : null;
  return (
    <ToolCardShell
      {...props}
      icon={<Calculator size={16} />}
      label={t("toolcards.calculate")}
      runningLabel={t("toolcards.calculating")}
      summary={expression}
    >
      {formatted !== null && (
        <p className="font-mono text-ui-sm text-[var(--foreground)]" data-testid="calculate-result">
          = {formatted}
        </p>
      )}
    </ToolCardShell>
  );
}

/** render_html 工具卡：字符数徽标 + 默认折叠的沙箱预览（手动展开才挂
 *  iframe——防闪烁、防未经查看的脚本执行）。 */
export function RenderHtmlCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const t = useT();
  const args = (props.args ?? {}) as CardArgs;
  const html = str(args, "html");
  const [previewOpen, setPreviewOpen] = useState(false);
  const charCount = html.length;
  return (
    <ToolCardShell
      {...props}
      icon={<Eye size={16} />}
      label={t("toolcards.htmlPreview")}
      runningLabel={t("toolcards.preparingHtmlPreview")}
      summary={t("toolcards.sandboxPreviewCard")}
      badges={
        charCount > 0 ? (
          <span
            data-testid="render-html-chars"
            className="shrink-0 rounded bg-[var(--muted)] px-1.5 py-0.5 text-ui-sm font-medium text-[var(--muted-foreground)]"
          >
            {t("toolcards.charCount", { count: charCount })}
          </span>
        ) : undefined
      }
    >
      {charCount > 0 && (
        <div>
          <button
            type="button"
            data-testid="render-html-toggle"
            onClick={() => setPreviewOpen((v) => !v)}
            className="cursor-pointer text-ui-sm text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
            aria-expanded={previewOpen}
          >
            {previewOpen ? t("toolcards.collapsePreview") : t("toolcards.expandPreview")}
          </button>
          {/* 展开才挂载 iframe：未经查看不执行预览页脚本 */}
          {previewOpen && (
            <div className="mt-2">
              <HtmlPreview html={html} />
            </div>
          )}
        </div>
      )}
      <p className="text-[var(--muted-foreground)]">{t("toolcards.sandboxNote")}</p>
    </ToolCardShell>
  );
}
