import { beforeEach, describe, expect, it } from "vitest";
import type { SequencedFrame } from "@/ipc/types";
import { contextProgress, useRunMetricsStore } from "./run-metrics";

/**
 * runMetrics store 口径测试（§10.5 + C-9；16 号票改按会话分桶）：
 * - rate = usage 帧 rate（缺省保持上次值）；
 * - turnUsage 运行内累计、runStarted 清零、runEnded 冻结（lastRun 摘要）；
 * - sessionUsage 只增不减（M1 无后台小调用源，C-9 口径天然满足）；
 * - 同 store 多会话桶互不串扰（跨 Tab 并行 run）——全仓该模式的完整断言保留在此。
 */

/** 帧序号自增分配器：给无 seq 的帧载荷编号（对齐真实后端行为）。 */
let seq = 0;
type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
function f(frame: DistributiveOmit<SequencedFrame, "seq">): SequencedFrame {
  return { seq: seq++, ...frame } as SequencedFrame;
}

/** 每条用例前把 store 重置回初始空桶（用例间互不影响）。 */
beforeEach(() => {
  useRunMetricsStore.setState(useRunMetricsStore.getInitialState());
});

describe("run-metrics（§10.5 + C-9）", () => {
  it("ingest 剧本：usage 双口径累计、runStarted 清零/冻结摘要、取消终态、多会话桶互不串扰与 forgetSession、contextProgress 折算", () => {
    const store = useRunMetricsStore.getState();

    // ---- usage 帧累计 turn 与 session 两口径，rate 取帧值（无 rate 帧时保持上次值）----
    store.ingest("s", f({ type: "runStarted", runId: "run-1", model: "gpt-4o-mock" }));
    store.ingest(
      "s",
      f({
        type: "usage",
        usage: { input: 120, output: 48, cacheRead: 0, cacheWrite: 0 },
        rate: 120.5,
      }),
    );
    store.ingest(
      "s",
      f({
        type: "usage",
        usage: { input: 200, output: 30, cacheRead: 64, cacheWrite: 8 },
      }),
    );
    const bucket = useRunMetricsStore.getState().bySession.s;
    expect(bucket?.turnUsage).toEqual({ input: 320, output: 78, cacheRead: 64, cacheWrite: 8 });
    expect(bucket?.sessionUsage).toEqual(bucket?.turnUsage);
    expect(bucket?.rate).toBe(120.5);

    // ---- runEnded 冻结摘要；新 run 起点清零 turnUsage，sessionUsage 保留 ----
    store.ingest(
      "s",
      f({ type: "runEnded", reason: "done", durationMs: 800, toolCount: 1, turn: 2 }),
    );
    const after = useRunMetricsStore.getState().bySession.s;
    expect(after?.lastRun).toEqual({
      runId: "run-1",
      reason: "done",
      durationMs: 800,
      toolCount: 1,
    });
    expect(after?.turnUsage.input).toBe(320);
    store.ingest("s", f({ type: "runStarted", runId: "run-2", model: "m" }));
    const next = useRunMetricsStore.getState().bySession.s;
    expect(next?.turnUsage).toEqual({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
    expect(next?.sessionUsage.input).toBe(320); // 会话累计跨 run 保留
    expect(next?.lastRun).toBeNull();

    // ---- 取消终态照常记录（runMetrics 无异常路径）----
    useRunMetricsStore.setState(useRunMetricsStore.getInitialState());
    useRunMetricsStore.getState().ingest("s", f({ type: "runStarted", runId: "r", model: "m" }));
    useRunMetricsStore
      .getState()
      .ingest(
        "s",
        f({ type: "runEnded", reason: "cancelled", durationMs: 10, toolCount: 2, turn: 1 }),
      );
    expect(useRunMetricsStore.getState().bySession.s?.lastRun?.reason).toBe("cancelled");

    // ---- 多会话桶互不串扰（跨 Tab 并行 run）+ forgetSession 只清自己的桶 ----
    useRunMetricsStore
      .getState()
      .ingest(
        "s1",
        f({ type: "usage", usage: { input: 10, output: 0, cacheRead: 0, cacheWrite: 0 } }),
      );
    useRunMetricsStore
      .getState()
      .ingest(
        "s2",
        f({ type: "usage", usage: { input: 99, output: 0, cacheRead: 0, cacheWrite: 0 } }),
      );
    expect(useRunMetricsStore.getState().bySession.s1?.sessionUsage.input).toBe(10);
    expect(useRunMetricsStore.getState().bySession.s2?.sessionUsage.input).toBe(99);
    useRunMetricsStore.getState().forgetSession("s1");
    expect(useRunMetricsStore.getState().bySession.s1).toBeUndefined();
    expect(useRunMetricsStore.getState().bySession.s2?.sessionUsage.input).toBe(99);

    // ---- contextProgress：sessionUsage 折算 contextWindow 占比并封顶 1 ----
    expect(contextProgress({ input: 60, output: 40, cacheRead: 0, cacheWrite: 0 }, 1000)).toBe(0.1);
    expect(contextProgress({ input: 900, output: 200, cacheRead: 0, cacheWrite: 0 }, 1000)).toBe(1);
    expect(contextProgress({ input: 1, output: 1, cacheRead: 0, cacheWrite: 0 }, 0)).toBe(0);
  });
});
