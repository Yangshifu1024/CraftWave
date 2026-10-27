import { beforeEach, describe, expect, it } from "vitest";
import type { ChatFrame } from "@/ipc/types";
import { contextProgress, useRunMetricsStore } from "./run-metrics";

/**
 * runMetrics store 口径测试（§10.5 + C-9）：
 * - rate = usage 帧 rate（缺省保持上次值）；
 * - turnUsage 运行内累计、runStarted 清零、runEnded 冻结（lastRun 摘要）；
 * - sessionUsage 只增不减（M1 无后台小调用源，C-9 口径天然满足）。
 */

let seq = 0;
type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
function f(frame: DistributiveOmit<ChatFrame, "seq">): ChatFrame {
  return { seq: seq++, ...frame } as ChatFrame;
}

beforeEach(() => {
  useRunMetricsStore.setState({
    rate: null,
    turnUsage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    sessionUsage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    lastRun: null,
  });
});

describe("runMetrics ingest", () => {
  it("usage 帧累计 turn 与 session 两口径，rate 取帧值", () => {
    const store = useRunMetricsStore.getState();
    store.ingest(f({ type: "runStarted", runId: "run-1", model: "gpt-4o-mock" }));
    store.ingest(
      f({
        type: "usage",
        usage: { input: 120, output: 48, cacheRead: 0, cacheWrite: 0 },
        rate: 120.5,
      }),
    );
    store.ingest(
      f({
        type: "usage",
        usage: { input: 200, output: 30, cacheRead: 64, cacheWrite: 8 },
      }),
    );
    const { turnUsage, sessionUsage, rate } = useRunMetricsStore.getState();
    expect(turnUsage).toEqual({ input: 320, output: 78, cacheRead: 64, cacheWrite: 8 });
    expect(sessionUsage).toEqual(turnUsage);
    expect(rate).toBe(120.5); // 无 rate 帧时保持上次值
  });

  it("新 run 起点清零 turnUsage，sessionUsage 保留；runEnded 冻结摘要", () => {
    const store = useRunMetricsStore.getState();
    store.ingest(f({ type: "runStarted", runId: "run-1", model: "m" }));
    store.ingest(
      f({ type: "usage", usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0 } }),
    );
    store.ingest(f({ type: "runEnded", reason: "done", durationMs: 800, toolCount: 1, turn: 2 }));
    const after = useRunMetricsStore.getState();
    expect(after.lastRun).toEqual({ reason: "done", durationMs: 800, toolCount: 1 });
    expect(after.turnUsage.input).toBe(10);

    store.ingest(f({ type: "runStarted", runId: "run-2", model: "m" }));
    const next = useRunMetricsStore.getState();
    expect(next.turnUsage).toEqual({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
    expect(next.sessionUsage.input).toBe(10); // 会话累计跨 run 保留
    expect(next.lastRun).toBeNull();
  });

  it("取消终态与错误终态照常记录（runMetrics 无异常路径）", () => {
    const store = useRunMetricsStore.getState();
    store.ingest(f({ type: "runStarted", runId: "r", model: "m" }));
    store.ingest(
      f({ type: "runEnded", reason: "cancelled", durationMs: 10, toolCount: 2, turn: 1 }),
    );
    expect(useRunMetricsStore.getState().lastRun?.reason).toBe("cancelled");
  });
});

describe("contextProgress", () => {
  it("sessionUsage 折算 contextWindow 占比并封顶 1", () => {
    expect(contextProgress({ input: 60, output: 40, cacheRead: 0, cacheWrite: 0 }, 1000)).toBe(0.1);
    expect(contextProgress({ input: 900, output: 200, cacheRead: 0, cacheWrite: 0 }, 1000)).toBe(1);
    expect(contextProgress({ input: 1, output: 1, cacheRead: 0, cacheWrite: 0 }, 0)).toBe(0);
  });
});
