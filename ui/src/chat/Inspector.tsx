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
import { ArtifactsPanel } from "@/chat/ArtifactsPanel";
import { ChangesPanel } from "@/chat/ChangesPanel";
import { CompactButton } from "@/chat/CompactionUI";
import { useChangesStore } from "@/chat/changes-store";
import { ExternalGrantsPanel } from "@/chat/ExternalGrants";
import { EMPTY_PLAN_STATE, type PlanItem, usePlanStateStore } from "@/chat/plan-state";
import { QuotaPanel } from "@/chat/QuotaPanel";
import { useRunMetricsStore, useSessionMetrics } from "@/chat/run-metrics";
import { useSessionScope } from "@/chat/session-scope";
import { compactTokens, DailyBars, ModelBars } from "@/chat/usage-charts";
import {
  cacheHitRate,
  restoreSessionUsage,
  type StatsRangeKey,
  useUsageStats,
  useUsageStatsStore,
} from "@/chat/usage-stats";
import { useT } from "@/i18n/core";
import { approvePlan, getConfig, gitStatus, loadSubagentHistory, sessionUsage } from "@/ipc/client";
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

/** 空子代理列表快照（同上：防 selector 引用抖动）。 */
const EMPTY_SUBS: SubagentItem[] = [];

/** 右栏面板本体：tab（用量 | 配额 | 子代理）+ 各 tab 内容。宽度受控
 *  （拖宽手柄，min=初始宽 max=半屏）；开合动画走外层宽度过渡，内层定宽
 *  防内容回流；收起后 inert 挡键盘焦点（面板常驻挂载）。 */
export function Inspector({
  width,
  open,
  animate,
  onOpenSettings,
}: {
  width: number;
  open: boolean;
  animate: boolean;
  /** 「去设置」跳转（21 号票配额卡 no_key 行）：打开设置页供应商页。 */
  onOpenSettings: () => void;
}) {
  const t = useT();
  const sessionId = useSessionScope();
  // tab 态（内存态：重载回「用量」缺省页）；「变更」tab（26 号票）= 打开
  // 宽弹窗的入口（§10.7 变更面板为 Dialog 形态，tab 只承载入口按钮）；
  // 「产物」tab（28 号票）= 会话产物登记页（write/edit 自动登记 + 一键打开）。
  const [tab, setTab] = useState<
    "usage" | "artifacts" | "quota" | "subagents" | "grants" | "changes"
  >("usage");
  const subs = useSubagentsStore((s) => s.bySession[sessionId] ?? EMPTY_SUBS);
  // 运行中有子代理时自动切到子代理页（盯着它干活；用户可手动切回）
  const runningCount = subs.filter((s) => s.state === "running").length;
  useEffect(() => {
    if (runningCount > 0) setTab("subagents");
  }, [runningCount]);
  // 会话累计恢复（Q37，20 号票）：会话打开/切换时拉 #59k，四口径经
  // tokens:update 同款 replace 回填 runMetrics（旧数据标量入回落桶）。
  // Inspector 面板常驻挂载（open 只控宽度），本副作用即会话级恢复点。
  useEffect(() => {
    restoreSessionUsage(sessionId, sessionUsage, (id, usage) => {
      useRunMetricsStore.getState().applyTokensUpdate(id, usage);
    });
  }, [sessionId]);

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
            {t("inspector.tabUsage")}
          </button>
          <button
            type="button"
            data-testid="inspector-tab-artifacts"
            className={`rounded-md px-2 py-1 ${
              tab === "artifacts"
                ? "font-medium text-[var(--foreground)]"
                : "text-[var(--muted-foreground)]"
            }`}
            onClick={() => setTab("artifacts")}
          >
            {t("inspector.tabArtifacts")}
          </button>
          <button
            type="button"
            data-testid="inspector-tab-quota"
            className={`rounded-md px-2 py-1 ${
              tab === "quota"
                ? "font-medium text-[var(--foreground)]"
                : "text-[var(--muted-foreground)]"
            }`}
            onClick={() => setTab("quota")}
          >
            {t("inspector.tabQuota")}
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
            {t("inspector.tabSubagents")}
            {runningCount > 0 && t("inspector.tabSubagentsRunning", { count: runningCount })}
          </button>
          <button
            type="button"
            data-testid="inspector-tab-grants"
            className={`rounded-md px-2 py-1 ${
              tab === "grants"
                ? "font-medium text-[var(--foreground)]"
                : "text-[var(--muted-foreground)]"
            }`}
            onClick={() => setTab("grants")}
          >
            {t("inspector.tabGrants")}
          </button>
          <button
            type="button"
            data-testid="inspector-tab-changes"
            className={`rounded-md px-2 py-1 ${
              tab === "changes"
                ? "font-medium text-[var(--foreground)]"
                : "text-[var(--muted-foreground)]"
            }`}
            onClick={() => setTab("changes")}
          >
            {t("inspector.tabChanges")}
          </button>
        </div>
        {tab === "usage" ? (
          <UsagePanel sessionId={sessionId} />
        ) : tab === "artifacts" ? (
          <ArtifactsPanel sessionId={sessionId} />
        ) : tab === "quota" ? (
          <QuotaPanel sessionId={sessionId} onOpenSettings={onOpenSettings} />
        ) : tab === "grants" ? (
          <ExternalGrantsPanel />
        ) : tab === "changes" ? (
          <ChangesEntryPanel sessionId={sessionId} />
        ) : (
          <SubagentsPanel sessionId={sessionId} />
        )}
        {/* 变更面板（26 号票，FR-11-02）：宽弹窗单例挂载——tab 入口与
            openChangesPanel 联动（28 号票产物页）都经 changes-store 驱动 */}
        <ChangesPanel />
      </div>
    </aside>
  );
}

/** 「变更」tab 内容（26 号票）：打开宽弹窗的入口按钮 + 当前变更文件数
 *  摘要（轻量拉取 #37 一次，不随 tab 切换轮询）。 */
function ChangesEntryPanel({ sessionId }: { sessionId: string }) {
  const t = useT();
  const openPanel = useChangesStore((s) => s.openPanel);
  const open = useChangesStore((s) => s.open);
  const [fileCount, setFileCount] = useState<number | null>(null);
  const [isRepo, setIsRepo] = useState(true);
  useEffect(() => {
    let cancelled = false;
    void gitStatus({ sessionId })
      .then((result) => {
        if (cancelled) return;
        setIsRepo(result.isRepo);
        setFileCount(result.files.length);
      })
      .catch(() => {
        if (!cancelled) setFileCount(null);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);
  return (
    <div className="p-3 text-ui-sm text-[var(--muted-foreground)]">
      {!isRepo ? (
        <p data-testid="changes-entry-empty">{t("inspector.changesNotRepo")}</p>
      ) : (
        <div className="space-y-2">
          <p>
            {fileCount === null
              ? t("inspector.changesLoading")
              : fileCount === 0
                ? t("inspector.changesClean")
                : t("inspector.changesCount", { count: fileCount })}
          </p>
          <button
            type="button"
            data-testid="changes-open-button"
            onClick={() => openPanel(sessionId)}
            disabled={open}
            className="rounded-full border border-[var(--border)] px-3 py-1 text-ui-sm text-[var(--foreground)] hover:border-[var(--primary)] disabled:opacity-40"
          >
            {t("inspector.changesOpenPanel")}
          </button>
        </div>
      )}
    </div>
  );
}

/** 统计区块（20 号票，FR-15-02）：范围切换（今天/近 7 天/近 30 天/全部）
 *  + 按日柱状图（近 30 天）+ 按模型排行（四口径/占比条）；数据源
 *  stats_token，tokens:update 到达时本地增量修正今日（event-bridge 接线）。
 *  范围缓存命中率 = cacheRead/(input+cacheRead)（§10.5 公式）。 */
const RANGE_LABELS: { key: StatsRangeKey; labelKey: string }[] = [
  { key: "today", labelKey: "inspector.rangeToday" },
  { key: "7d", labelKey: "inspector.range7d" },
  { key: "30d", labelKey: "inspector.range30d" },
  { key: "all", labelKey: "inspector.rangeAll" },
];

function UsageStatsSection() {
  const t = useT();
  const stats = useUsageStats();
  const range = useUsageStatsStore((s) => s.range);
  const setRange = useUsageStatsStore((s) => s.setRange);
  const error = useUsageStatsStore((s) => s.error);
  // 打开区块（挂载）即拉取；tokens:update 的增量修正与权威重拉在事件桥
  useEffect(() => {
    void useUsageStatsStore.getState().load();
  }, []);
  // 模型排行按 total 降序（重排不动后端数据面）
  const byModel = [...stats.byModel].sort((a, b) => b.total - a.total);
  const hit = cacheHitRate(stats.total);
  return (
    <section data-testid="inspector-stats">
      <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">{t("inspector.statsTitle")}</h3>
      {/* 范围切换：四档联动重拉 */}
      <div className="mb-2 flex gap-1" role="tablist" aria-label={t("inspector.statsRangeAria")}>
        {RANGE_LABELS.map(({ key, labelKey }) => (
          <button
            key={key}
            type="button"
            data-testid={`stats-range-${key}`}
            className={`rounded px-1.5 py-0.5 text-ui-sm ${
              range === key
                ? "bg-[var(--accent)] font-medium text-[var(--foreground)]"
                : "text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
            }`}
            onClick={() => setRange(key)}
          >
            {t(labelKey)}
          </button>
        ))}
      </div>
      {error !== null ? (
        <p className="text-ui-sm text-red-600 dark:text-red-400" data-testid="stats-error">
          {t("inspector.statsLoadFailed", { error })}
        </p>
      ) : (
        <>
          <DailyBars days={stats.byDay} />
          <div className="mt-1 mb-2 flex justify-between">
            <span data-testid="stats-range-total">{compactTokens(stats.total.total)} tok</span>
            <span data-testid="stats-range-hit">
              {t("inspector.cacheHitRate", {
                rate: hit === null ? "—" : `${Math.round(hit * 100)}%`,
              })}
            </span>
          </div>
          <ModelBars models={byModel} />
        </>
      )}
    </section>
  );
}

/** 用量 tab（原 Inspector 主体，17 号票形态保留；20 号票增统计区块与
 *  Q37 恢复回落显示）。 */
function UsagePanel({ sessionId }: { sessionId: string }) {
  const t = useT();
  const { rate, turnUsage, sessionUsage, lastRun } = useSessionMetrics(sessionId);
  const contextWindow = useContextWindow();
  const plan = usePlanStateStore((s) => s.bySession[sessionId] ?? EMPTY_PLAN_STATE);
  const planItems: PlanItem[] = plan.items;
  const legacy = useUsageStatsStore((s) => s.legacyTotals[sessionId]);
  const total = (u: { input: number; output: number }) => u.input + u.output;
  const hitRate =
    sessionUsage.cacheRead > 0
      ? sessionUsage.cacheRead / (sessionUsage.input + sessionUsage.cacheRead)
      : null;
  // 上下文占用：会话累计（输入侧 + 缓存读，§10.5 口径折算）
  const contextUsed = sessionUsage.input + sessionUsage.cacheRead;
  const contextRatio = Math.min(1, contextUsed / contextWindow);
  // Q37 旧数据回落：明细四口径全零且 index.json 有历史标量 → 合计行回落
  const legacyTotal =
    legacy !== undefined && total(sessionUsage) === 0 && legacy > 0 ? legacy : null;
  return (
    <div className="flex flex-col gap-3 overflow-y-auto p-3 text-ui-sm tabular-nums text-[var(--muted-foreground)]">
      {/* 计划（B 组）：plan:update 驱动的当前计划（内存态，重启清空）。
          25 号票：批准态标记（已批准 + 基线条目数）、写范围声明与批准按钮
          （§8.6 批准基线冻结的用户面——批准动作经 #26c approve_plan）。 */}
      {planItems.length > 0 && (
        <section>
          <h3 className="mb-1 flex items-center gap-2 text-ui-sm text-[var(--cw-dim)]">
            <span>
              {t("inspector.planTitle", {
                done: planItems.filter((i) => i.status === "completed").length,
                total: planItems.length,
              })}
            </span>
            {plan.approved ? (
              <span
                data-testid="inspector-plan-baseline"
                className="rounded bg-[var(--primary)]/10 px-1.5 py-0.5 text-[10px] text-[var(--primary)]"
              >
                {t("inspector.planApprovedBadge", { count: plan.baselinePaths.length })}
              </span>
            ) : (
              <button
                type="button"
                data-testid="inspector-plan-approve"
                onClick={() => void approvePlan(sessionId).catch(() => {})}
                className="cursor-pointer rounded border border-[var(--border)] px-1.5 py-0.5 text-[10px] text-[var(--foreground)] hover:border-[var(--primary)]"
              >
                {t("inspector.approvePlan")}
              </button>
            )}
          </h3>
          {plan.paths.length > 0 && (
            <p data-testid="inspector-plan-paths" className="mb-1 break-all font-mono text-[10px]">
              {t("inspector.planScope", { paths: plan.paths.join("、") })}
            </p>
          )}
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
        <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">{t("inspector.sectionLive")}</h3>
        <div className="flex justify-between">
          <span>{t("inspector.outputRate")}</span>
          <span data-testid="inspector-rate">
            {rate === null ? "—" : `${rate.toFixed(0)} tok/s`}
          </span>
        </div>
      </section>
      <section>
        <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">{t("inspector.sectionTurn")}</h3>
        <MetricRow label={t("inspector.metricInput")} value={turnUsage.input} />
        <MetricRow label={t("inspector.metricOutput")} value={turnUsage.output} />
        <MetricRow label={t("inspector.metricCacheRead")} value={turnUsage.cacheRead} />
        <MetricRow label={t("inspector.metricCacheWrite")} value={turnUsage.cacheWrite} />
        <div className="mt-1 flex justify-between text-[var(--foreground)]">
          <span>{t("inspector.total")}</span>
          <span data-testid="inspector-turn-total">{total(turnUsage)} tok</span>
        </div>
      </section>
      <section>
        <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">{t("inspector.sectionSession")}</h3>
        <MetricRow label={t("inspector.metricInput")} value={sessionUsage.input} />
        <MetricRow label={t("inspector.metricOutput")} value={sessionUsage.output} />
        <div className="flex justify-between">
          <span>{t("inspector.cacheHitRateLabel")}</span>
          <span data-testid="inspector-cache">
            {hitRate === null ? "—" : `${(hitRate * 100).toFixed(0)}%`}
          </span>
        </div>
        <div className="mt-1 flex justify-between text-[var(--foreground)]">
          <span>{t("inspector.total")}</span>
          <span data-testid="inspector-session-total">
            {legacyTotal !== null
              ? t("inspector.legacyTotalNote", { total: legacyTotal })
              : `${total(sessionUsage)} tok`}
          </span>
        </div>
      </section>
      <UsageStatsSection />
      <section>
        <h3 className="mb-1 flex items-center text-ui-sm text-[var(--cw-dim)]">
          {t("inspector.sectionContext")}
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
          <span>{t("inspector.contextWindow", { count: contextWindow })}</span>
        </div>
      </section>
      {lastRun && (
        <section>
          <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">{t("inspector.sectionLastRun")}</h3>
          <div className="flex justify-between">
            <span>{t("inspector.lastRunState")}</span>
            <span data-testid="inspector-last-run">
              {lastRun.reason === "done"
                ? t("inspector.runDone", { duration: (lastRun.durationMs / 1000).toFixed(1) })
                : lastRun.reason === "cancelled"
                  ? t("inspector.runCancelled")
                  : t("inspector.runError")}
              {lastRun.reason === "done"
                ? t("inspector.runToolCount", { count: lastRun.toolCount })
                : ""}
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
  const t = useT();
  const subs = useSubagentsStore((s) => s.bySession[sessionId] ?? EMPTY_SUBS);
  const openSubId = useSubagentsStore((s) => s.openSubId);
  if (subs.length === 0) {
    return (
      <div className="p-3 text-ui-sm text-[var(--muted-foreground)]" data-testid="subagents-empty">
        {t("inspector.subagentsEmptyBefore")}
        <span className="mx-1 rounded bg-[var(--muted)] px-1">$</span>
        {t("inspector.subagentsEmptyAfter")}
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
                <span className="ml-auto">{t("inspector.subagentTurnN", { turn: sub.turn })}</span>
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
  const t = useT();
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
        <span>{t("inspector.subDrawerTitle")}</span>
        <button
          type="button"
          className="ml-auto text-ui-sm text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
          onClick={() => setOpenSub(null)}
        >
          {t("inspector.subDrawerCollapse")}
        </button>
      </div>
      {history === undefined || history.loading ? (
        <p className="text-ui-sm">{t("inspector.subDrawerLoading")}</p>
      ) : history.error !== null ? (
        <p className="text-ui-sm text-red-600 dark:text-red-400">{history.error}</p>
      ) : (
        <SubHistoryMessages messages={history.messages as MessageEntry[]} />
      )}
      {sub.report.length > 0 && (
        <div className="mt-2 border-t border-[var(--border)] pt-2">
          <p className="mb-1 text-ui-sm text-[var(--cw-dim)]">{t("inspector.subReportTitle")}</p>
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
  const t = useT();
  if (messages.length === 0) {
    return <p className="text-ui-sm">{t("inspector.subHistoryEmpty")}</p>;
  }
  return (
    <ul className="space-y-2">
      {messages.map((message) => (
        <li key={message.id} className="rounded bg-[var(--card)] p-2">
          <p className="mb-1 text-ui-sm text-[var(--cw-dim)]">
            {message.id.endsWith(":report")
              ? t("inspector.subFinalReport")
              : t("inspector.subTurnN", { turn: message.id.split(":a")[1] ?? "" })}
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
