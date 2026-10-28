/**
 * 聊天主界面（Thread.tsx）：用 assistant-ui 的原语组件手工装配的完整聊天窗。
 *
 * 组成：消息列表（用户/助手消息、折叠分组的过程区）+ 输入区（输入框、
 * 权限档指示、@ 补全菜单浮层）+ ghost 提示词（输入框上方的灰色续写建议）
 * + 计数条（输出速率/本轮用量/会话用量）。折叠分组指把一次运行的
 * 思考过程与工具卡收起为一条摘要行，点击可重新展开查看完整过程。
 * 这里只管单个会话内的呈现，多会话的 Tab 装配在 SessionWorkspace.tsx。
 */
import {
  ActionBarPrimitive,
  AuiIf,
  BranchPickerPrimitive,
  ComposerPrimitive,
  ErrorPrimitive,
  MessagePrimitive,
  QueueItemPrimitive,
  ThreadPrimitive,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import { useEffect, useRef, useState } from "react";
import { Streamdown } from "streamdown";
import { useComposerKeys } from "@/chat/composer-keys";
import { TIER_LABELS, usePermissionTierStore } from "@/chat/permission-tier";
import { useSessionMetrics } from "@/chat/run-metrics";
import { useSessionModelStore } from "@/chat/session-model";
import { useRunSummaries, useSessionScope } from "@/chat/session-scope";
import { getConfig } from "@/ipc/client";
import type { PermissionTier } from "@/ipc/types";
import { matchGhost, useAskStore } from "@/stores/ask";
import { useSessionsStore } from "@/stores/sessions";

/** 消息区（FR-13-15 滚动跟随：Viewport autoScroll 内建「跟随最新 + 上滚
 *  暂停」；不在底部时显示回底按钮，17 号票）。内容列限宽居中（参考样式
 *  A1：工作流风格平铺，不占满整屏宽）。 */
export function ChatThread() {
  return (
    // 透明背景：透出承载它的大圆角卡（bg-card），自身不再铺底色
    <div className="flex h-full min-h-0 flex-1 flex-col text-[var(--foreground)]">
      <ThreadPrimitive.Root className="relative flex min-h-0 flex-1 flex-col">
        <ThreadPrimitive.Viewport className="flex flex-1 flex-col overflow-y-auto p-4">
          <AuiIf condition={(s) => s.thread.isEmpty}>
            <ThreadEmptyState />
          </AuiIf>
          {/* 内容列限宽居中（约 62%，A1）：消息与过程行统一宽度 */}
          <div className="mx-auto w-full max-w-4xl">
            <ThreadPrimitive.Messages>
              {({ message }) =>
                // system = runMarker 等存储标记节点（custom 载荷保真，§6.2 跳过内容）——不渲染
                message.role === "system" ? null : message.role === "user" ? (
                  <UserMessage />
                ) : (
                  <AssistantMessage />
                )
              }
            </ThreadPrimitive.Messages>
            {/* 运行中末尾 spinner（A3） */}
            <AuiIf condition={(s) => s.thread.isRunning}>
              <div
                data-testid="thread-spinner"
                className="flex items-center gap-2 px-1 py-1 text-secondary text-[var(--muted-foreground)]"
              >
                <LoaderIcon />
                处理中…
              </div>
            </AuiIf>
          </div>
        </ThreadPrimitive.Viewport>
        {/* composer 固定底区：在滚动容器（Viewport）之外，不随内容滚动、
            内容也不会钻入其背后（sticky 方案的两个老问题一并消除） */}
        <div className="shrink-0 px-4 pb-3">
          <div className="relative mx-auto w-full max-w-4xl">
            {/* 回底按钮（FR-13-15）：已在底部时原语输出 disabled，用
                disabled:hidden 隐藏；挂 composer 容器上方自适应其高度 */}
            <ThreadPrimitive.ScrollToBottom className="cw-pop-in absolute bottom-full left-1/2 z-20 mb-2 flex size-8 -translate-x-1/2 items-center justify-center rounded-full border border-[var(--border)] bg-[var(--card)] text-[var(--muted-foreground)] shadow hover:text-[var(--foreground)] disabled:hidden">
              <ArrowDownIcon />
            </ThreadPrimitive.ScrollToBottom>
            <ComposerBar />
            <ComposerGhost />
          </div>
        </div>
      </ThreadPrimitive.Root>
    </div>
  );
}

/** 时段问候（B1 空状态，六档边界 5/9/12/14/18/23 与 ZCode 同口径）。 */
function greeting(): string {
  const hour = new Date().getHours();
  if (hour < 5) return "夜深了";
  if (hour < 9) return "早上好";
  if (hour < 12) return "上午好";
  if (hour < 14) return "中午好";
  if (hour < 18) return "下午好";
  if (hour < 23) return "晚上好";
  return "夜深了";
}

/** 空状态建议词（编码代理场景，点击整句填入 composer 不发送）。 */
const EMPTY_STATE_SUGGESTIONS = [
  "梳理当前项目的目录结构与关键模块",
  "审查最近的改动并给出改进建议",
  "为某个模块补齐单元测试",
  "解释一段看不懂的代码",
];

/** 新会话首屏（B1，ZCode 同款）：时段问候 + 建议提示词 chips；
 *  chips 逐个下落入场（cw-rise，delay 内联 ×65ms，reduced-motion 关闭）。 */
function ThreadEmptyState() {
  const aui = useAui();
  return (
    <div className="mx-auto flex w-full max-w-4xl flex-1 flex-col items-center justify-center gap-5 py-10">
      <div className="flex flex-col items-center gap-1.5">
        <p className="text-hero font-medium text-[var(--foreground)]">{greeting()}</p>
        <p className="text-body text-[var(--muted-foreground)]">开始新对话，或从下面的建议开始</p>
      </div>
      <div className="flex flex-wrap items-center justify-center gap-2">
        {EMPTY_STATE_SUGGESTIONS.map((text, index) => (
          <button
            key={text}
            type="button"
            className="cw-rise h-8 rounded-lg border border-[var(--border)] px-3 text-secondary text-[var(--muted-foreground)] transition-colors hover:border-[var(--ring)] hover:text-[var(--foreground)]"
            style={{ animationDelay: `${index * 65}ms` }}
            onClick={() => aui.composer.setText(text, { origin: "user" })}
          >
            {text}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Composer 区（§10.6 / FR-13-02，17 号票）：输入框 + 权限档指示 + @ 菜单浮层。
 *  Enter/Shift+Enter/IME 由 ComposerPrimitive.Input 原生承担。 */
function ComposerBar() {
  const { onKeyDown, onCursor, atMenu, pickActive, hoverIndex } = useComposerKeys();
  return (
    <div className="relative">
      {/* @ 路径补全浮层（search_workspace_paths 驱动；Tab/Enter 选中，Esc 关闭） */}
      {atMenu !== null && atMenu.items.length > 0 && (
        <ul
          data-testid="at-menu"
          className="cw-pop-in absolute bottom-full left-0 z-20 mb-1 max-h-56 w-72 overflow-y-auto rounded-lg border border-[var(--border)] bg-[var(--card)] p-1 text-secondary shadow"
        >
          {atMenu.items.map((hit, index) => (
            <li key={hit.path}>
              <button
                type="button"
                className={`flex w-full items-center gap-2 rounded-md px-2 py-1 text-left ${
                  index === atMenu.activeIndex
                    ? "bg-[var(--accent)] text-[var(--foreground)]"
                    : "text-[var(--muted-foreground)]"
                }`}
                onClick={pickActive}
                onMouseEnter={() => hoverIndex(index)}
              >
                {hit.kind === "dir" ? <FolderIconSmall /> : <FileIconSmall />}
                <span className="truncate">{hit.path}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {/* 消息队列条（FR-01-07 排队面）：运行中发送的消息先排队展示，
          当前 run 结束后自动依次发送；× 可移除未发送项 */}
      <ComposerPrimitive.Queue>
        {() => (
          <div
            data-testid="queue-item"
            className="mb-1 flex items-center gap-2 rounded-xl border border-[var(--border)] bg-[var(--muted)]/60 px-2.5 py-1 text-secondary text-[var(--muted-foreground)]"
          >
            <span className="shrink-0 rounded-full border border-[var(--border)] px-1.5 text-meta leading-4">
              排队中
            </span>
            <QueueItemPrimitive.Text className="min-w-0 flex-1 truncate" />
            <QueueItemPrimitive.Remove
              aria-label="移除排队消息"
              className="shrink-0 rounded px-1 hover:text-[var(--destructive)]"
            >
              ×
            </QueueItemPrimitive.Remove>
          </div>
        )}
      </ComposerPrimitive.Queue>
      {/* 输入壳（ZCode 同款结构）：p-3 卡，输入框与工具栏/状态栏纵向 gap-2；
          hover 边框提亮一档（--input），focus-within 蓝边呼应聚焦令牌；
          底色用 --composer-bg（暗色 #272728 亮于中部卡，亮色同卡白） */}
      <ComposerPrimitive.Root className="flex flex-col gap-2 rounded-2xl border border-[var(--border)] bg-[var(--composer-bg)] p-3 shadow-sm transition-colors hover:border-[var(--input)] focus-within:border-[var(--cw-focus)]">
        <ComposerPrimitive.Input
          minRows={2}
          placeholder="发一条消息，运行中的消息将自动排队…"
          className="max-h-40 w-full resize-none bg-transparent py-0.5 text-body leading-5 placeholder:text-[var(--muted-foreground)] focus:outline-none"
          onKeyDown={onKeyDown}
          onKeyUp={(e) => onCursor(e.currentTarget.selectionStart ?? 0)}
          onClick={(e) => onCursor(e.currentTarget.selectionStart ?? 0)}
        />
        {/* 工具栏：左簇（附件 + 权限档）右簇（模型 + 发送/停止），簇间纯空隙 */}
        <div className="flex items-center gap-2">
          <button
            type="button"
            aria-label="附件（M2 提供）"
            disabled
            title="附件功能随 M2 提供"
            className="flex size-7 shrink-0 items-center justify-center rounded-lg text-[var(--muted-foreground)] hover:bg-[var(--accent)] disabled:opacity-40"
          >
            <PlusIcon />
          </button>
          <TierMenu />
          <div className="ml-auto flex items-center gap-1.5">
            <ModelPicker />
            {/* 发送恒显：运行中点击即入队（队列条展示，run 结束自动续发）——
                故不与停止同位替换（ZCode 无排队面，两键并列是本应用的差异） */}
            <ComposerPrimitive.Send
              aria-label="发送"
              className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-[var(--primary)] text-[var(--primary-foreground)] hover:opacity-90"
            >
              <ArrowUpIcon />
            </ComposerPrimitive.Send>
            <AuiIf condition={(s) => s.thread.isRunning}>
              <ComposerPrimitive.Cancel
                aria-label="停止"
                title="停止（Esc）"
                className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-[var(--muted)] text-[var(--foreground)] hover:opacity-90"
              >
                <SquareIcon />
              </ComposerPrimitive.Cancel>
            </AuiIf>
          </div>
        </div>
        {/* 状态栏（A5）——思考计时 + 速率/本轮/会话三数 + 工具次数（ZCode 无此层，保留） */}
        <ComposerStatusBar />
      </ComposerPrimitive.Root>
    </div>
  );
}

/** 权限档下拉（FR-13-02）：盾牌图标 + 当前档名，点开四档清单直接选定
 *  （Shift+Tab 循环仍可用，§10.6 内存态——本次会话有效）。 */
function TierMenu() {
  const sessionId = useSessionScope();
  const tier = usePermissionTierStore((s) => s.bySession[sessionId] ?? "autoEdit");
  const setTier = usePermissionTierStore((s) => s.setTier);
  const [open, setOpen] = useState(false);
  const TIERS: PermissionTier[] = ["plan", "confirmEach", "autoEdit", "fullAccess"];
  return (
    <div className="relative shrink-0">
      {open && (
        <ul
          data-testid="tier-menu"
          className="cw-pop-in absolute bottom-full left-0 z-20 mb-1 w-40 rounded-lg border border-[var(--border)] bg-[var(--card)] p-1 text-secondary shadow"
        >
          {TIERS.map((t) => (
            <li key={t}>
              <button
                type="button"
                className={`flex w-full items-center gap-2 rounded-md px-2 py-1 text-left ${
                  t === tier
                    ? "bg-[var(--accent)] text-[var(--foreground)]"
                    : "text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
                }`}
                onClick={() => {
                  setTier(sessionId, t);
                  setOpen(false);
                }}
              >
                <span aria-hidden>{t === tier ? "●" : "○"}</span>
                {TIER_LABELS[t]}
              </button>
            </li>
          ))}
        </ul>
      )}
      <button
        type="button"
        data-testid="tier-badge"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex h-7 items-center gap-1 rounded-lg px-2 text-secondary text-[var(--muted-foreground)] hover:bg-[var(--accent)]"
        title="权限档（本次会话有效，Shift+Tab 循环切换）"
      >
        <ShieldIcon />
        {TIER_LABELS[tier]}
        <ChevronDownIcon />
      </button>
    </div>
  );
}

/**
 * 模型选择下拉（FR-13-05 Ctrl+L 的 M1 简化形态）：列出配置里全部供应商
 * 的模型；选中即本会话覆盖（发送时经 RunConfig.modelId 生效，§11.1），
 * 「跟随默认」回到全局 activeModelId。清单在每次打开时拉取（设置页
 * 改动即时反映）。
 */
function ModelPicker() {
  const sessionId = useSessionScope();
  const selected = useSessionModelStore((s) => s.bySession[sessionId]);
  const setModel = useSessionModelStore((s) => s.setModel);
  const [open, setOpen] = useState(false);
  // 全部模型（按供应商分组）与全局默认模型 id：打开时拉取
  const [groups, setGroups] = useState<
    { provider: string; models: { id: string; name: string }[] }[]
  >([]);
  const [activeId, setActiveId] = useState<string | undefined>(undefined);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void getConfig()
      .then((config) => {
        if (cancelled) return;
        setActiveId(config.activeModelId);
        setGroups(
          config.providers.map((p) => ({
            provider: p.name,
            models: (p.models ?? []).map((m) => ({ id: m.id, name: m.model })),
          })),
        );
      })
      .catch(() => {
        // IPC 异常（纯浏览器等）：清单留空，仅显示当前选择
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  // 当前显示名：会话覆盖优先，否则全局默认的模型名，再否则占位提示
  const currentName =
    groups.flatMap((g) => g.models).find((m) => m.id === (selected ?? activeId))?.name ??
    selected ??
    "默认模型";

  return (
    <div className="relative shrink-0">
      {open && (
        <ul
          data-testid="model-menu"
          className="cw-pop-in absolute bottom-full right-0 z-20 mb-1 max-h-64 w-64 overflow-y-auto rounded-lg border border-[var(--border)] bg-[var(--card)] p-1 text-secondary shadow"
        >
          <li>
            <button
              type="button"
              className={`flex w-full rounded-md px-2 py-1 text-left ${
                selected === undefined
                  ? "bg-[var(--accent)] text-[var(--foreground)]"
                  : "text-[var(--muted-foreground)]"
              }`}
              onClick={() => {
                setModel(sessionId, undefined);
                setOpen(false);
              }}
            >
              跟随默认
            </button>
          </li>
          {groups.map((group) => (
            <li key={group.provider} className="mt-1">
              <div className="px-2 py-0.5 text-meta text-[var(--cw-dim)]">{group.provider}</div>
              <ul>
                {group.models.map((model) => (
                  <li key={model.id}>
                    <button
                      type="button"
                      className={`flex w-full rounded-md px-2 py-1 text-left ${
                        model.id === selected
                          ? "bg-[var(--accent)] text-[var(--foreground)]"
                          : "text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
                      }`}
                      onClick={() => {
                        setModel(sessionId, model.id);
                        setOpen(false);
                      }}
                    >
                      <span className="truncate">{model.name}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
      )}
      <button
        type="button"
        data-testid="model-picker"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex h-7 max-w-40 min-w-0 items-center gap-1 rounded-lg px-2 text-secondary text-[var(--muted-foreground)] hover:bg-[var(--accent)]"
        title="选择本会话使用的模型"
      >
        <span className="truncate">{currentName}</span>
        <ChevronDownIcon />
      </button>
    </div>
  );
}

/** 用户消息：右对齐浅底块（工作流风格 A1：弱化为 muted 底，不用主色）。 */
function UserMessage() {
  return (
    <MessagePrimitive.Root className="flex flex-col items-end py-2">
      {/* 气泡右上 2px 折角 + 1px 边框（ZCode 同款），与平铺的助手消息区分 */}
      <div className="max-w-[85%] rounded-xl rounded-tr-xs border border-[var(--border)] bg-[var(--muted)] px-4 py-2.5 text-body text-[var(--foreground)]">
        <MessagePrimitive.Parts>
          {({ part }) =>
            part.type === "text" && part.text.length > 0 ? (
              <Streamdown className="cw-md">{part.text}</Streamdown>
            ) : null
          }
        </MessagePrimitive.Parts>
      </div>
      <BranchBar editOnly />
    </MessagePrimitive.Root>
  );
}

/**
 * 折叠分组状态机（§10.4 / FR-01-10，17 号票）：
 *
 * - 运行中（thread.isRunning 且本消息是链上最后一条）：过程 parts（思考流/
 *   工具卡）全量实时展开——FR-01-10「不预先折叠」；
 * - run 结束后 3 秒收起为摘要行（自动折叠开关可关，ui-state）；
 * - 历史消息（挂载即非运行中）：默认收起；
 * - 点击摘要行随时展开/收起回看完整过程（过程数据由边车补全，FR-04-07）；
 * - 最终答复（text parts）始终保留在折叠区外常显。
 */
function useRunCollapse(processCount: number): {
  collapsed: boolean;
  toggle: () => void;
} {
  const isRunning = useAuiState((s) => s.thread.isRunning);
  const isLast = useAuiState((s) => s.message.isLast);
  const autoCollapse = useSessionsStore((s) => s.collapseRuns);
  const active = isRunning && isLast;
  const [expanded, setExpanded] = useState(() => active);
  const prevActive = useRef(active);
  useEffect(() => {
    if (active && !prevActive.current) {
      // 新 run 开始（含从本消息再次发起）：过程全部展开
      setExpanded(true);
    } else if (!active && prevActive.current && autoCollapse && processCount > 0) {
      // run 刚结束：留 3 秒看完最后输出再收起（§10.4）
      const timer = setTimeout(() => setExpanded(false), 3000);
      prevActive.current = active;
      return () => clearTimeout(timer);
    }
    prevActive.current = active;
  }, [active, autoCollapse, processCount]);
  return { collapsed: !expanded, toggle: () => setExpanded((v) => !v) };
}

/** 时长格式化（摘要行）：秒级一位小数，分钟级取整。 */
function formatDuration(ms: number): string {
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m${Math.round(seconds - minutes * 60)}s`;
}

/**
 * 助手消息：正文 text 常显；思考过程与工具卡（过程 parts）按 useRunCollapse
 * 的折叠状态决定显示；收起时展示「N 次工具调用 · 时长」摘要行，可点击展开。
 * 时长与中断态取自 run 收口摘要表（SessionTab 注入，见 tree-persistence）。
 */
function AssistantMessage() {
  const parts = useAuiState((s) => s.message.parts);
  const runId = useAuiState((s) => (s.message.metadata as { runId?: string } | undefined)?.runId);
  const summaries = useRunSummaries();
  const summary = runId !== undefined ? summaries.get(runId) : undefined;
  // 过程 parts = 思考流 + 工具卡（text 恒常显）
  const processParts = parts.filter((p) => p.type === "reasoning" || p.type === "tool-call");
  const toolCount = parts.filter((p) => p.type === "tool-call").length;
  const { collapsed, toggle } = useRunCollapse(processParts.length);
  const sessionId = useSessionScope();
  const lastRun = useSessionMetrics(sessionId).lastRun;
  // 时长优先取 marker 收口摘要；运行刚结束的 run 由 run-metrics 兜底（此时 marker 尚未收养）
  const durationMs =
    summary?.durationMs ??
    (lastRun !== null && lastRun.runId === runId ? lastRun.durationMs : undefined);
  // 崩溃中断态（15 号票 shortcut 渲染层落地，17 号票）：marker 无 endedAt
  const interrupted = summary !== undefined && summary.endedAt === undefined;

  return (
    <MessagePrimitive.Root className="flex w-full flex-col items-start py-1">
      {/* 摘要行（§10.4，参考样式 A1 单行风）：过程存在且收起时显示；点击展开/收起 */}
      {collapsed && processParts.length > 0 && (
        <button
          type="button"
          data-testid="run-summary"
          onClick={toggle}
          className="flex items-center gap-2 rounded px-1 py-0.5 text-secondary text-[var(--muted-foreground)] hover:bg-[var(--accent)]/60"
        >
          <WrenchIcon />
          <span>
            {summary?.toolCount ?? toolCount} 次工具调用
            {durationMs !== undefined ? ` · ${formatDuration(durationMs)}` : ""}
          </span>
          {interrupted && <span className="text-[var(--destructive)]">已中断</span>}
          <span aria-hidden className="text-[var(--cw-dim)]">
            ▸
          </span>
        </button>
      )}
      {/* 工作流风格 A1：助手内容平铺（无气泡底色），与过程行同列 */}
      <div className="w-full text-body">
        {/* 内联 render props（勿抽组件）：S1 实证 Parts 对 tool-call part 渲染
            null 时才 fallback 到注册的镜像工具卡；抽组件会让 fallback 判定失效。
            折叠态过程 part 返回隐藏空元素（非 null）以抑制 fallback 卡。 */}
        <MessagePrimitive.Parts>
          {({ part }) => {
            if (part.type === "text") {
              return part.text.length === 0 ? (
                <span hidden />
              ) : (
                // 流式 Markdown（FR-01-03）：Streamdown 增量解析全量累计快照
                <Streamdown className="cw-md">{part.text}</Streamdown>
              );
            }
            if (collapsed) return <span hidden />;
            if (part.type === "reasoning") {
              // 思考过程单行摘要（A2）：与工具行同风格，点击展开全文
              return (
                <details className="text-secondary text-[var(--muted-foreground)]">
                  <summary className="flex w-fit cursor-default select-none items-center gap-2 rounded px-1 py-0.5 hover:bg-[var(--accent)]/60">
                    <LightbulbIcon />
                    <span>思考过程</span>
                    <span aria-hidden className="text-[var(--cw-dim)]">
                      ▸
                    </span>
                  </summary>
                  <p className="text-secondary my-1 ml-2 max-h-60 overflow-y-auto border-l border-[var(--border)] pl-3.5 whitespace-pre-wrap text-[var(--muted-foreground)]">
                    {part.text}
                  </p>
                </details>
              );
            }
            return null;
          }}
        </MessagePrimitive.Parts>
        <MessagePrimitive.Error>
          <ErrorPrimitive.Root className="mt-1 flex w-full items-start gap-2 rounded-lg border border-[var(--destructive)]/30 bg-[var(--destructive)]/5 px-3 py-2.5">
            <AlertIcon />
            <div className="min-w-0 text-secondary">
              <div className="font-medium text-[var(--destructive)]">运行出错</div>
              <ErrorPrimitive.Message className="mt-0.5 break-words text-[var(--muted-foreground)]" />
            </div>
          </ErrorPrimitive.Root>
        </MessagePrimitive.Error>
      </div>
      <BranchBar editOnly={false} />
    </MessagePrimitive.Root>
  );
}

/** 告警图标（lucide circle-alert 造型，错误块前缀）。 */
function AlertIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="mt-0.5 shrink-0 text-[var(--destructive)]"
    >
      <circle cx="12" cy="12" r="10" />
      <path d="M12 8v4" />
      <path d="M12 16h.01" />
    </svg>
  );
}

/** 盾牌图标（lucide shield 造型，权限档指示前缀，16px 与 ZCode size-4 同档）。 */
function ShieldIcon() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="inline-block shrink-0 align-[-1px]"
    >
      <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />
    </svg>
  );
}

/** 灯泡图标（lucide lightbulb 造型，思考过程/思考中前缀）。 */
function LightbulbIcon() {
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
      className="inline-block shrink-0 align-[-1px]"
    >
      <path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5" />
      <path d="M9 18h6" />
      <path d="M10 22h4" />
    </svg>
  );
}

/** 扳手图标（lucide wrench 造型，run 摘要行前缀）。 */
function WrenchIcon() {
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
      <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
    </svg>
  );
}

/** 下降箭头图标（lucide arrow-down 造型，回底按钮）。 */
function ArrowDownIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="shrink-0"
    >
      <path d="M12 5v14" />
      <path d="m19 12-7 7-7-7" />
    </svg>
  );
}

/** 上升箭头图标（lucide arrow-up 造型，发送钮，16px 与 ZCode size-4 同档）。 */
function ArrowUpIcon() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="inline-block shrink-0 align-[-2px]"
    >
      <path d="m5 12 7-7 7 7" />
      <path d="M12 19V5" />
    </svg>
  );
}

/** 加号图标（lucide plus 造型，附件占位钮，16px）。 */
function PlusIcon() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="shrink-0"
    >
      <path d="M5 12h14" />
      <path d="M12 5v14" />
    </svg>
  );
}

/** 方块图标（lucide square 实心造型，停止钮；ZCode 同款 fill-current）。 */
function SquareIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden
      className="shrink-0"
    >
      <rect x="6" y="6" width="12" height="12" rx="2" />
    </svg>
  );
}

/** 下拉箭头图标（lucide chevron-down 造型，权限档/模型触发器尾部，14px）。 */
function ChevronDownIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="shrink-0 opacity-60"
    >
      <path d="m6 9 6 6 6-6" />
    </svg>
  );
}

/** 加载圈图标（lucide loader-circle 造型，处理中旋转指示）。 */
function LoaderIcon() {
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
      className="inline-block shrink-0 animate-spin"
    >
      <path d="M21 12a9 9 0 1 1-6.219-8.56" />
    </svg>
  );
}

/** 小号文件夹图标（lucide folder 造型，@ 菜单目录行前缀）。 */
function FolderIconSmall() {
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
      <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
    </svg>
  );
}

/** 小号文件图标（lucide file 造型，@ 菜单文件行前缀）。 */
function FileIconSmall() {
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
      <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
      <path d="M14 2v4a2 2 0 0 0 2 2h4" />
    </svg>
  );
}

/** 分支操作条（§6.3 树操作 API 的 UI 接地，15 号票）：上一/下一分支 +
 * 重生成（assistant）/ 编辑重发（user）/ 复制。官方原语自带禁用态。 */
function BranchBar({ editOnly }: { editOnly: boolean }) {
  const btn =
    "rounded border border-transparent px-1.5 py-0.5 text-meta text-[var(--muted-foreground)] hover:border-[var(--border)] hover:text-[var(--foreground)] disabled:opacity-30";
  return (
    <div className="mt-0.5 flex items-center gap-0.5" data-testid="branch-bar">
      <BranchPickerPrimitive.Root className="flex items-center gap-0.5">
        <BranchPickerPrimitive.Previous className={btn} aria-label="上一个分支">
          ←
        </BranchPickerPrimitive.Previous>
        <span className="text-meta text-[var(--cw-dim)]">
          <BranchPickerPrimitive.Count />
        </span>
        <BranchPickerPrimitive.Next className={btn} aria-label="下一个分支">
          →
        </BranchPickerPrimitive.Next>
      </BranchPickerPrimitive.Root>
      <ActionBarPrimitive.Root className="flex items-center gap-0.5">
        {editOnly ? (
          <ActionBarPrimitive.Edit className={btn} aria-label="编辑重发">
            编辑
          </ActionBarPrimitive.Edit>
        ) : (
          <>
            <ActionBarPrimitive.Reload className={btn} aria-label="重新生成">
              重新生成
            </ActionBarPrimitive.Reload>
            <ActionBarPrimitive.Copy className={btn} aria-label="复制">
              复制
            </ActionBarPrimitive.Copy>
          </>
        )}
      </ActionBarPrimitive.Root>
    </div>
  );
}

/** Composer 状态栏（A5，§10.5 三数 + 思考计时 + 工具次数）：常驻输入卡片
 *  底部一行；思考计时由 reasoning/text 帧驱动（run-metrics thinkingSince）。 */
function ComposerStatusBar() {
  const sessionId = useSessionScope();
  const { rate, turnUsage, sessionUsage, lastRun, thinkingSince } = useSessionMetrics(sessionId);
  // 思考中秒表：thinkingSince 有值即每秒重算（组件级刷新，无需全局定时器）
  const [, forceTick] = useState(0);
  useEffect(() => {
    if (thinkingSince === null) return;
    const timer = setInterval(() => forceTick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [thinkingSince]);
  const tokens = (u: { input: number; output: number }) => `${u.input + u.output}`;
  const thinkingSeconds =
    thinkingSince !== null ? Math.max(0, Math.round((Date.now() - thinkingSince) / 1000)) : 0;
  return (
    <div
      data-testid="composer-status"
      className="flex items-center gap-3 text-meta tabular-nums text-[var(--cw-dim)]"
    >
      {thinkingSince !== null && (
        <span data-testid="metric-thinking" className="text-[var(--muted-foreground)]">
          <LightbulbIcon /> 思考中 {thinkingSeconds}s
        </span>
      )}
      <span data-testid="metric-rate">{rate === null ? "—" : `${rate.toFixed(0)} tok/s`}</span>
      <span data-testid="metric-turn">本轮 {tokens(turnUsage)} tok</span>
      <span data-testid="metric-session">会话 {tokens(sessionUsage)} tok</span>
      {lastRun && (
        <span data-testid="metric-run">
          {lastRun.reason === "done"
            ? `完成 · ${(lastRun.durationMs / 1000).toFixed(1)}s · ${lastRun.toolCount} 次工具`
            : lastRun.reason === "cancelled"
              ? "已取消"
              : "出错"}
        </span>
      )}
    </div>
  );
}

/** ghost 填入动作（Tab 与按钮共用）：填入整句建议并清除 hint（§10.4）。
 * hint 取本会话桶（16 号票）。键盘路径（Tab 优先级：@ 菜单 > hint）在
 * composer-keys.ts。 */
function useGhostFill(): (() => void) | null {
  const sessionId = useSessionScope();
  const hint = useAskStore((s) => s.hints[sessionId]);
  const clearHint = useAskStore((s) => s.clearHint);
  const input = useAuiState((s) => s.composer.text);
  const aui = useAui();
  if (matchGhost(hint ?? null, input) === null) return null;
  return () => {
    aui.composer.setText(hint ?? "", { origin: "user" });
    clearHint(sessionId);
  };
}

/** 建议提示词 ghost（§10.4：空输入整句 / 前缀匹配补尾；接受 = 填入不发送；
 * hint 取本会话桶，16 号票）。 */
function ComposerGhost() {
  const sessionId = useSessionScope();
  const hint = useAskStore((s) => s.hints[sessionId]) ?? null;
  const input = useAuiState((s) => s.composer.text);
  const ghost = matchGhost(hint, input);
  const fill = useGhostFill();
  if (ghost === null || !fill) return null;
  return (
    <div
      data-testid="composer-ghost"
      className="mt-1 flex items-center gap-2 px-2 text-meta text-[var(--muted-foreground)]"
    >
      <span>
        <span className="text-[var(--foreground)]">{input}</span>
        {ghost}
      </span>
      <button
        type="button"
        onClick={fill}
        className="rounded border border-[var(--border)] px-1.5 py-0.5 text-[var(--muted-foreground)] hover:border-[var(--ring)]"
      >
        Tab 填入
      </button>
    </div>
  );
}
