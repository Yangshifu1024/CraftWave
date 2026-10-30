/**
 * 右栏 Inspector（FR-13-01，17 号票；10 号票增子代理页 tab——§10.7 M2 五页
 * 之一）：tab 一「用量」= 计数条三数的明细展开、缓存命中率与上下文进度条；
 * tab 二「子代理」= sub:spawn/done 事件 + subFrame 驱动的列表（角色徽标/
 * subId/状态/步数/用量），运行中自动跟随展开活跃项，抽屉回看 =
 * load_subagent_history 拉全量只读渲染。
 *
 * 数据按活跃会话取（useSessionMetrics 分桶）；上下文窗口取当前激活模型
 * 的 contextWindow（get_config 一次拉取；无激活模型时按文档默认 128000）。
 * 「run 结束自动折叠」（§10.4/FR-01-10）行为保留，UI 开关已移除——按
 * ui-state 持久化值（缺省 true）执行。
 */

import { useEffect, useState } from "react";
import { CompactButton } from "@/chat/CompactionUI";
import { type PlanItem, usePlanStateStore } from "@/chat/plan-state";
import { useSessionMetrics } from "@/chat/run-metrics";
import { useSessionScope } from "@/chat/session-scope";
import { getConfig, loadSubagentHistory } from "@/ipc/client";
import type { MessageEntry, ProviderConfig } from "@/ipc/types";
import {
  markHistoryLoading,
  outcomeLabel,
  type SubagentItem,
  setHistoryLoaded,
  setOpenSub,
  useSubagentsStore,
} from "@/stores/subagents";

/** 技术设计文档 normalize 默认：contextWindow 缺省 128000。 */
const DEFAULT_CONTEXT_WINDOW = 128000;

/** 激活模型的上下文窗口（无激活模型 → 默认值；解析失败不阻塞渲染）。 */
function useContextWindow(): number {
  const [window, setWindow] = useState(DEFAULT_CONTEXT_WINDOW);
  useEffect(() => {
    let cancelled = false;
    void getConfig()
      .then((config) => {
        if (cancelled) return;
        const modelId = config.activeModelId;
        if (modelId === undefined) return;
        for (const provider of config.providers as ProviderConfig[]) {
          const hit = (provider.models ?? []).find((m) => m.id === modelId);
          if (hit?.contextWindow && hit.contextWindow > 0) {
            setWindow(hit.contextWindow);
            return;
          }
        }
      })
      .catch(() => {
        // 纯浏览器/IPC 异常：维持默认窗口值
      });
    return () => {
      cancelled = true;
    };
  }, []);
  return window;
}

/** 空计划快照（模块级常量：selector 缺桶时返回稳定引用，防 React
 *  「getSnapshot should be cached」无限渲染循环——每次 `?? []` 都会新建
 *  数组引用，zustand 的 Object.is 比较恒不等）。 */
const EMPTY_PLAN: PlanItem[] = [];

/** 空子代理列表快照（同上：防 selector 引用抖动）。 */
const EMPTY_SUBS: SubagentItem[] = [];

/** 右栏面板本体：tab（用量 | 子代理）+ 各 tab 内容。宽度受控
 *  （拖宽手柄，min=初始宽 max=半屏）；开合动画走外层宽度过渡，内层定宽
 *  防内容回流；收起后 inert 挡键盘焦点（面板常驻挂载）。 */
export function Inspector({
  width,
  open,
  animate,
}: {
  width: number;
  open: boolean;
  animate: boolean;
}) {
  const sessionId = useSessionScope();
  // tab 态（内存态：重载回「用量」缺省页）
  const [tab, setTab] = useState<"usage" | "subagents">("usage");
  const subs = useSubagentsStore((s) => s.bySession[sessionId] ?? EMPTY_SUBS);
  // 运行中有子代理时自动切到子代理页（盯着它干活；用户可手动切回）
  const runningCount = subs.filter((s) => s.state === "running").length;
  useEffect(() => {
    if (runningCount > 0) setTab("subagents");
  }, [runningCount]);

  return (
    <aside
      data-testid="inspector"
      className={`shrink-0 overflow-hidden ${
        animate ? "transition-[width] duration-200 ease-out" : ""
      }`}
      style={{ width: open ? width : 0 }}
      inert={!open}
    >
      {/* pt-2 + h-12 头行与会话卡头完全同基线；与窗口背景一体化（无底色无边框） */}
      <div className="flex h-full min-h-0 flex-col pt-2" style={{ width }}>
        <div className="flex h-12 shrink-0 items-center gap-1 px-3 text-ui-sm text-[var(--foreground)]">
          <button
            type="button"
            data-testid="inspector-tab-usage"
            className={`rounded-md px-2 py-1 ${
              tab === "usage"
                ? "font-medium text-[var(--foreground)]"
                : "text-[var(--muted-foreground)]"
            }`}
            onClick={() => setTab("usage")}
          >
            用量
          </button>
          <button
            type="button"
            data-testid="inspector-tab-subagents"
            className={`rounded-md px-2 py-1 ${
              tab === "subagents"
                ? "font-medium text-[var(--foreground)]"
                : "text-[var(--muted-foreground)]"
            }`}
            onClick={() => setTab("subagents")}
          >
            子代理{runningCount > 0 ? `（${runningCount} 运行中）` : ""}
          </button>
        </div>
        {tab === "usage" ? (
          <UsagePanel sessionId={sessionId} />
        ) : (
          <SubagentsPanel sessionId={sessionId} />
        )}
      </div>
    </aside>
  );
}

/** 用量 tab（原 Inspector 主体，17 号票形态保留）。 */
function UsagePanel({ sessionId }: { sessionId: string }) {
  const { rate, turnUsage, sessionUsage, lastRun } = useSessionMetrics(sessionId);
  const contextWindow = useContextWindow();
  const planItems = usePlanStateStore((s) => s.bySession[sessionId] ?? EMPTY_PLAN);
  const total = (u: { input: number; output: number }) => u.input + u.output;
  const hitRate =
    sessionUsage.cacheRead > 0
      ? sessionUsage.cacheRead / (sessionUsage.input + sessionUsage.cacheRead)
      : null;
  // 上下文占用：会话累计（输入侧 + 缓存读，§10.5 口径折算）
  const contextUsed = sessionUsage.input + sessionUsage.cacheRead;
  const contextRatio = Math.min(1, contextUsed / contextWindow);
  return (
    <div className="flex flex-col gap-3 overflow-y-auto p-3 text-ui-sm tabular-nums text-[var(--muted-foreground)]">
      {/* 计划（B 组）：plan:update 驱动的当前计划（内存态，重启清空） */}
      {planItems.length > 0 && (
        <section>
          <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">
            计划（{planItems.filter((i) => i.status === "completed").length}/{planItems.length}）
          </h3>
          <ul className="space-y-0.5" data-testid="inspector-plan">
            {planItems.map((item) => (
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
                <span className={item.status === "completed" ? "line-through" : ""}>
                  {item.text}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}
      <section>
        <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">实时</h3>
        <div className="flex justify-between">
          <span>输出速率</span>
          <span data-testid="inspector-rate">
            {rate === null ? "—" : `${rate.toFixed(0)} tok/s`}
          </span>
        </div>
      </section>
      <section>
        <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">本轮用量</h3>
        <MetricRow label="输入" value={turnUsage.input} />
        <MetricRow label="输出" value={turnUsage.output} />
        <MetricRow label="缓存读" value={turnUsage.cacheRead} />
        <MetricRow label="缓存写" value={turnUsage.cacheWrite} />
        <div className="mt-1 flex justify-between text-[var(--foreground)]">
          <span>合计</span>
          <span data-testid="inspector-turn-total">{total(turnUsage)} tok</span>
        </div>
      </section>
      <section>
        <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">会话累计</h3>
        <MetricRow label="输入" value={sessionUsage.input} />
        <MetricRow label="输出" value={sessionUsage.output} />
        <div className="flex justify-between">
          <span>缓存命中率</span>
          <span data-testid="inspector-cache">
            {hitRate === null ? "—" : `${(hitRate * 100).toFixed(0)}%`}
          </span>
        </div>
        <div className="mt-1 flex justify-between text-[var(--foreground)]">
          <span>合计</span>
          <span data-testid="inspector-session-total">{total(sessionUsage)} tok</span>
        </div>
      </section>
      <section>
        <h3 className="mb-1 flex items-center text-ui-sm text-[var(--cw-dim)]">
          上下文进度
          {/* §10.5 落点：进度条旁的手动压缩（FR-03-05，04 号票）；
              运行中/压缩中禁用（后端 E_SESSION_RUNNING 兜底） */}
          <CompactButton />
        </h3>
        <div
          className="relative h-2 w-full overflow-hidden rounded bg-[var(--muted)]"
          data-testid="inspector-context"
        >
          <div className="h-full bg-[var(--primary)]" style={{ width: `${contextRatio * 100}%` }} />
          {/* §10.5：0.6 阈值标线（超过即接近压缩线） */}
          <div
            className="absolute top-0 h-full w-px bg-[var(--destructive)]"
            style={{ left: "60%" }}
          />
        </div>
        <div className="mt-1 flex justify-between">
          <span>{contextUsed} tok</span>
          <span>{contextWindow} 窗口</span>
        </div>
      </section>
      {lastRun && (
        <section>
          <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">最近运行</h3>
          <div className="flex justify-between">
            <span>终态</span>
            <span data-testid="inspector-last-run">
              {lastRun.reason === "done"
                ? `完成 · ${(lastRun.durationMs / 1000).toFixed(1)}s`
                : lastRun.reason === "cancelled"
                  ? "已取消"
                  : "出错"}
              {lastRun.reason === "done" ? ` · ${lastRun.toolCount} 次工具` : ""}
            </span>
          </div>
        </section>
      )}
    </div>
  );
}

/** 终态徽标配色（四象限着色，票面：subDone 徽标按象限着色）。 */
function outcomeColor(state: SubagentItem["state"]): string {
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
      return "text-blue-600 dark:text-blue-400";
  }
}

/** 子代理 tab（10 号票）：列表（角色徽标/subId/状态/步数/用量）+ 抽屉回看。
 *  列表按 spawn 顺序（时间线序）；运行中项高亮；点击行开抽屉拉全量历史
 *  只读渲染。 */
function SubagentsPanel({ sessionId }: { sessionId: string }) {
  const subs = useSubagentsStore((s) => s.bySession[sessionId] ?? EMPTY_SUBS);
  const openSubId = useSubagentsStore((s) => s.openSubId);
  if (subs.length === 0) {
    return (
      <div className="p-3 text-ui-sm text-[var(--muted-foreground)]" data-testid="subagents-empty">
        本会话还没有子代理。主代理可用 subagent 工具委派，或在输入框输入
        <span className="mx-1 rounded bg-[var(--muted)] px-1">$</span>
        点名角色。
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto p-3 text-ui-sm text-[var(--muted-foreground)]">
      <ul className="space-y-1" data-testid="subagents-list">
        {subs.map((sub) => (
          <li key={sub.subId}>
            <button
              type="button"
              data-testid={`subagent-row-${sub.subId}`}
              className={`flex w-full flex-col gap-0.5 rounded-lg border px-2 py-1.5 text-left ${
                openSubId === sub.subId
                  ? "border-[var(--cw-focus)]"
                  : "border-transparent hover:border-[var(--border)]"
              } ${sub.state === "running" ? "bg-[var(--muted)]" : ""}`}
              onClick={() => setOpenSub(openSubId === sub.subId ? null : sub.subId)}
            >
              <span className="flex items-center gap-1.5">
                <span className="rounded bg-[var(--accent)] px-1.5 py-0.5 text-ui-sm font-medium text-[var(--foreground)]">
                  {sub.role}
                </span>
                <span className={`ml-auto ${outcomeColor(sub.state)}`}>
                  {outcomeLabel(sub.state)}
                </span>
              </span>
              <span className="flex items-center gap-2 tabular-nums">
                <span className="truncate text-ui-sm">{sub.subId.slice(0, 8)}</span>
                <span className="ml-auto">第 {sub.turn} 轮</span>
                <span>{sub.usage.input + sub.usage.output} tok</span>
              </span>
            </button>
            {openSubId === sub.subId && <SubDrawer sub={sub} />}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** 抽屉回看（FR-07-06）：load_subagent_history 拉全量只读渲染（角色运行
 *  中的轮次输出 + 汇报）；失败给行内错误。 */
function SubDrawer({ sub }: { sub: SubagentItem }) {
  const history = useSubagentsStore((s) => s.historyBySub[sub.subId]);
  useEffect(() => {
    if (history !== undefined) return;
    markHistoryLoading(sub.subId);
    void loadSubagentHistory({ sessionId: sub.sessionId, subId: sub.subId })
      .then((messages) => setHistoryLoaded(sub.subId, messages, null))
      .catch((err: unknown) => {
        const message =
          typeof err === "object" && err !== null && "message" in err
            ? String((err as { message: unknown }).message)
            : String(err);
        setHistoryLoaded(sub.subId, [], message);
      });
  }, [history, sub.sessionId, sub.subId]);
  return (
    <div
      className="mt-1 rounded-lg border border-[var(--border)] bg-[var(--muted)] p-2"
      data-testid={`subagent-drawer-${sub.subId}`}
    >
      <div className="flex items-center gap-2 pb-1 text-ui-sm text-[var(--cw-dim)]">
        <span>历史回看</span>
        <button
          type="button"
          className="ml-auto text-ui-sm text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
          onClick={() => setOpenSub(null)}
        >
          收起
        </button>
      </div>
      {history === undefined || history.loading ? (
        <p className="text-ui-sm">加载中…</p>
      ) : history.error !== null ? (
        <p className="text-ui-sm text-red-600 dark:text-red-400">{history.error}</p>
      ) : (
        <SubHistoryMessages messages={history.messages as MessageEntry[]} />
      )}
      {sub.report.length > 0 && (
        <div className="mt-2 border-t border-[var(--border)] pt-2">
          <p className="mb-1 text-ui-sm text-[var(--cw-dim)]">&lt;report&gt; 汇报</p>
          <pre className="max-h-48 overflow-y-auto whitespace-pre-wrap text-ui-sm text-[var(--foreground)]">
            {sub.report}
          </pre>
        </div>
      )}
    </div>
  );
}

/** 抽屉消息的只读渲染（WireMessage 最小子集：assistant 文本/工具行）。 */
function SubHistoryMessages({ messages }: { messages: MessageEntry[] }) {
  if (messages.length === 0) {
    return <p className="text-ui-sm">（无历史记录）</p>;
  }
  return (
    <ul className="space-y-2">
      {messages.map((message) => (
        <li key={message.id} className="rounded bg-[var(--card)] p-2">
          <p className="mb-1 text-ui-sm text-[var(--cw-dim)]">
            {message.id.endsWith(":report")
              ? "最终汇报"
              : `轮次 ${message.id.split(":a")[1] ?? ""}`}
          </p>
          {message.parts.map((part) => {
            // part 无独立 id：以 message.id + part 判别类型 + 内容哈希作 key
            //（只读渲染不重排，同类型多 part 以 callId/文本前缀区分）
            const partKey =
              part.type === "toolCall"
                ? `${part.callId}-call`
                : part.type === "toolResult"
                  ? `${part.callId}-result`
                  : `${part.type}-${part.type === "text" ? part.text.slice(0, 16) : ""}`;
            if (part.type === "toolCall") {
              return (
                <p key={partKey} className="text-ui-sm">
                  <span className="rounded bg-[var(--muted)] px-1">{part.name}</span>{" "}
                  <span className="break-all text-[var(--muted-foreground)]">{part.argsJson}</span>
                </p>
              );
            }
            if (part.type === "toolResult") {
              return (
                <p
                  key={partKey}
                  className={`text-ui-sm ${part.ok ? "text-emerald-600 dark:text-emerald-400" : "text-red-600 dark:text-red-400"}`}
                >
                  ↳ {part.summary}
                </p>
              );
            }
            if (part.type === "text" && part.text.length > 0) {
              return (
                <pre
                  key={partKey}
                  className="max-h-40 overflow-y-auto whitespace-pre-wrap text-ui-sm text-[var(--foreground)]"
                >
                  {part.text}
                </pre>
              );
            }
            return null;
          })}
        </li>
      ))}
    </ul>
  );
}

/** 单行指标（标签 + 数值 + tok 单位）。 */
function MetricRow({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex justify-between">
      <span>{label}</span>
      <span>{value} tok</span>
    </div>
  );
}
