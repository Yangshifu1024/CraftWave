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
  MessagePartPrimitive,
  MessagePrimitive,
  ThreadPrimitive,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import { useEffect, useRef, useState } from "react";
import { useComposerKeys } from "@/chat/composer-keys";
import { TIER_LABELS, usePermissionTierStore } from "@/chat/permission-tier";
import { useSessionMetrics } from "@/chat/run-metrics";
import { useRunSummaries, useSessionScope } from "@/chat/session-scope";
import { matchGhost, useAskStore } from "@/stores/ask";
import { useSessionsStore } from "@/stores/sessions";

/** 消息区（FR-13-15 滚动跟随：Viewport autoScroll 内建「跟随最新 + 上滚
 * 暂停」；不在底部时显示回底按钮，17 号票）。 */
export function ChatThread() {
  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-[var(--background)] text-[var(--foreground)]">
      <ThreadPrimitive.Root className="relative flex min-h-0 flex-1 flex-col">
        <ThreadPrimitive.Viewport className="flex flex-1 flex-col overflow-y-auto p-4">
          <AuiIf condition={(s) => s.thread.isEmpty}>
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-sm text-[var(--muted-foreground)]">
              <p className="text-lg text-[var(--foreground)]">CraftWave</p>
              <p>空聊天窗已就绪 —— rig 流式接入见 S2/S3 票</p>
            </div>
          </AuiIf>
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
          <ThreadPrimitive.ViewportFooter className="sticky bottom-0 pt-2">
            <ComposerBar />
            <ComposerGhost />
            <RunMetricsBar />
          </ThreadPrimitive.ViewportFooter>
        </ThreadPrimitive.Viewport>
        {/* 回底按钮（FR-13-15）：官方原语自带「不在底部才显示」判定 */}
        <ThreadPrimitive.ScrollToBottom className="absolute bottom-32 left-1/2 z-10 -translate-x-1/2 rounded-full border border-[var(--border)] bg-[var(--card)] px-3 py-1 text-xs text-[var(--muted-foreground)] shadow" />
      </ThreadPrimitive.Root>
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
          className="absolute bottom-full left-0 z-20 mb-1 max-h-56 w-72 overflow-y-auto rounded border border-[var(--border)] bg-[var(--card)] py-1 text-xs shadow"
        >
          {atMenu.items.map((hit, index) => (
            <li key={hit.path}>
              <button
                type="button"
                className={`flex w-full items-center gap-2 px-2 py-1 text-left ${
                  index === atMenu.activeIndex
                    ? "bg-[var(--accent)] text-[var(--foreground)]"
                    : "text-[var(--muted-foreground)]"
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
      <ComposerPrimitive.Root className="flex items-end gap-2 rounded-2xl border border-[var(--border)] bg-[var(--card)] p-2">
        <ComposerPrimitive.Input
          rows={1}
          placeholder="发一条消息试试骨架回声…"
          className="max-h-32 flex-1 resize-none bg-transparent px-2 py-1.5 text-sm focus:outline-none"
          onKeyDown={onKeyDown}
          onKeyUp={(e) => onCursor(e.currentTarget.selectionStart ?? 0)}
          onClick={(e) => onCursor(e.currentTarget.selectionStart ?? 0)}
        />
        <TierBadge />
        <AuiIf condition={(s) => !s.thread.isRunning}>
          <ComposerPrimitive.Send
            aria-label="发送"
            className="rounded-full bg-[var(--primary)] px-3 py-1.5 text-sm text-[var(--primary-foreground)]"
          />
        </AuiIf>
        <AuiIf condition={(s) => s.thread.isRunning}>
          <ComposerPrimitive.Cancel className="rounded-full border border-[var(--border)] px-3 py-1.5 text-sm">
            停止
          </ComposerPrimitive.Cancel>
        </AuiIf>
      </ComposerPrimitive.Root>
    </div>
  );
}

/** 权限档指示（FR-13-02：Shift+Tab 循环切换，§10.6 内存态）。 */
function TierBadge() {
  const sessionId = useSessionScope();
  const tier = usePermissionTierStore((s) => s.bySession[sessionId] ?? "autoEdit");
  return (
    <span
      data-testid="tier-badge"
      className="shrink-0 rounded-full border border-[var(--border)] px-2 py-0.5 text-[11px] text-[var(--muted-foreground)]"
      title="Shift+Tab 切换权限档（本次会话有效）"
    >
      {TIER_LABELS[tier]}
    </span>
  );
}

/** 用户消息气泡：右对齐，仅渲染文本 part，附编辑重发操作条。 */
function UserMessage() {
  return (
    <MessagePrimitive.Root className="flex flex-col items-end py-1">
      <div className="max-w-[80%] rounded-2xl bg-[var(--primary)] px-4 py-2 text-sm text-[var(--primary-foreground)]">
        <MessagePrimitive.Parts>
          {({ part }) => (part.type === "text" ? <MessagePartPrimitive.Text /> : null)}
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
      {/* 摘要行（§10.4）：过程存在且收起时显示；点击展开/收起 */}
      {collapsed && processParts.length > 0 && (
        <button
          type="button"
          data-testid="run-summary"
          onClick={toggle}
          className="mb-1 flex items-center gap-2 rounded-full border border-[var(--border)] bg-[var(--card)] px-3 py-0.5 text-xs text-[var(--muted-foreground)] hover:border-[var(--ring)]"
        >
          <span aria-hidden>⚙</span>
          <span>
            {summary?.toolCount ?? toolCount} 次工具调用
            {durationMs !== undefined ? ` · ${formatDuration(durationMs)}` : ""}
          </span>
          {interrupted && <span className="text-[var(--destructive)]">已中断</span>}
          <span className="text-[var(--cw-dim)]">展开</span>
        </button>
      )}
      <div className="w-full max-w-[80%] rounded-2xl bg-[var(--card)] px-4 py-2 text-sm">
        {/* 内联 render props（勿抽组件）：S1 实证 Parts 对 tool-call part 渲染
            null 时才 fallback 到注册的镜像工具卡；抽组件会让 fallback 判定失效。
            折叠态过程 part 返回隐藏空元素（非 null）以抑制 fallback 卡。 */}
        <MessagePrimitive.Parts>
          {({ part }) => {
            if (part.type === "text") {
              return part.text.length === 0 ? (
                <span hidden />
              ) : (
                <p className="whitespace-pre-wrap">
                  <MessagePartPrimitive.Text />
                </p>
              );
            }
            if (collapsed) return <span hidden />;
            if (part.type === "reasoning") {
              return (
                <details className="my-1 rounded border border-[var(--border)] bg-[var(--muted)] px-2 py-1 text-xs text-[var(--muted-foreground)]">
                  <summary className="cursor-default select-none">思考过程</summary>
                  <p className="mt-1 whitespace-pre-wrap">{part.text}</p>
                </details>
              );
            }
            return null;
          }}
        </MessagePrimitive.Parts>
        <MessagePrimitive.Error>
          <ErrorPrimitive.Root className="text-xs text-[var(--destructive)]">
            <ErrorPrimitive.Message />
          </ErrorPrimitive.Root>
        </MessagePrimitive.Error>
      </div>
      <BranchBar editOnly={false} />
    </MessagePrimitive.Root>
  );
}

/** 分支操作条（§6.3 树操作 API 的 UI 接地，15 号票）：上一/下一分支 +
 * 重生成（assistant）/ 编辑重发（user）/ 复制。官方原语自带禁用态。 */
function BranchBar({ editOnly }: { editOnly: boolean }) {
  const btn =
    "rounded border border-transparent px-1.5 py-0.5 text-[11px] text-[var(--muted-foreground)] hover:border-[var(--border)] hover:text-[var(--foreground)] disabled:opacity-30";
  return (
    <div className="mt-0.5 flex items-center gap-0.5" data-testid="branch-bar">
      <BranchPickerPrimitive.Root className="flex items-center gap-0.5">
        <BranchPickerPrimitive.Previous className={btn} aria-label="上一个分支">
          ←
        </BranchPickerPrimitive.Previous>
        <span className="text-[11px] text-[var(--cw-dim)]">
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

/** 计数条三数（§10.5：rate / turnUsage / sessionUsage 常驻 Composer 下方；
 * 16 号票按本会话桶取数）。 */
function RunMetricsBar() {
  const sessionId = useSessionScope();
  const { rate, turnUsage, sessionUsage, lastRun } = useSessionMetrics(sessionId);
  const tokens = (u: { input: number; output: number }) => `${u.input + u.output}`;
  return (
    <div className="mt-1 flex items-center gap-3 px-2 text-[11px] text-[var(--muted-foreground)]">
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
      className="mt-1 flex items-center gap-2 px-2 text-[11px] text-[var(--muted-foreground)]"
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
