/**
 * 聊天主界面（Thread.tsx）：用 assistant-ui 的原语组件手工装配的完整聊天窗。
 *
 * 界面像素级复刻 ZCode 桌面端（对照其 v4/ConversationTimeline、
 * ConversationUserInputBubble、reasoning.tsx、ChatPromptEditor）：
 * - 消息列限宽居中（max-w-4xl），用户消息右对齐浅底气泡（右上角小圆角），
 *   助手内容平铺无气泡（text-ui-base + 1.75 行高 + 宽字距）；
 * - 思考过程为单行折叠行（脑图标 + 「思考」+ hover 显现的展开箭头），
 *   展开后左侧竖导线缩进；
 * - 输入区为 16px 大圆角白卡（hover/聚焦边框加深），工具栏行 = 附件占位 +
 *   盾牌权限档下拉 + 模型选择 + 方形发送钮（brand 底白箭头）；
 * - 状态栏（速率/本轮/会话三数 + 思考计时）常驻卡片底部一行。
 * 这里只管单个会话内的呈现，多会话的装配在 SessionWorkspace.tsx。
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
  useAuiState,
} from "@assistant-ui/react";
import {
  ArrowDown,
  ArrowUp,
  ChevronLeft,
  ChevronRight,
  Copy,
  Loader,
  Plus,
  RefreshCw,
  ShieldCheck,
  Square,
} from "lucide-react";
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
 *  暂停」；不在底部时显示回底按钮，17 号票）。回底按钮为自研实现
 *  （见 ScrollBottomButton），滚动判定与点击行为完全可控。 */
export function ChatThread() {
  const viewportRef = useRef<HTMLDivElement>(null);
  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-background text-foreground">
      <ThreadPrimitive.Root className="relative flex min-h-0 flex-1 flex-col">
        <ThreadPrimitive.Viewport
          ref={viewportRef}
          className="flex flex-1 flex-col overflow-y-auto [scrollbar-gutter:stable]"
        >
          <AuiIf condition={(s) => s.thread.isEmpty}>
            {/* 空态（ZCode 草稿视图口径）：顶部约 29dvh 留白 + max-w-2xl 居中窄列 */}
            <div className="mx-auto flex w-full max-w-2xl flex-1 flex-col items-center justify-start gap-2 px-6 pt-[29dvh] text-ui-base text-foreground-subtle">
              <p className="text-ui-xl font-semibold text-foreground">CraftWave</p>
              <p>新对话已就绪</p>
            </div>
          </AuiIf>
          {/* 内容列限宽居中（ZCode conversationLayout：max-w-4xl 居中）；
              内边距与 Composer 容器严格一致（px-4），保证消息列与输入卡等宽 */}
          <div className="mx-auto w-full max-w-4xl shrink-0 px-4">
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
            {/* 运行中末尾 spinner（ZCode ChatLoading：size-4 弧形 loader） */}
            <AuiIf condition={(s) => s.thread.isRunning}>
              <div
                data-testid="thread-spinner"
                className="flex min-h-5 items-center py-1"
                data-zcode-chat-loading-animate="true"
              >
                <span className="flex size-4 animate-spin items-center justify-center text-foreground-subtle">
                  <Loader className="size-4" />
                </span>
              </div>
            </AuiIf>
          </div>
          <ThreadPrimitive.ViewportFooter className="sticky bottom-0 bg-background">
            {/* 顶部渐变带：Composer 上方的消息尾部自然淡入背景；底座实底
                （bg-background）封住卡片下缘，消息不再从透明区漏出 */}
            <div
              aria-hidden
              className="pointer-events-none mx-auto h-16 w-full max-w-4xl bg-gradient-to-t from-background to-transparent"
            />
            <div className="mx-auto w-full max-w-4xl px-4 pb-4">
              <ComposerBar />
            </div>
          </ThreadPrimitive.ViewportFooter>
        </ThreadPrimitive.Viewport>
        <ScrollBottomButton viewportRef={viewportRef} />
      </ThreadPrimitive.Root>
    </div>
  );
}

/** 回底按钮（自研，FR-13-15）：距底超过 24px 时浮现，点击平滑回底。
 *  之前用 ThreadPrimitive.ScrollToBottom，实际运行中「常驻且点击无效」
 *  （原语的 isAtBottom 判定/滚动联动在真实 DOM 层级下不可控）——改为
 *  直接监听滚动容器，行为完全透明。 */
function ScrollBottomButton({
  viewportRef,
}: {
  viewportRef: React.RefObject<HTMLDivElement | null>;
}) {
  const [away, setAway] = useState(false);
  useEffect(() => {
    const el = viewportRef.current;
    if (el === null) return;
    const update = () => {
      const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
      setAway(distance > 24);
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    // 内容高度变化（流式追加/历史收养）也影响距底值，一并重判
    const observer = new ResizeObserver(update);
    for (const child of el.children) observer.observe(child);
    return () => {
      el.removeEventListener("scroll", update);
      observer.disconnect();
    };
  }, [viewportRef]);
  if (!away) return null;
  return (
    <button
      type="button"
      data-testid="scroll-to-bottom"
      aria-label="滚动到底部"
      onClick={() => {
        const el = viewportRef.current;
        el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
      }}
      className="absolute bottom-40 left-1/2 z-10 -translate-x-1/2 rounded-full border border-border bg-card p-2 text-foreground-subtle shadow-sm transition-colors hover:bg-card-selected hover:text-foreground"
    >
      <ArrowDown className="size-4" />
    </button>
  );
}

/** Composer 区（§10.6 / FR-13-02，17 号票）：16px 大圆角输入卡（ZCode
 *  ChatPromptEditor：rounded-2xl border-input-border bg-input p-3），
 *  Enter/Shift+Enter/IME 由 ComposerPrimitive.Input 原生承担。 */
function ComposerBar() {
  const { onKeyDown, onCursor, atMenu, pickActive, hoverIndex } = useComposerKeys();
  const sessionId = useSessionScope();
  const input = useAuiState((s) => s.composer.text);
  const hint = useAskStore((s) => s.hints[sessionId]) ?? null;
  const ghost = matchGhost(hint, input);
  // ghost hint 仅单行输入时启用（镜像层不做多行排版）；此时占位符让位
  const ghostVisible = ghost !== null && !input.includes("\n");
  return (
    <div className="relative">
      {/* @ 路径补全浮层（search_workspace_paths 驱动；Tab/Enter 选中，Esc 关闭） */}
      {atMenu !== null && atMenu.items.length > 0 && (
        <ul
          data-testid="at-menu"
          className="absolute bottom-full left-0 z-20 mb-1 max-h-56 w-72 overflow-y-auto rounded-lg border border-popover-border bg-popover py-1 text-ui-base shadow-md"
        >
          {atMenu.items.map((hit, index) => (
            <li key={hit.path}>
              <button
                type="button"
                className={`flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left ${
                  index === atMenu.activeIndex
                    ? "bg-menu-hover text-foreground"
                    : "text-foreground-subtle"
                }`}
                onClick={pickActive}
                onMouseEnter={() => hoverIndex(index)}
              >
                <span aria-hidden>{hit.kind === "dir" ? "▣" : "▢"}</span>
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
            className="mb-1 flex items-center gap-2 rounded-lg border border-dashed border-border bg-surface px-3 py-1.5 text-ui-sm text-foreground-subtle"
          >
            <span className="shrink-0 rounded-full border border-border px-1.5 py-px text-ui-2xs">
              排队中
            </span>
            <QueueItemPrimitive.Text className="min-w-0 flex-1 truncate" />
            <QueueItemPrimitive.Remove
              aria-label="移除排队消息"
              className="shrink-0 rounded px-1 hover:text-destructive"
            >
              <XGlyph />
            </QueueItemPrimitive.Remove>
          </div>
        )}
      </ComposerPrimitive.Queue>
      <ComposerPrimitive.Root className="relative flex flex-col gap-3 overflow-hidden rounded-2xl border border-input-border bg-input p-3 transition-colors hover:border-input-border-hover focus-within:border-input-border-focused focus-within:bg-input-focused">
        {/* 第一层：输入框（无独立边框，占满卡片宽；min 40px / max 160px）。
            ghost 预测以「输入框内 hint」呈现（对照 ZCode Lexical 的内联灰色
            续写）：镜像层叠在 textarea 上，已输入部分透明占位、尾部灰色续写；
            仅单行输入时启用（镜像层不做多行排版），此时占位符让位。 */}
        <div className="relative">
          <GhostHintMirror visible={ghostVisible} inputText={input} ghost={ghost ?? ""} />
          <ComposerPrimitive.Input
            rows={1}
            placeholder={ghostVisible ? "" : "发一条消息，运行中的消息将自动排队…"}
            className="max-h-40 min-h-10 w-full resize-none bg-transparent px-1 text-ui-base leading-5 text-foreground outline-none placeholder:text-foreground-subtlest"
            onKeyDown={onKeyDown}
            onKeyUp={(e) => onCursor(e.currentTarget.selectionStart ?? 0)}
            onClick={(e) => onCursor(e.currentTarget.selectionStart ?? 0)}
          />
        </div>
        {/* 第二层：工具栏——左（附件占位 + 权限档下拉）右（模型 + 发送/停止） */}
        <div className="group/toolbar flex items-end gap-3">
          <div className="flex min-w-0 flex-1 items-center">
            <div className="flex shrink-0 items-center gap-1">
              <button
                type="button"
                aria-label="附件（M2 提供）"
                disabled
                title="附件功能随 M2 提供"
                className="inline-flex size-7 shrink-0 items-center justify-center rounded-lg text-foreground-subtle opacity-50"
              >
                <Plus className="size-4" />
              </button>
              <TierMenu />
            </div>
          </div>
          <div className="ml-auto flex shrink-0 items-center justify-end gap-1.5">
            <ModelPicker />
            {/* 发送/停止单钮双态（对齐 ZCode）：空闲=brand 底发送；运行中=secondary
                底停止。运行中入队走键盘 Enter（消息队列机制，队列条展示） */}
            <AuiIf condition={(s) => s.thread.isRunning}>
              <ComposerPrimitive.Cancel
                aria-label="停止"
                className="inline-flex size-7 shrink-0 cursor-pointer items-center justify-center gap-1 rounded-lg bg-secondary text-foreground transition-opacity hover:opacity-80"
              >
                <Square className="size-3.5 fill-current" />
              </ComposerPrimitive.Cancel>
            </AuiIf>
            <AuiIf condition={(s) => !s.thread.isRunning}>
              <ComposerPrimitive.Send
                aria-label="发送"
                className="inline-flex size-7 shrink-0 cursor-pointer items-center justify-center gap-1 rounded-lg bg-brand text-foreground-inverse transition-opacity hover:opacity-80 disabled:opacity-50"
              >
                <ArrowUp className="size-4" />
              </ComposerPrimitive.Send>
            </AuiIf>
          </div>
        </div>
        {/* 第三层：状态栏（A5）——思考计时 + 速率/本轮/会话三数 + 工具次数 */}
        <ComposerStatusBar />
      </ComposerPrimitive.Root>
    </div>
  );
}

/** ×字形（队列移除钮），lucide X 的内联等价（避免额外 import 噪音）。 */
function XGlyph() {
  return (
    <svg
      className="size-3.5"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      aria-hidden
    >
      <path d="M18 6 6 18M6 6l12 12" />
    </svg>
  );
}

/** 权限档下拉（FR-13-02）：盾牌图标 + 当前档名（ZCode V4ComposerModeControls：
 *  h-7 rounded-lg px-2 + ShieldCheck size-4 + chevron size-3.5），点开四档
 *  清单直接选定（Shift+Tab 循环仍可用，§10.6 内存态——本次会话有效）。 */
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
          className="absolute bottom-full left-0 z-20 mb-2 w-64 rounded-xl border border-popover-border bg-menu p-1 shadow-md"
        >
          {TIERS.map((t) => (
            <li key={t}>
              <button
                type="button"
                className={`flex w-full items-center gap-2.5 rounded-lg px-2 py-2 text-left text-ui-base ${
                  t === tier
                    ? "bg-selected text-foreground"
                    : "text-foreground-subtle hover:bg-menu-hover hover:text-foreground"
                }`}
                onClick={() => {
                  setTier(sessionId, t);
                  setOpen(false);
                }}
              >
                <ShieldCheck className="size-4 shrink-0" />
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
        className="inline-flex h-7 shrink-0 items-center gap-1 rounded-lg px-2 text-ui-base text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
        title="权限档（本次会话有效，Shift+Tab 循环切换）"
      >
        <ShieldCheck className="size-4" />
        {TIER_LABELS[tier]}
        <ChevronRight className="size-3.5 -rotate-90" />
      </button>
    </div>
  );
}

/**
 * 模型选择下拉（FR-13-05 Ctrl+L 的 M1 简化形态）：列出配置里全部供应商
 * 的模型；选中即本会话覆盖（发送时经 RunConfig.modelId 生效，§11.1），
 * 「跟随默认」回到全局 activeModelId。清单在每次打开时拉取（设置页
 * 改动即时反映）。触发钮对齐 ZCode ModelConfigSelect（rounded-lg pl-2
 * pr-1.5 + chevron）。
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
          className="absolute bottom-full right-0 z-20 mb-2 max-h-64 w-64 overflow-y-auto rounded-xl border border-popover-border bg-menu p-1 shadow-md"
        >
          <li>
            <button
              type="button"
              className={`flex w-full rounded-lg px-2 py-1.5 text-left text-ui-base ${
                selected === undefined
                  ? "bg-selected text-foreground"
                  : "text-foreground-subtle hover:bg-menu-hover hover:text-foreground"
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
              <div className="px-2 py-1 text-ui-xs text-foreground-subtlest">{group.provider}</div>
              <ul>
                {group.models.map((model) => (
                  <li key={model.id}>
                    <button
                      type="button"
                      className={`flex w-full rounded-lg px-2 py-1.5 text-left text-ui-base ${
                        model.id === selected
                          ? "bg-selected text-foreground"
                          : "text-foreground-subtle hover:bg-menu-hover hover:text-foreground"
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
        className="inline-flex h-7 min-w-0 max-w-44 shrink-0 items-center gap-1 rounded-lg pl-2 pr-1.5 text-ui-base text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
        title="选择本会话使用的模型"
      >
        <span className="truncate">{currentName}</span>
        <ChevronRight className="size-3.5 shrink-0 -rotate-90" />
      </button>
    </div>
  );
}

/** 用户消息：右对齐浅底气泡（ZCode ConversationUserInputBubble：rounded-xl
 *  + 右上角小圆角 rounded-tr-xs + border + surface 底 + 16px/12px 内边距）。 */
function UserMessage() {
  return (
    <MessagePrimitive.Root className="group/user-row mb-5 flex w-full flex-col items-end">
      <div className="flex max-w-[min(100%,36rem)] flex-col gap-2 rounded-xl rounded-tr-xs border border-border bg-surface px-4 py-3 text-ui-base text-foreground">
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
 * 平铺排版对照 ZCode：无气泡、text-ui-base、与过程行同列。
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
    <MessagePrimitive.Root className="group/assistant-row mb-5 flex w-full flex-col items-start">
      {/* 摘要行（§10.4）：过程存在且收起时显示；点击展开/收起。样式对照
          ZCode 工时折叠头（text-ui-base subtle + chevron 旋转）。 */}
      {collapsed && processParts.length > 0 && (
        <button
          type="button"
          data-testid="run-summary"
          onClick={toggle}
          className="group/history-message inline-flex max-w-full cursor-pointer items-center gap-2 rounded px-1 py-0.5 text-left text-ui-base text-foreground-subtle transition-colors hover:text-foreground"
        >
          <ChevronRight className="size-4 shrink-0 text-foreground-subtlest opacity-70 transition-transform" />
          <span className="font-medium">{summary?.toolCount ?? toolCount} 次工具调用</span>
          {durationMs !== undefined && (
            <span className="font-normal text-foreground-subtlest">
              · {formatDuration(durationMs)}
            </span>
          )}
          {interrupted && <span className="font-normal text-destructive">已中断</span>}
        </button>
      )}
      {/* 助手内容平铺（无气泡底色），与过程行同列（ZCode ConversationRowView:1693） */}
      <div className="w-full text-ui-base">
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
                <Streamdown className="cw-md" data-zcode-stream-animate="true">
                  {part.text}
                </Streamdown>
              );
            }
            if (collapsed) return <span hidden />;
            if (part.type === "reasoning") {
              // 思考过程单行折叠行（ZCode reasoning.tsx trigger：脑图标 +
              // 完成态「思考 · 持续了 N 秒」/ 流式态「思考中」扫光 +
              // 展开箭头 hover 显现；展开后竖导线缩进）
              const streaming = part.status?.type === "running";
              const durationSec =
                part.timing?.completedAt !== undefined
                  ? Math.max(
                      1,
                      Math.round((part.timing.completedAt - part.timing.startedAt) / 1000),
                    )
                  : undefined;
              return (
                <details className="group/reasoning w-full" data-zcode-tool-stream-animate="true">
                  <summary className="inline-flex max-w-full min-w-0 cursor-pointer select-none list-none items-center gap-2 text-left text-ui-base transition-colors [&::-webkit-details-marker]:hidden">
                    <BrainGlyph className="size-4 shrink-0 text-foreground-subtlest" />
                    {streaming ? (
                      <span className="animated-gradient-text font-medium whitespace-nowrap">
                        思考中
                      </span>
                    ) : (
                      <>
                        <span className="font-medium whitespace-nowrap text-foreground-subtlest">
                          思考
                        </span>
                        {durationSec !== undefined && (
                          <>
                            <span className="font-normal text-foreground-subtlest">·</span>
                            <span className="font-normal text-foreground-subtlest">
                              持续了{durationSec} 秒
                            </span>
                          </>
                        )}
                      </>
                    )}
                    <ChevronRight className="size-4 shrink-0 text-foreground-subtlest opacity-0 transition-all duration-200 group-hover/reasoning:opacity-100 group-open/reasoning:rotate-90 group-open/reasoning:opacity-100" />
                  </summary>
                  <div className="pt-3">
                    <div className="ml-2 max-h-60 space-y-2 overflow-auto border-l border-border pl-3.5">
                      <p className="min-w-0 whitespace-pre-wrap break-words text-ui-base text-foreground-subtlest">
                        {part.text}
                      </p>
                    </div>
                  </div>
                </details>
              );
            }
            return null;
          }}
        </MessagePrimitive.Parts>
        <MessagePrimitive.Error>
          <ErrorPrimitive.Root className="text-ui-sm text-destructive">
            <ErrorPrimitive.Message />
          </ErrorPrimitive.Root>
        </MessagePrimitive.Error>
      </div>
      <BranchBar editOnly={false} />
    </MessagePrimitive.Root>
  );
}

/** 脑字形（思考行图标）：lucide Brain 的线性造型，随 currentColor。 */
function BrainGlyph({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588A4 4 0 1 0 12 18Z" />
      <path d="M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588A4 4 0 1 1 12 18Z" />
      <path d="M15 13a4.5 4.5 0 0 1-3-4 4.5 4.5 0 0 1-3 4" />
      <path d="M17.599 6.5a3 3 0 0 0 .399-1.375" />
      <path d="M6.003 5.125A3 3 0 0 0 6.401 6.5" />
      <path d="M3.477 10.896a4 4 0 0 1 .585-.396" />
      <path d="M19.938 10.5a4 4 0 0 1 .585.396" />
      <path d="M6 18a4 4 0 0 1-1.967-.516" />
      <path d="M19.967 17.484A4 4 0 0 1 18 18" />
    </svg>
  );
}

/** 分支操作条（§6.3 树操作 API 的 UI 接地，15 号票）：上一/下一分支 +
 * 重生成（assistant）/ 编辑重发（user）/ 复制。官方原语自带禁用态。
 * 样式对齐 ZCode 的行内 ghost 小钮（size-6 圆角 + subtlest 前景）。 */
function BranchBar({ editOnly }: { editOnly: boolean }) {
  const btn =
    "inline-flex size-6 items-center justify-center rounded-md text-foreground-subtlest transition-colors hover:bg-hover hover:text-foreground disabled:opacity-30";
  return (
    <div className="mt-1 flex items-center gap-0.5" data-testid="branch-bar">
      <BranchPickerPrimitive.Root className="flex items-center gap-0.5">
        <BranchPickerPrimitive.Previous className={btn} aria-label="上一个分支">
          <ChevronLeft className="size-3.5" />
        </BranchPickerPrimitive.Previous>
        <span className="text-ui-xs text-foreground-subtlest">
          <BranchPickerPrimitive.Count />
        </span>
        <BranchPickerPrimitive.Next className={btn} aria-label="下一个分支">
          <ChevronRight className="size-3.5" />
        </BranchPickerPrimitive.Next>
      </BranchPickerPrimitive.Root>
      <ActionBarPrimitive.Root className="flex items-center gap-0.5">
        {editOnly ? (
          <ActionBarPrimitive.Edit className={btn} aria-label="编辑重发">
            <PenLineGlyph />
          </ActionBarPrimitive.Edit>
        ) : (
          <>
            <ActionBarPrimitive.Reload className={btn} aria-label="重新生成">
              <RefreshCw className="size-3.5" />
            </ActionBarPrimitive.Reload>
            <ActionBarPrimitive.Copy className={btn} aria-label="复制">
              <Copy className="size-3.5" />
            </ActionBarPrimitive.Copy>
          </>
        )}
      </ActionBarPrimitive.Root>
    </div>
  );
}

function PenLineGlyph() {
  return (
    <svg
      className="size-3.5"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z" />
    </svg>
  );
}

/** Composer 状态栏（A5，§10.5 三数 + 思考计时 + 工具次数）：常驻输入卡片
 *  底部一行（text-ui-xs + subtlest 前景 + tabular-nums，ZCode 数字口径）。 */
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
      className="flex items-center gap-3 px-1 text-ui-xs tabular-nums text-foreground-subtlest"
    >
      {thinkingSince !== null && (
        <span data-testid="metric-thinking" className="text-foreground-subtle">
          思考中 {thinkingSeconds}s
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

/** ghost 预测 hint（§10.4）：镜像层叠在输入框上——已输入部分透明占位
 * （invisible 保持等宽），尾部灰色续写；接受走 Tab 键（composer-keys 的
 * 键盘优先级：@ 菜单 > hint）。仅单行输入时渲染（镜像层不做多行排版）。 */
function GhostHintMirror({
  visible,
  inputText,
  ghost,
}: {
  visible: boolean;
  inputText: string;
  ghost: string;
}) {
  if (!visible || ghost.length === 0) return null;
  return (
    <div
      aria-hidden
      data-testid="composer-ghost"
      className="pointer-events-none absolute inset-0 overflow-hidden px-1 py-0 text-ui-base leading-5 text-foreground"
    >
      <span className="invisible whitespace-pre-wrap break-words">{inputText}</span>
      <span className="whitespace-pre-wrap break-words text-foreground-subtlest">{ghost}</span>
    </div>
  );
}
