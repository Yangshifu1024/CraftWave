/**
 * 配额快照 store（Inspector 配额卡数据面）。
 *
 * 数据源 = `quota_snapshots`（后端并发实时拉取 + quota.json 离线
 * 兜底面）。刷新策略在 QuotaPanel 编排：配额页可见时每
 * 5 分钟周期 + 手动刷新按钮 + 切换 provider 即时；不可见即停（visible 门
 * 控，条件渲染卸载即清理定时器）。
 *
 * 本模块另承载「当前会话所用 provider」的解析（纯函数，测试锚点）：
 * 会话覆盖模型（session-model store，Ctrl+L 选择）> 全局 activeModelId →
 * 按 models 归属供应商；与后端的模型解析优先级一致。
 */

import { create } from "zustand";
import { quotaSnapshots } from "@/ipc/client";
import type { AppConfig, QuotaEntry, QuotaSnapshot } from "@/ipc/types";

/** store 状态与方法面。 */
export interface QuotaStore {
  /** 最近一次快照（null = 尚未加载；加载失败保留旧值做兜底显示）。 */
  rows: QuotaSnapshot[] | null;
  loading: boolean;
  /** 拉取快照（周期/手动/切 provider 共用同一入口）。失败静默
   *  （「查询失败静默降级」：不弹错，行面由后端六态表达）。 */
  load: () => Promise<void>;
}

export const useQuotaStore = create<QuotaStore>((set) => ({
  rows: null,
  loading: false,

  load: async () => {
    if (useQuotaStore.getState().loading) return;
    set({ loading: true });
    try {
      const rows = await quotaSnapshots();
      set({ rows, loading: false });
    } catch {
      // 静默降级：保留旧 rows（离线兜底），仅清 loading
      set({ loading: false });
    }
  },
}));

/** 配额自动刷新周期（配额页可见时每 5 分钟）。 */
export const QUOTA_REFRESH_INTERVAL_MS = 5 * 60 * 1000;

/**
 * 解析「当前会话所用 provider」（首屏口径）：会话覆盖模型 > 全局
 * activeModelId → 找 models 里含该 modelId 的供应商；都缺省/悬空 → None。
 */
export function providerOfModel(
  config: Pick<AppConfig, "providers" | "activeModelId">,
  modelId: string | undefined,
): string | undefined {
  const wanted = modelId ?? config.activeModelId;
  if (wanted === undefined) return undefined;
  for (const provider of config.providers) {
    if (provider.models.some((model) => model.id === wanted)) return provider.id;
  }
  return undefined;
}

/** 百分比行的剩余值（供主指标展示：取最紧张的窗口，CodeWave 择劣口径）。 */
export function worstRemainingPercent(entries: QuotaEntry[]): number | null {
  const percents = entries
    .map((entry) => entry.remainingPercent)
    .filter((value): value is number => value !== undefined);
  if (percents.length === 0) return null;
  return Math.min(...percents);
}
