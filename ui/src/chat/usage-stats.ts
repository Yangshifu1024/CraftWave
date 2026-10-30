/**
 * 用量统计 store（20 号票，FR-02-13/FR-15-02 Inspector 统计区块数据面）。
 *
 * 数据源 = `stats_token`（§3.3 #52，后端优先 stats_cache、降级直读
 * stats/*.json）；范围切换（今天 / 近 7 天 / 近 30 天 / 全部）即重拉。
 *
 * tokens:update 到达时的本地增量修正（不等轮询）：按事件载荷的 modelId 把
 * 本次增量累进「今日」的 byDay/byModel/total 条目（新当日/新模型即建行），
 * 随后静默重拉做权威校正（防本地漂移；modelId 缺省的老载荷直接重拉）。
 *
 * 另承载 Q37 的旧数据标量回落面：会话恢复拉到的 legacyTotalTokens 存桶，
 * Inspector 合计行回落显示（明细四口径不可知时不伪造拆分）。
 */

import { create } from "zustand";
import { statsToken } from "@/ipc/client";
import type { DayUsage, ModelUsage, StatsRange, StatsTokenResult, UsageTotal } from "@/ipc/types";

/** 统计范围（票面四档）。 */
export type StatsRangeKey = "today" | "7d" | "30d" | "all";

/** 本地日期 → `YYYY-MM-DD`（与后端记账日同口径：本机时区）。 */
export function localDay(date = new Date()): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

/** 本地日期平移 n 天（负数往回；跨月由 Date 归一）。 */
export function shiftLocalDay(base: string, days: number): string {
  const [y, m, d] = base.split("-").map(Number);
  const shifted = new Date(y, m - 1, d + days);
  return localDay(shifted);
}

/** 范围键 → stats_token 的 range 参数（all = 缺省 = 全部保留期）。 */
export function rangeToArgs(key: StatsRangeKey, today: string): { range?: StatsRange } {
  switch (key) {
    case "today":
      return { range: { from: today, to: today } };
    case "7d":
      return { range: { from: shiftLocalDay(today, -6), to: today } };
    case "30d":
      return { range: { from: shiftLocalDay(today, -29), to: today } };
    case "all":
      return {};
  }
}

/** 四口径逐项差分（增量 = 新全量 − 旧全量；异常回退钳 0，不倒扣）。 */
export function usageDelta(prev: UsageTotal, next: UsageTotal): UsageTotal {
  const sub = (a: number, b: number) => (a > b ? a - b : 0);
  return {
    input: sub(next.input, prev.input),
    output: sub(next.output, prev.output),
    cacheRead: sub(next.cacheRead, prev.cacheRead),
    cacheWrite: sub(next.cacheWrite, prev.cacheWrite),
  };
}

/** UsageTotal → 合计（四口径之和，与后端 total 字段同式）。 */
function usageSum(u: UsageTotal): number {
  return u.input + u.output + u.cacheRead + u.cacheWrite;
}

/** store 状态与方法面。 */
export interface UsageStatsStore {
  /** 当前范围（重载回「近 7 天」）。 */
  range: StatsRangeKey;
  /** 当前范围查询结果（null = 尚未加载）。 */
  data: StatsTokenResult | null;
  loading: boolean;
  error: string | null;
  /** Q37 旧数据标量回落桶（sessionId → index.json totalTokens）。 */
  legacyTotals: Record<string, number>;
  /** 切换范围并立即重拉。 */
  setRange: (key: StatsRangeKey) => void;
  /** 按当前范围拉取（面板打开/事件触发）。 */
  load: () => Promise<void>;
  /** tokens:update 到达的本地增量修正（今日条目 + 权威重拉排程）。 */
  applyTokensDelta: (delta: UsageTotal, modelId: string | undefined) => void;
  /** Q37 恢复：记录旧数据标量（合计行回落显示）。 */
  setLegacyTotal: (sessionId: string, total: number) => void;
  /** 会话删除后清回落桶。 */
  forgetSession: (sessionId: string) => void;
}

/** 权威重拉的防抖窗口（增量修正先上屏，随后一次拉取校正）。 */
const RELOAD_DEBOUNCE_MS = 500;

let reloadTimer: ReturnType<typeof setTimeout> | null = null;

/** 把增量累进「今日」条目（byDay 行 / byModel 行 / total；行缺省即建）。 */
function applyDeltaToData(
  data: StatsTokenResult,
  delta: UsageTotal,
  modelId: string | undefined,
  today: string,
): StatsTokenResult {
  const byDay: DayUsage[] = data.byDay.map((d) => ({ ...d }));
  let todayRow = byDay.find((d) => d.day === today);
  if (!todayRow) {
    // 全部/窗口含今日但尚无今日行：按 day 升序插入（今日最大，追加尾部）
    todayRow = {
      day: today,
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      total: 0,
    };
    byDay.push(todayRow);
    byDay.sort((a, b) => a.day.localeCompare(b.day));
  }
  todayRow.input += delta.input;
  todayRow.output += delta.output;
  todayRow.cacheRead += delta.cacheRead;
  todayRow.cacheWrite += delta.cacheWrite;
  todayRow.total += usageSum(delta);

  const byModel: ModelUsage[] = data.byModel.map((m) => ({ ...m }));
  if (modelId) {
    let bucket = byModel.find((m) => m.modelId === modelId);
    if (!bucket) {
      bucket = {
        modelId,
        modelName: modelId,
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        total: 0,
        runs: 0,
      };
      byModel.push(bucket);
      byModel.sort((a, b) => b.total - a.total);
    }
    bucket.input += delta.input;
    bucket.output += delta.output;
    bucket.cacheRead += delta.cacheRead;
    bucket.cacheWrite += delta.cacheWrite;
    bucket.total += usageSum(delta);
  }

  const total = { ...data.total };
  total.input += delta.input;
  total.output += delta.output;
  total.cacheRead += delta.cacheRead;
  total.cacheWrite += delta.cacheWrite;
  total.total += usageSum(delta);

  return { byDay, byModel, total };
}

/** usage-stats zustand store：范围态 + 查询结果 + 增量修正。 */
export const useUsageStatsStore = create<UsageStatsStore>((set, get) => ({
  range: "7d",
  data: null,
  loading: false,
  error: null,
  legacyTotals: {},

  setRange: (key) => {
    set({ range: key });
    void get().load();
  },

  load: async () => {
    const { range } = get();
    set({ loading: true, error: null });
    try {
      const args = rangeToArgs(range, localDay());
      const data = await statsToken(args);
      // 防抖窗口内切了范围则丢弃旧范围结果（stale response 防护）
      if (get().range === range) {
        set({ data, loading: false });
      } else {
        set({ loading: false });
      }
    } catch (err) {
      if (get().range === range) {
        set({ error: err instanceof Error ? err.message : String(err), loading: false });
      } else {
        set({ loading: false });
      }
    }
  },

  applyTokensDelta: (delta, modelId) => {
    if (usageSum(delta) === 0) return;
    const { data } = get();
    const today = localDay();
    if (data) {
      // 本地增量修正（不等轮询）：先上屏
      set({ data: applyDeltaToData(data, delta, modelId, today) });
    }
    // 权威校正排程（防抖合并连续事件；modelId 缺省也重拉兜底归属）
    if (reloadTimer !== null) clearTimeout(reloadTimer);
    reloadTimer = setTimeout(() => {
      reloadTimer = null;
      void get().load();
    }, RELOAD_DEBOUNCE_MS);
  },

  setLegacyTotal: (sessionId, total) =>
    set((s) => ({ legacyTotals: { ...s.legacyTotals, [sessionId]: total } })),

  forgetSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.legacyTotals)) return s;
      const legacyTotals = { ...s.legacyTotals };
      delete legacyTotals[sessionId];
      return { legacyTotals };
    }),
}));

/** 零值快照（模块级常量：防 selector 引用抖动）。 */
const EMPTY_DATA: StatsTokenResult = {
  byDay: [],
  byModel: [],
  total: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0, runs: 0 },
};

/** 响应式取统计结果（未加载返回零值快照，渲染恒有定义）。 */
export function useUsageStats(): StatsTokenResult {
  return useUsageStatsStore((s) => s.data) ?? EMPTY_DATA;
}

/** 缓存命中率（§10.5 公式 = cacheRead/(input+cacheRead)；分母 0 → null）。 */
export function cacheHitRate(u: { input: number; cacheRead: number }): number | null {
  const denom = u.input + u.cacheRead;
  return denom > 0 ? u.cacheRead / denom : null;
}

/** 会话累计恢复（Q37，#59k 读面）：会话打开/切换时调用——四口径经
 *  tokens:update 同款 replace 回填 runMetrics；旧数据标量入回落桶。
 *  失败静默（统计面不阻塞会话加载）。返回 void 供 fire-and-forget。 */
export function restoreSessionUsage(
  sessionId: string,
  fetch: (id: string) => Promise<{ usage: UsageTotal; legacyTotalTokens?: number }>,
  apply: (sessionId: string, usage: UsageTotal) => void,
): void {
  void fetch(sessionId)
    .then((result) => {
      if (result.legacyTotalTokens !== undefined) {
        // 旧数据：明细不可知——标量入桶回落显示，不伪造四口径
        useUsageStatsStore.getState().setLegacyTotal(sessionId, result.legacyTotalTokens);
        return;
      }
      useUsageStatsStore.getState().setLegacyTotal(sessionId, 0);
      if (
        result.usage.input !== 0 ||
        result.usage.output !== 0 ||
        result.usage.cacheRead !== 0 ||
        result.usage.cacheWrite !== 0
      ) {
        apply(sessionId, result.usage);
      }
    })
    .catch(() => {
      // 恢复失败不阻塞会话（下次 run 的 tokens:update 全量校正兜底）
    });
}
