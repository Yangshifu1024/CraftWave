/**
 * usage-stats store 测试（20 号票）：范围换算、本地增量修正、范围切换重拉、
 * Q37 旧数据标量回落桶。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StatsTokenResult, UsageTotal } from "@/ipc/types";
import {
  cacheHitRate,
  localDay,
  rangeToArgs,
  restoreSessionUsage,
  shiftLocalDay,
  usageDelta,
  useUsageStatsStore,
} from "./usage-stats";

/** statsToken 的可注入替身（vi.spyOn ipc/client 模块）。 */
vi.mock("@/ipc/client", () => ({
  statsToken: vi.fn(),
  sessionUsage: vi.fn(),
}));

import { statsToken } from "@/ipc/client";

const mockStatsToken = vi.mocked(statsToken);

/** 固定两日两模型的结果面（fixture）。 */
function fixture(): StatsTokenResult {
  return {
    byDay: [
      { day: "2026-10-29", input: 100, output: 10, cacheRead: 20, cacheWrite: 5, total: 135 },
      { day: "2026-10-30", input: 50, output: 5, cacheRead: 0, cacheWrite: 0, total: 55 },
    ],
    byModel: [
      {
        modelId: "m1",
        modelName: "GPT-4o",
        input: 150,
        output: 15,
        cacheRead: 20,
        cacheWrite: 5,
        total: 190,
        runs: 2,
      },
    ],
    total: { input: 150, output: 15, cacheRead: 20, cacheWrite: 5, total: 190, runs: 2 },
  };
}

const u = (input: number, output = 0, cacheRead = 0, cacheWrite = 0): UsageTotal => ({
  input,
  output,
  cacheRead,
  cacheWrite,
});

beforeEach(() => {
  useUsageStatsStore.setState({
    range: "7d",
    data: null,
    loading: false,
    error: null,
    legacyTotals: {},
  });
  mockStatsToken.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("usage-stats（20 号票）", () => {
  it("纯函数面：本地日期换算、范围参数、增量修正（回退钳 0）、缓存命中率公式", () => {
    // localDay 与 shiftLocalDay 产生 YYYY-MM-DD 且跨月归一
    expect(localDay(new Date(2026, 9, 30))).toBe("2026-10-30");
    expect(shiftLocalDay("2026-10-30", -6)).toBe("2026-10-24");
    expect(shiftLocalDay("2026-10-30", -29)).toBe("2026-10-01");
    expect(shiftLocalDay("2026-11-01", -1)).toBe("2026-10-31");
    // 范围键 → range 参数（today/7d/30d/all）
    const today = localDay();
    expect(rangeToArgs("today", today)).toEqual({ range: { from: today, to: today } });
    expect(rangeToArgs("7d", today)).toEqual({
      range: { from: shiftLocalDay(today, -6), to: today },
    });
    expect(rangeToArgs("30d", today)).toEqual({
      range: { from: shiftLocalDay(today, -29), to: today },
    });
    expect(rangeToArgs("all", today)).toEqual({}); // 全部保留期 = 缺省
    // usageDelta：新全量 − 旧全量，异常回退（新值 < 旧值）不倒扣
    expect(usageDelta(u(100, 10, 20, 5), u(150, 12, 25, 5))).toEqual(u(50, 2, 5, 0));
    expect(usageDelta(u(100), u(50))).toEqual(u(0, 0, 0, 0));
    // cacheHitRate：§10.5 公式（cacheRead/(input+cacheRead)）
    expect(cacheHitRate({ input: 30, cacheRead: 70 })).toBeCloseTo(0.7);
    expect(cacheHitRate({ input: 0, cacheRead: 0 })).toBeNull();
  });

  it("store 面：load 按范围拉取、setRange 切换重拉、失败记 error、本地增量修正与防抖重拉、restoreSessionUsage 三口径", async () => {
    // ---- load 按当前范围拉取并把结果写入 store（缺省 7d = 本地日期闭区间）----
    mockStatsToken.mockResolvedValue(fixture());
    await useUsageStatsStore.getState().load();
    expect(useUsageStatsStore.getState().data?.total.total).toBe(190);
    const today = localDay();
    expect(mockStatsToken).toHaveBeenLastCalledWith({
      range: { from: shiftLocalDay(today, -6), to: today },
    });

    // ---- setRange 切换即重拉对应范围（all = 缺省无 range）；失败记录 error ----
    await useUsageStatsStore.getState().setRange("today");
    expect(mockStatsToken).toHaveBeenLastCalledWith({ range: { from: today, to: today } });
    expect(useUsageStatsStore.getState().range).toBe("today");
    await useUsageStatsStore.getState().setRange("all");
    expect(mockStatsToken).toHaveBeenLastCalledWith({});
    mockStatsToken.mockRejectedValue(new Error("E_IO"));
    await useUsageStatsStore.getState().load();
    expect(useUsageStatsStore.getState().error).toBe("E_IO");

    // ---- tokens:update 本地增量修正：累进今日行/模型行/total，随后防抖重拉校正 ----
    vi.useFakeTimers();
    mockStatsToken.mockResolvedValue(fixture());
    mockStatsToken.mockClear();
    await useUsageStatsStore.getState().load();
    expect(mockStatsToken).toHaveBeenCalledTimes(1);
    const before = useUsageStatsStore.getState().data!;
    // 本地今日 = fixture 里没有的日期（fixture 固定 10 月）→ 新建今日行
    useUsageStatsStore.getState().applyTokensDelta(u(30, 4, 1, 0), "m2");
    const after = useUsageStatsStore.getState().data!;
    expect(after.total.total).toBe(before.total.total + 35);
    expect(after.byDay.find((d) => d.day === localDay())?.input).toBe(30);
    const m2 = after.byModel.find((m) => m.modelId === "m2");
    expect(m2?.total).toBe(35);
    // 本地增量不计 runs（runs 由权威面回填）；防抖重拉（权威校正）触发
    expect(m2?.runs).toBe(0);
    await vi.advanceTimersByTimeAsync(600);
    expect(mockStatsToken).toHaveBeenCalledTimes(2);
    // 防抖：窗口内多事件只拉一次
    useUsageStatsStore.getState().applyTokensDelta(u(1), "m2");
    useUsageStatsStore.getState().applyTokensDelta(u(2), "m2");
    await vi.advanceTimersByTimeAsync(600);
    expect(mockStatsToken).toHaveBeenCalledTimes(3);
    // modelId 缺省（老载荷）也触发权威重拉
    useUsageStatsStore.getState().applyTokensDelta(u(5), undefined);
    await vi.advanceTimersByTimeAsync(600);
    expect(mockStatsToken).toHaveBeenCalledTimes(4);
    vi.useRealTimers();

    // ---- Q37 旧数据标量回落与恢复：四口径回填 / legacy 入桶 / 失败静默 ----
    const applied: [string, UsageTotal][] = [];
    const apply = (id: string, usage: UsageTotal) => applied.push([id, usage]);
    // 正常明细：usage 回填
    restoreSessionUsage("s1", async () => ({ usage: u(40, 4, 2, 0) }), apply);
    await Promise.resolve();
    await Promise.resolve();
    expect(applied).toEqual([["s1", u(40, 4, 2, 0)]]);
    // 旧数据：legacyTotalTokens 入桶，不伪造四口径
    restoreSessionUsage("s2", async () => ({ usage: u(0), legacyTotalTokens: 777 }), apply);
    await Promise.resolve();
    await Promise.resolve();
    expect(applied.length).toBe(1);
    expect(useUsageStatsStore.getState().legacyTotals.s2).toBe(777);
    // 失败静默
    restoreSessionUsage(
      "s3",
      async () => {
        throw new Error("E_IO");
      },
      apply,
    );
    await Promise.resolve();
    expect(applied.length).toBe(1);
  });
});
