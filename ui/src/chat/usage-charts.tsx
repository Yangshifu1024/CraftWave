/**
 * 用量统计图表：手写轻量 SVG 组件（定案——
 * 无图表依赖，两种图数据 ≤90 点，体积零成本；评审若要求组件库，
 * recharts 体积代价另行记录）。
 *
 * - DailyBars：按日柱状图（近 30 天，柱高 = 当日四口径合计，hover title
 *   给日期与数字明细）；
 * - ModelBars：按模型排行条形（模型名 + 占比条 + 四口径/主 run 数字，
 *   按 total 降序，最长条 = 占比基准 100%）。
 */

import { useT } from "@/i18n/core";
import type { DayUsage, ModelUsage } from "@/ipc/types";

/** 数值的紧凑显示（1.2k / 34k / 1.2M——面板窄栏友好）。 */
export function compactTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

/** 按日柱状图：≤30 根柱，等宽排布，柱高按 max 归一（min 高度 2px 保证
 *  零值日可见）。SVG viewBox 固定逻辑坐标，CSS 宽度自适应。 */
export function DailyBars({ days, maxBars = 30 }: { days: DayUsage[]; maxBars?: number }) {
  const t = useT();
  // 只画最近 maxBars 天（按 day 升序进、尾部为最新）
  const window = days.slice(-maxBars);
  if (window.length === 0) {
    return (
      <p className="text-ui-sm" data-testid="stats-daily-empty">
        {t("inspector.chartNoData")}
      </p>
    );
  }
  const max = Math.max(...window.map((d) => d.total), 1);
  const barW = 100 / window.length;
  return (
    <svg
      data-testid="stats-daily-bars"
      viewBox="0 0 100 34"
      preserveAspectRatio="none"
      className="h-20 w-full"
      role="img"
      aria-label={t("inspector.dailyBarsAria")}
    >
      {/* 柱体：x 等分，高度按当日合计/max 归一（基线 y=32，顶部留日期层） */}
      {window.map((d) => {
        const h = Math.max((d.total / max) * 30, d.total > 0 ? 2 : 0.5);
        const label = t("inspector.dayTooltip", {
          day: d.day,
          total: compactTokens(d.total),
          input: compactTokens(d.input),
          output: compactTokens(d.output),
          cacheRead: compactTokens(d.cacheRead),
        });
        return (
          <rect
            key={d.day}
            x={window.indexOf(d) * barW + barW * 0.12}
            y={32 - h}
            width={barW * 0.76}
            height={h}
            className="fill-[var(--primary)] opacity-80"
          >
            <title>{label}</title>
          </rect>
        );
      })}
      {/* 基线 */}
      <line x1="0" y1="32" x2="100" y2="32" className="stroke-[var(--border)]" strokeWidth="0.4" />
    </svg>
  );
}

/** 按模型排行条形：total 降序（调用方保证），占比 = 该模型 total / 最大值。 */
export function ModelBars({ models }: { models: ModelUsage[] }) {
  const t = useT();
  if (models.length === 0) {
    return (
      <p className="text-ui-sm" data-testid="stats-models-empty">
        {t("inspector.chartNoModelData")}
      </p>
    );
  }
  const max = Math.max(...models.map((m) => m.total), 1);
  return (
    <ul className="space-y-1.5" data-testid="stats-model-bars">
      {models.map((m) => {
        const ratio = m.total / max;
        const hit =
          m.input + m.cacheRead > 0
            ? Math.round((m.cacheRead / (m.input + m.cacheRead)) * 100)
            : null;
        return (
          <li
            key={m.modelId}
            title={t("inspector.modelTooltip", { id: m.modelId, name: m.modelName })}
          >
            <div className="flex items-baseline justify-between gap-2">
              <span className="truncate text-[var(--foreground)]" title={m.modelName}>
                {m.modelName || m.modelId}
              </span>
              <span className="shrink-0 tabular-nums">
                {compactTokens(m.total)} tok
                {m.runs > 0 ? t("inspector.modelRuns", { count: m.runs }) : ""}
                {hit !== null ? t("inspector.modelCache", { rate: hit }) : ""}
              </span>
            </div>
            {/* 占比条：CSS 宽度即可（柱状/条形各自选型），色带同柱状图 */}
            <div className="mt-0.5 h-1.5 w-full overflow-hidden rounded bg-[var(--muted)]">
              <div
                className="h-full bg-[var(--primary)] opacity-80"
                style={{ width: `${Math.max(ratio * 100, 1)}%` }}
              />
            </div>
          </li>
        );
      })}
    </ul>
  );
}
