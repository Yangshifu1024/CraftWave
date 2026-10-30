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
  MessagePrimitive,
  ThreadPrimitive,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Copy,
  File,
  Folder,
  Hand,
  Lightbulb,
  LoaderCircle,
  Pencil,
  Plus,
  Shield,
  ShieldAlert,
  ShieldCheck,
  Square,
  Zap,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { ChatErrorBanner } from "@/chat/ChatErrorBanner";
import { CompactionDivider, CompactionIndicator } from "@/chat/CompactionUI";
import { useComposerKeys } from "@/chat/composer-keys";
import { removeQueuedFollowUp, resumeFollowUpQueue, useFollowUpStore } from "@/chat/followup-store";
import { Markdown } from "@/chat/Markdown";
import { TIER_LABELS, usePermissionTierStore } from "@/chat/permission-tier";
import { ReasoningPart } from "@/chat/ReasoningPart";
import { useSessionMetrics } from "@/chat/run-metrics";
import { useSessionModelStore } from "@/chat/session-model";
import { useRunSummaries, useSessionScope } from "@/chat/session-scope";
import { McpFallbackCard } from "@/chat/ToolCards";
import { getConfig } from "@/ipc/client";
import type { PermissionTier } from "@/ipc/types";
import { matchGhost, useAskStore } from "@/stores/ask";
import { useSessionsStore } from "@/stores/sessions";

/** selector 缺省空数组（模块级常量：zustand selector 每次返回新引用会触发
 *  useSyncExternalStore 的无限重渲染——「Maximum update depth」陷阱）。 */
const NO_ENTRIES: never[] = [];
const NO_NODES: never[] = [];

/** 消息区（FR-13-15 滚动跟随：Viewport autoScroll 内建「跟随最新 + 上滚
 *  暂停」；不在底部时显示回底按钮，17 号票）。内容列限宽居中（参考样式
 *  A1：工作流风格平铺，不占满整屏宽）。 */
export function ChatThread({ onOpenSettings }: { onOpenSettings?: () => void }) {
  return (
    // 透明背景：透出承载它的大圆角卡（bg-card），自身不再铺底色
    <div className="flex h-full min-h-0 flex-1 flex-col text-[var(--foreground)]">
      <ThreadPrimitive.Root className="relative flex min-h-0 flex-1 flex-col">
        <ThreadPrimitive.Viewport className="flex min-w-0 flex-1 flex-col overflow-y-auto px-4 py-6">
          <AuiIf condition={(s) => s.thread.isEmpty}>
            <ThreadEmptyState />
          </AuiIf>
          {/* 内容列限宽居中（约 62%，A1）：消息与过程行统一宽度 */}
          <div className="mx-auto min-w-0 w-full max-w-4xl space-y-4 2xl:max-w-6xl">
            {/* 压缩边界分隔提示（§6.3 定稿：非消息体，04 号票） */}
            <CompactionDivider />
            <ThreadPrimitive.Messages>
              {({ message }) =>
                // system = runMarker 等存储标记节点（custom 载荷保真，§6.2 跳过内容）——不渲染
                message.role === "system" ? null : message.role === "user" ? (
                  <UserMessage />
                ) : (
                  <AssistantMessage onOpenSettings={onOpenSettings} />
                )
              }
            </ThreadPrimitive.Messages>
            {/* steer 插话节点（06 号票：run:inject 回执本地插入，树收养后剔除） */}
            <SteerNodes />
            {/* 运行中末尾 spinner（A3） */}
            <AuiIf condition={(s) => s.thread.isRunning}>
              <div
                data-testid="thread-spinner"
                className="flex items-center gap-2 px-1 py-1 text-ui-sm text-[var(--muted-foreground)]"
              >
                <LoaderCircle
                  size={12}
                  aria-hidden
                  className="inline-block shrink-0 animate-spin"
                />
                处理中…
              </div>
            </AuiIf>
          </div>
        </ThreadPrimitive.Viewport>
        {/* composer 固定底区：在滚动容器（Viewport）之外，不随内容滚动、
            内容也不会钻入其背后（sticky 方案的两个老问题一并消除） */}
        <div className="shrink-0 px-4 pb-3">
          <div className="relative mx-auto w-full max-w-4xl 2xl:max-w-6xl">
            {/* 回底按钮（FR-13-15）：已在底部时原语输出 disabled，用
                disabled:hidden 隐藏；挂 composer 容器上方自适应其高度 */}
            <ThreadPrimitive.ScrollToBottom className="cw-pop-in absolute bottom-full left-1/2 z-20 mb-2 flex size-8 -translate-x-1/2 items-center justify-center rounded-full border border-[var(--border)] bg-[var(--card)] text-[var(--muted-foreground)] shadow hover:text-[var(--foreground)] disabled:hidden">
              <ArrowDown size={14} aria-hidden className="shrink-0" />
            </ThreadPrimitive.ScrollToBottom>
            {/* 压缩生命周期指示条（Composer 上方轻量呈现，04 号票） */}
            <CompactionIndicator />
            <ComposerBar />
            <ComposerGhost />
          </div>
        </div>
      </ThreadPrimitive.Root>
    </div>
  );
}

/** 权限档菜单图标（18px，档位映射同 ZCode：plan 灯泡 / confirmEach 手 /
 *  autoEdit 盾对勾 / fullAccess 盾警叹）。 */
function TierMenuIcon({ tier }: { tier: PermissionTier }) {
  const Icon =
    tier === "plan"
      ? Lightbulb
      : tier === "confirmEach"
        ? Hand
        : tier === "autoEdit"
          ? ShieldCheck
          : ShieldAlert;
  return <Icon size={18} aria-hidden className="mt-0.5 shrink-0 text-[var(--muted-foreground)]" />;
}

/** 时段问候（B1 空状态，六档边界 5/9/12/14/18/23 与 ZCode 同口径）。 */
function greeting(date: Date): string {
  const hour = date.getHours();
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
  const [now, setNow] = useState(() => new Date());
  // 跨小时保持问候准确，不要求用户重新打开空会话。
  useEffect(() => {
    const next = new Date(now);
    next.setHours(now.getHours() + 1, 0, 0, 0);
    const timer = setTimeout(() => setNow(new Date()), Math.max(1, next.getTime() - now.getTime()));
    return () => clearTimeout(timer);
  }, [now]);
  return (
    <div className="cw-empty-state mx-auto flex w-full max-w-4xl 2xl:max-w-6xl flex-1 flex-col items-center justify-center gap-5 py-10">
      <div className="flex flex-col items-center gap-1.5">
        <p className="cw-greeting font-medium text-[var(--foreground)]">{greeting(now)}</p>
        <p className="text-ui-base text-[var(--muted-foreground)]">
          开始新对话，或从下面的建议开始
        </p>
      </div>
      <div className="flex flex-wrap items-center justify-center gap-2">
        {EMPTY_STATE_SUGGESTIONS.map((text, index) => (
          <button
            key={text}
            type="button"
            className="cw-rise min-h-8 max-w-full rounded-lg py-1.5 border border-[var(--border)] px-3 text-ui-sm text-[var(--muted-foreground)] transition-colors hover:border-[var(--ring)] hover:text-[var(--foreground)]"
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

/** 队列条（FR-01-07 排队面，06 号票后端队列）：followUp 待发消息逐条展示
 *  （带「排队」徽标）——× 移除（后端删除 + 本地同步）、铅笔取出编辑（删除
 *  后文本回填输入框，改完 Alt+Enter 重新排队）；暂停态（run 失败，06 号票）
 *  显示横幅与「继续」入口。后端 queue.json 为投递权威，本地队列条以事件
 *  （run:queue）与本地操作共同驱动（stores/sessions.ts queues 桶）。 */
function FollowUpQueueBar() {
  const sessionId = useSessionScope();
  const entries = useSessionsStore((s) => s.queues[sessionId] ?? NO_ENTRIES);
  const paused = useSessionsStore((s) => s.queuePaused[sessionId] === true);
  const aui = useAui();
  if (entries.length === 0 && !paused) return null;
  const remove = (entryId: string) => {
    void removeQueuedFollowUp(sessionId, entryId).catch(() => {});
  };
  const edit = (entry: { id: string; text: string }) => {
    // 编辑取出：后端删除 + 本地同步 + 文本回填输入框（改完重新排队）
    void removeQueuedFollowUp(sessionId, entry.id).then((ok) => {
      if (ok) aui.composer.setText(entry.text, { origin: "user" });
    });
  };
  const resume = () => {
    void resumeFollowUpQueue(sessionId);
  };
  return (
    <div data-testid="followup-queue" className="mb-1 space-y-1">
      {paused && (
        <div
          data-testid="followup-paused"
          className="flex items-center gap-2 rounded-xl border border-[var(--warning)]/50 bg-[var(--muted)]/60 px-2.5 py-1 text-ui-sm text-[var(--warning)]"
        >
          <span className="min-w-0 flex-1 truncate">
            上一次运行失败，待发队列已暂停（{entries.length} 条待发）
          </span>
          <button
            type="button"
            data-testid="followup-resume"
            onClick={resume}
            className="shrink-0 rounded-lg border border-[var(--border)] px-2 py-0.5 text-ui-sm text-[var(--foreground)] hover:border-[var(--ring)]"
          >
            继续
          </button>
        </div>
      )}
      {entries.map((entry) => (
        <div
          key={entry.id}
          data-testid="queue-item"
          className="flex items-center gap-2 rounded-xl border border-[var(--border)] bg-[var(--muted)]/60 px-2.5 py-1 text-ui-sm text-[var(--muted-foreground)]"
        >
          <span className="shrink-0 rounded-full border border-[var(--border)] px-1.5 text-ui-sm leading-4">
            排队
          </span>
          <span className="min-w-0 flex-1 truncate text-[var(--foreground)]">{entry.text}</span>
          <button
            type="button"
            aria-label="编辑排队消息"
            title="取出编辑（改完 Alt+Enter 重新排队）"
            onClick={() => edit(entry)}
            className="shrink-0 rounded px-1 hover:text-[var(--foreground)]"
          >
            <Pencil size={12} aria-hidden className="shrink-0" />
          </button>
          <button
            type="button"
            aria-label="移除排队消息"
            onClick={() => remove(entry.id)}
            className="shrink-0 rounded px-1 hover:text-[var(--destructive)]"
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

/** steer 插话节点（06 号票，§5.5 本地插入面）：run:inject 回执驱动——时间线
 *  尾部带「插话」徽标的 user 节点。树重载收养后由 SessionTab 的 adoptTree
 *  按 entryId 剔除 store 节点（树内已有同 id 的普通 user 消息），此处不做
 *  消息列表订阅（assistant-ui messages selector 引用不稳定，防无限重渲染）。 */
function SteerNodes() {
  const sessionId = useSessionScope();
  const nodes = useFollowUpStore((s) => s.steerNodes[sessionId] ?? NO_NODES);
  if (nodes.length === 0) return null;
  return (
    <>
      {nodes.map((node) => (
        <div key={node.entryId} data-testid="steer-node" className="flex flex-col items-end py-2">
          <div className="flex max-w-full items-start gap-2">
            <span className="mt-3 shrink-0 rounded-full border border-[var(--border)] px-1.5 text-ui-sm leading-4 text-[var(--muted-foreground)]">
              插话
            </span>
            <div className="min-w-0 max-w-full rounded-xl rounded-tr-xs border border-[var(--border)] bg-[var(--muted)] px-4 py-3 text-ui-base leading-6 text-[var(--foreground)] sm:max-w-xl">
              <p className="whitespace-pre-wrap [overflow-wrap:anywhere]">{node.text}</p>
            </div>
          </div>
        </div>
      ))}
    </>
  );
}

/** Composer 区（§10.6 / FR-13-02，17 号票）：输入框 + 权限档指示 + @ 菜单
 *  浮层 + / 技能菜单浮层（09 号票）。
 *  Enter 三态路由（06 号票）/Shift+Enter 换行/IME 组合期防误发由
 *  useComposerKeys + ComposerPrimitive.Input 共同承担。 */
function ComposerBar() {
  const { onKeyDown, onCursor, atMenu, skillMenu, pickActive, pickSkill, hoverIndex, hoverSkill } =
    useComposerKeys();
  return (
    <div className="relative">
      {/* @ 路径补全浮层（search_workspace_paths 驱动；Tab/Enter 选中，Esc 关闭） */}
      {atMenu !== null && atMenu.items.length > 0 && (
        <ul
          data-testid="at-menu"
          className="cw-pop-in absolute bottom-full left-0 z-20 mb-1 max-h-56 w-72 overflow-y-auto rounded-lg border border-[var(--border)] bg-[var(--card)] p-1 text-ui-sm shadow"
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
                {hit.kind === "dir" ? (
                  <Folder size={12} aria-hidden className="shrink-0" />
                ) : (
                  <File size={12} aria-hidden className="shrink-0" />
                )}
                <span className="truncate">{hit.path}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {/* / 技能菜单浮层（09 号票，FR-01-17 技能半边：skill_list 驱动，
          ↑↓ 导航、Tab/Enter 选中展开为明确指令文本，Esc 关闭） */}
      {skillMenu !== null && skillMenu.items.length > 0 && (
        <ul
          data-testid="skill-menu"
          className="cw-pop-in absolute bottom-full left-0 z-20 mb-1 max-h-56 w-80 overflow-y-auto rounded-lg border border-[var(--border)] bg-[var(--card)] p-1 text-ui-sm shadow"
        >
          {skillMenu.items.map((skill, index) => (
            <li key={skill.name}>
              <button
                type="button"
                className={`flex w-full items-start gap-2 rounded-md px-2 py-1 text-left ${
                  index === skillMenu.activeIndex
                    ? "bg-[var(--accent)] text-[var(--foreground)]"
                    : "text-[var(--muted-foreground)]"
                }`}
                onClick={pickSkill}
                onMouseEnter={() => hoverSkill(index)}
              >
                <Zap size={12} aria-hidden className="mt-0.5 shrink-0" />
                <span className="flex min-w-0 flex-col">
                  <span className="truncate text-[var(--foreground)]">{skill.name}</span>
                  <span className="truncate text-ui-sm text-[var(--muted-foreground)]">
                    {skill.description}
                  </span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {/* followUp 队列条（06 号票后端队列：排队徽标 + 暂停横幅 + 继续入口） */}
      <FollowUpQueueBar />
      {/* 输入壳（ZCode 同款结构）：p-3 卡，输入框与工具栏/状态栏纵向 gap-2；
          hover 边框提亮一档（--input），focus-within 蓝边呼应聚焦令牌；
          底色用 --composer-bg（暗色 #272728 亮于中部卡，亮色同卡白） */}
      <ComposerPrimitive.Root className="flex flex-col gap-2 rounded-2xl border border-[var(--border)] bg-[var(--composer-bg)] p-3 transition-colors hover:border-[var(--input)] focus-within:border-[var(--cw-focus)]">
        <ComposerPrimitive.Input
          minRows={2}
          placeholder="发一条消息，Alt+Enter 排队待发…"
          className="max-h-40 w-full resize-none bg-transparent py-0.5 text-ui-base leading-5 placeholder:text-[var(--muted-foreground)] focus:outline-none"
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
            <Plus size={16} aria-hidden className="shrink-0" />
          </button>
          <TierMenu />
          <div className="ml-auto flex items-center gap-1.5">
            <ModelPicker />
            {/* 键位同位替换（ZCode 单键形态）：空闲=发送；运行中=停止（点击中断本轮）。
                运行中排队不再占按钮：queue capability 打开时 Input 的 Enter 仍入队
                （队列条照常展示）——按钮只承担「立即动作」（发送/停止），排队是键盘语义。 */}
            <AuiIf condition={(s) => s.thread.isRunning}>
              <ComposerPrimitive.Cancel
                aria-label="停止"
                title="停止（Esc）"
                className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-[var(--primary)] text-[var(--primary-foreground)] hover:opacity-90"
              >
                <Square size={14} aria-hidden fill="currentColor" className="shrink-0" />
              </ComposerPrimitive.Cancel>
            </AuiIf>
            <AuiIf condition={(s) => !s.thread.isRunning}>
              <ComposerPrimitive.Send
                aria-label="发送"
                className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-[var(--primary)] text-[var(--primary-foreground)] hover:opacity-90"
              >
                <ArrowUp size={16} aria-hidden className="shrink-0" />
              </ComposerPrimitive.Send>
            </AuiIf>
          </div>
        </div>
        {/* 状态栏（A5）——思考计时 + 速率/本轮/会话三数 + 工具次数（ZCode 无此层，保留） */}
        <ComposerStatusBar />
      </ComposerPrimitive.Root>
    </div>
  );
}

/** 权限档描述（双行菜单第二行，文案直接取自 ZCode zh-CN 语言包
 *  mode.description.glm.*，按档位映射：plan→plan / confirmEach→build /
 *  autoEdit→edit / fullAccess→yolo）。 */
const TIER_DESCRIPTIONS: Record<PermissionTier, string> = {
  plan: "编辑前先出计划。",
  confirmEach: "改文件前先问我。",
  autoEdit: "自动编辑文件。",
  fullAccess: "减少确认次数。",
};

/** 权限档下拉（FR-13-02）：双行菜单（图标 + 档名 + 描述，行尾勾标记当前
 *  档，ZCode 模式菜单同款）；fullAccess 触发器警示橙。Shift+Tab 循环仍可用，
 *  §10.6 内存态——本次会话有效。 */
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
          className="cw-pop-in absolute bottom-full left-0 z-20 mb-1 w-64 rounded-lg border border-[var(--border)] bg-[var(--card)] p-1 shadow"
        >
          {TIERS.map((t) => (
            <li key={t}>
              <button
                type="button"
                className="relative flex min-h-13 w-full items-start gap-3 rounded-md py-2 pr-8 pl-2 text-left hover:bg-[var(--accent)]"
                onClick={() => {
                  setTier(sessionId, t);
                  setOpen(false);
                }}
              >
                <TierMenuIcon tier={t} />
                <span className="flex min-w-0 flex-col gap-0.5">
                  <span className="text-ui-base text-[var(--foreground)]">{TIER_LABELS[t]}</span>
                  <span className="text-ui-sm text-[var(--muted-foreground)]">
                    {TIER_DESCRIPTIONS[t]}
                  </span>
                </span>
                {t === tier && (
                  <Check
                    size={16}
                    aria-hidden
                    className="absolute top-1/2 right-2 -translate-y-1/2 text-[var(--muted-foreground)]"
                  />
                )}
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
        className={`flex h-7 items-center gap-1 rounded-lg px-2 text-ui-sm hover:bg-[var(--accent)] ${
          tier === "fullAccess"
            ? "text-[var(--warning)] hover:text-[var(--warning)]"
            : "text-[var(--muted-foreground)]"
        }`}
        title="权限档（本次会话有效，Shift+Tab 循环切换）"
      >
        <Shield size={16} aria-hidden className="inline-block shrink-0 align-[-1px]" />
        {TIER_LABELS[tier]}
        <ChevronDown size={14} aria-hidden className="shrink-0 opacity-60" />
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
          className="cw-pop-in absolute right-0 bottom-full z-20 mb-1 max-h-72 w-64 overflow-y-auto rounded-lg border border-[var(--border)] bg-[var(--card)] p-1 shadow [mask-image:linear-gradient(to_bottom,black_calc(100%-24px),transparent)]"
        >
          <li>
            <button
              type="button"
              className="relative flex min-h-8 w-full items-center rounded-md pr-8 pl-2 text-left text-ui-base text-[var(--foreground)] hover:bg-[var(--accent)]"
              onClick={() => {
                setModel(sessionId, undefined);
                setOpen(false);
              }}
            >
              跟随默认
              {selected === undefined && (
                <Check
                  size={16}
                  aria-hidden
                  className="absolute top-1/2 right-2 -translate-y-1/2 text-[var(--muted-foreground)]"
                />
              )}
            </button>
          </li>
          {groups.map((group) => (
            <li key={group.provider}>
              <div className="-mx-1 my-1 h-px bg-[var(--border)]" />
              <div className="px-2 py-1.5 text-ui-sm font-medium text-[var(--cw-dim)]">
                {group.provider}
              </div>
              <ul className="space-y-0.5">
                {group.models.map((model) => (
                  <li key={model.id}>
                    <button
                      type="button"
                      className="relative flex min-h-8 w-full items-center gap-2 rounded-md pr-8 pl-2 text-left text-ui-base text-[var(--foreground)] hover:bg-[var(--accent)]"
                      onClick={() => {
                        setModel(sessionId, model.id);
                        setOpen(false);
                      }}
                    >
                      <span className="min-w-0 truncate">{model.name}</span>
                      {model.id === selected && (
                        <Check
                          size={16}
                          aria-hidden
                          className="absolute top-1/2 right-2 -translate-y-1/2 text-[var(--muted-foreground)]"
                        />
                      )}
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
        className="flex h-7 max-w-40 min-w-0 items-center gap-1 rounded-lg px-2 text-ui-sm text-[var(--muted-foreground)] hover:bg-[var(--accent)]"
        title="选择本会话使用的模型"
      >
        <span className="truncate">{currentName}</span>
        <ChevronDown size={14} aria-hidden className="shrink-0 opacity-60" />
      </button>
    </div>
  );
}

/** 用户消息（ZCode group/user-row 同款）：右对齐浅底气泡，复制/编辑操作行
 *  悬停才显（opacity-0 → group-hover 显），常显操作栏会把每条消息下方
 *  都钉一行按钮。 */
function UserMessage() {
  return (
    <MessagePrimitive.Root className="group/user-row flex flex-col items-end py-2">
      {/* 气泡右上 2px 折角 + 1px 边框（ZCode 同款），与平铺的助手消息区分 */}
      <div className="min-w-0 max-w-full rounded-xl rounded-tr-xs border border-[var(--border)] bg-[var(--muted)] px-4 py-3 text-ui-base leading-6 text-[var(--foreground)] sm:max-w-xl">
        <MessagePrimitive.Parts>
          {({ part }) =>
            part.type === "text" && part.text.length > 0 ? (
              <p className="whitespace-pre-wrap [overflow-wrap:anywhere]">{part.text}</p>
            ) : null
          }
        </MessagePrimitive.Parts>
      </div>
      <div className="mt-1 flex items-center gap-1 opacity-0 transition-opacity group-focus-within/user-row:opacity-100 group-hover/user-row:opacity-100">
        <BranchBar editOnly noTopGap />
      </div>
    </MessagePrimitive.Root>
  );
}

/**
 * 折叠分组状态（§10.4）：运行中（链上最后一条消息）过程区全量实时展开；
 * **最终答复开始输出的瞬间即折叠**（ZCode 的折叠发生在 run 结束，这里按
 * 需求前移到总结出现——finalAnswerStarted 见 AssistantMessage）；纯问答
 * （无工具、信号不触发）的 run 结束后留 3 秒兜底收起。摘要行是唯一出入口，
 * 点击随时展开/收起回看完整过程（过程数据由边车补全，FR-04-07）。
 */
function useRunCollapse(
  processCount: number,
  finalAnswerStarted: boolean,
): {
  collapsed: boolean;
  toggle: () => void;
} {
  const isRunning = useAuiState((s) => s.thread.isRunning);
  const isLast = useAuiState((s) => s.message.isLast);
  const active = isRunning && isLast;
  const [expanded, setExpanded] = useState(() => active);
  const prevActive = useRef(active);
  // 本轮 run 的自动折叠锁：总结信号只触发一次，避免后续状态变化反复开合
  const autoCollapsed = useRef(false);
  useEffect(() => {
    if (active && !prevActive.current) {
      // 新 run 开始：过程全部展开并复位折叠锁
      autoCollapsed.current = false;
      setExpanded(true);
    } else if (!active && prevActive.current) {
      // run 结束兜底（总结信号未触发的 run）：留 3 秒看完最后输出再收起
      if (!autoCollapsed.current && processCount > 0 && expanded) {
        const timer = setTimeout(() => setExpanded(false), 3000);
        prevActive.current = active;
        return () => clearTimeout(timer);
      }
    }
    prevActive.current = active;
  }, [active, processCount, expanded]);
  useEffect(() => {
    // 最终答复开始流式输出：立即折叠过程区，让总结独占视野
    if (active && finalAnswerStarted && !autoCollapsed.current) {
      autoCollapsed.current = true;
      setExpanded(false);
    }
  }, [active, finalAnswerStarted]);
  return { collapsed: !expanded, toggle: () => setExpanded((v) => !v) };
}

/** 时长格式化（ZCode formatConversationWorkDuration 口径）：秒取整最少 1 秒，
 *  中文单位，跨分钟带分。 */
function formatDuration(ms: number): string {
  const totalSeconds = Math.max(1, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes <= 0) return `${totalSeconds} 秒`;
  return seconds > 0 ? `${minutes} 分 ${seconds} 秒` : `${minutes} 分钟`;
}

/**
 * 助手消息：轮尾最终答复常显（流式）；思考卡、工具卡与过程叙述段按
 * useRunCollapse 的折叠状态显示，收起时展示「N 次工具调用 · 时长」摘要行
 * （运行中带 spinner），可点击展开。时长与中断态取自 run 收口摘要表
 * （SessionTab 注入，见 tree-persistence）。
 *
 * 折叠分界对齐 ZCode 轮渲染语义（latestAssistantTextRow）：工具调用之间的
 * 叙述短句属于过程历史，折叠后一并收进摘要行，只保留最后一段非空 text
 * （轮尾最终答复）常显——否则完成态仍散落着「现在深入」「：」这类碎片。
 */
function AssistantMessage({ onOpenSettings }: { onOpenSettings?: () => void }) {
  const error = useAuiState((s) =>
    s.message.status?.type === "incomplete" && s.message.status.reason === "error"
      ? String(s.message.status.error ?? "运行失败，请重试")
      : "",
  );
  const parts = useAuiState((s) => s.message.parts);
  const streaming = useAuiState((s) => s.message.status?.type === "running");
  // runId：live run 在 metadata.custom（adapter yield 写入，chat adapter 路径
  // 只进 custom）；历史恢复的消息在顶层（MessageLike 转换保留 runId）。两处都读。
  const runId = useAuiState((s) => {
    const meta = s.message.metadata as { runId?: string; custom?: { runId?: string } } | undefined;
    return meta?.custom?.runId ?? meta?.runId;
  });
  const summaries = useRunSummaries();
  const summary = runId !== undefined ? summaries.get(runId) : undefined;
  // 过程 parts = 思考流 + 工具卡（最终答复 text 常显）
  const processParts = parts.filter((p) => p.type === "reasoning" || p.type === "tool-call");
  // 最终答复开始信号（ZCode latestAssistantTextRow 的协议适配）：合批后
  // 「最后一次工具之后的第一个非空 text」= 总结开头（中间叙述都在工具之前，
  // 工具分界是帧协议权威）。命中即折叠过程区，总结独占视野。
  const finalAnswerStarted = (() => {
    if (!streaming) return false;
    let sawTool = false;
    for (const part of parts) {
      if (part.type === "tool-call") sawTool = true;
      else if (part.type === "text" && part.text.length > 0 && sawTool) return true;
    }
    return false;
  })();
  const { collapsed, toggle } = useRunCollapse(processParts.length, finalAnswerStarted);
  // 轮尾最终答复锚点：取最后一个非空 text 的内容做匹配（回调拿不到 part 序号，
  // 内容重复的极端场景只会多显示一段，不会把最终答复误藏进折叠区）。
  const finalText = (() => {
    for (let i = parts.length - 1; i >= 0; i--) {
      const part = parts[i];
      if (part.type === "text" && part.text.length > 0) return part.text;
    }
    return null;
  })();
  const sessionId = useSessionScope();
  const lastRun = useSessionMetrics(sessionId).lastRun;
  // 时长优先取 marker 收口摘要；运行刚结束的 run 由 run-metrics 兜底（此时 marker 尚未收养）
  const durationMs =
    summary?.durationMs ??
    (lastRun !== null && lastRun.runId === runId ? lastRun.durationMs : undefined);
  // 崩溃中断态（15 号票 shortcut 渲染层落地，17 号票）：marker 无 endedAt
  const interrupted = summary !== undefined && summary.endedAt === undefined;

  return (
    <MessagePrimitive.Root className="flex min-w-0 w-full flex-col items-start gap-2 py-1">
      {/* 摘要行（ZCode AssistantHistoryStatus 同款）：过程存在且收起时显示，
          「已工作/工作中 + 时长」+ 底部分隔线；点击展开/收起回看完整过程 */}
      {collapsed && processParts.length > 0 && (
        <button
          type="button"
          data-testid="run-summary"
          onClick={toggle}
          className="flex w-full items-center gap-2 rounded-sm border-b border-[var(--border)]/50 pb-2 text-left text-ui-base text-[var(--foreground-subtle)] transition-colors hover:text-[var(--foreground)]"
        >
          {streaming && <LoaderCircle size={12} aria-hidden className="shrink-0 animate-spin" />}
          <span className="min-w-0 truncate">
            {streaming ? "工作中" : "已工作"}
            {durationMs !== undefined ? ` ${formatDuration(durationMs)}` : ""}
          </span>
          {interrupted && <span className="shrink-0 text-[var(--destructive)]">已中断</span>}
          <ChevronRight
            size={16}
            aria-hidden
            className="shrink-0 text-[var(--foreground-subtlest)] opacity-70"
          />
        </button>
      )}
      {/* 工作流风格 A1：助手内容平铺（无气泡底色），与过程行同列；
          行距 16px 对齐 ZCode 过程区组容器（gap-4），正文/工具行/思考行同一节奏 */}
      <div className="min-w-0 w-full space-y-4 text-ui-base">
        {/* 内联 render props（勿抽组件）：S1 实证 Parts 对 tool-call part 渲染
            null 时才 fallback 到注册的镜像工具卡；抽组件会让 fallback 判定失效。
            折叠态过程 part 返回隐藏空元素（非 null）以抑制 fallback 卡。
            08 号票：toolUI 为 null = 工具未注册（MCP 动态工具等），兜底渲染
            McpFallbackCard（工具名 + 参数 JSON 折叠 + uiSummary + 错误态）。 */}
        <MessagePrimitive.Parts>
          {({ part }) => {
            if (part.type === "text") {
              // 折叠态只保留轮尾最终答复；过程叙述短句随摘要行收起（ZCode 同款）
              if (part.text.length === 0 || (collapsed && part.text !== finalText)) {
                return <span hidden />;
              }
              // 流式 Markdown（FR-01-03）：Streamdown 增量解析全量累计快照
              return <Markdown streaming={streaming}>{part.text}</Markdown>;
            }
            if (collapsed) {
              // 折叠不卸载只给工具卡（经注册渲染 part.toolUI 包 hidden 容器）：
              // 展开/收起瞬时切换且不重挂，卡内状态机与入场动画不受折叠往返
              // 影响；思考行无挂载语义，折叠即卸载（省计时与重渲染）
              if (part.type === "tool-call") {
                return <div hidden>{part.toolUI ?? <McpFallbackCard {...part} />}</div>;
              }
              return <span hidden />;
            }
            if (part.type === "reasoning") {
              return <ReasoningPart text={part.text} status={part.status} timing={part.timing} />;
            }
            if (part.type === "tool-call") {
              // toolUI 为 null = 该工具名未注册任何卡（MCP 动态工具，08 号票）
              // → 兜底卡；已注册工具返回 null，由 assistant-ui 落注册卡
              if (part.toolUI == null) {
                return <McpFallbackCard {...part} />;
              }
            }
            return null;
          }}
        </MessagePrimitive.Parts>
        <MessagePrimitive.Error>
          <ChatErrorBanner message={error} onOpenSettings={onOpenSettings} />
        </MessagePrimitive.Error>
      </div>
      <BranchBar editOnly={false} />
    </MessagePrimitive.Root>
  );
}

/** 保留历史分支切换、用户编辑重发和助手回复复制。 */
function BranchBar({ editOnly, noTopGap }: { editOnly: boolean; noTopGap?: boolean }) {
  const btn =
    "flex size-7 items-center justify-center rounded-md text-ui-sm text-[var(--muted-foreground)] hover:bg-[var(--accent)] hover:text-[var(--foreground)] disabled:opacity-30";
  return (
    <div
      className={`flex items-center gap-0.5 ${noTopGap ? "" : "mt-0.5"}`}
      data-testid="branch-bar"
    >
      <BranchPickerPrimitive.Root hideWhenSingleBranch className="flex items-center gap-0.5">
        <BranchPickerPrimitive.Previous className={btn} aria-label="上一个分支">
          <ChevronLeft size={14} aria-hidden />
        </BranchPickerPrimitive.Previous>
        <span className="text-ui-sm text-[var(--cw-dim)]">
          <BranchPickerPrimitive.Count />
        </span>
        <BranchPickerPrimitive.Next className={btn} aria-label="下一个分支">
          <ChevronRight size={14} aria-hidden />
        </BranchPickerPrimitive.Next>
      </BranchPickerPrimitive.Root>
      <ActionBarPrimitive.Root className="flex items-center gap-0.5">
        {editOnly ? (
          <ActionBarPrimitive.Edit className={btn} aria-label="编辑重发">
            <Pencil size={14} aria-hidden />
          </ActionBarPrimitive.Edit>
        ) : (
          <ActionBarPrimitive.Copy className={btn} aria-label="复制" title="复制回复">
            <AuiIf condition={(s) => s.message.isCopied}>
              <Check size={14} aria-hidden />
            </AuiIf>
            <AuiIf condition={(s) => !s.message.isCopied}>
              <Copy size={14} aria-hidden />
            </AuiIf>
          </ActionBarPrimitive.Copy>
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
      className="flex flex-wrap items-center gap-x-3 gap-y-1 text-ui-sm tabular-nums text-[var(--cw-dim)]"
    >
      {thinkingSince !== null && (
        <span data-testid="metric-thinking" className="text-[var(--muted-foreground)]">
          <Lightbulb size={12} aria-hidden className="inline-block shrink-0 align-[-1px]" /> 思考中{" "}
          {thinkingSeconds}s
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
      className="mt-1 flex items-center gap-2 px-2 text-ui-sm text-[var(--muted-foreground)]"
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
