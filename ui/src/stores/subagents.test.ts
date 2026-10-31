/**
 * 子代理 store 测试（10 号票）：事件六键与帧面的双通道分发（幂等合并）、
 * 四象限收口、subFrame 的 turn 推进、抽屉开合与历史缓存。
 */

import { beforeEach, describe, expect, it } from "vitest";
import {
  applySubagentFrame,
  markHistoryLoading,
  outcomeLabel,
  setHistoryLoaded,
  setOpenSub,
  subDone,
  subError,
  subReport,
  subSpawn,
  subStep,
  subUsage,
  useSubagentsStore,
} from "./subagents";

/** 读某会话的子代理列表。 */
const list = (sessionId: string) => useSubagentsStore.getState().bySession[sessionId] ?? [];

beforeEach(() => {
  useSubagentsStore.setState({ bySession: {}, openSubId: null, historyBySub: {} });
});

describe("subagents store", () => {
  it("事件分发剧本（sub:* 六键）：spawn 建行、step 推进只进不退、usage 累计、report 暂存、done 四象限收口", () => {
    // sub:spawn 建行（角色/父 run/running 态），sub:step 推进轮次
    subSpawn({ sessionId: "s1", runId: "r1", subId: "sub-1", role: "explore" });
    expect(list("s1")).toHaveLength(1);
    expect(list("s1")[0]!.role).toBe("explore");
    expect(list("s1")[0]!.state).toBe("running");
    expect(list("s1")[0]!.turn).toBe(0);
    subStep({ sessionId: "s1", subId: "sub-1", turn: 3 });
    expect(list("s1")[0]!.turn).toBe(3);
    // 轮次只进不退（事件与帧乱序到达时保留最大值）
    subStep({ sessionId: "s1", subId: "sub-1", turn: 2 });
    expect(list("s1")[0]!.turn).toBe(3);

    // sub:usage 累计子代理自身消耗（Q40：不进 tokens:update 面）
    subUsage({
      sessionId: "s1",
      subId: "sub-1",
      usage: { input: 10, output: 4, cacheRead: 2, cacheWrite: 0 },
    });
    expect(list("s1")[0]!.usage).toEqual({ input: 10, output: 4, cacheRead: 2, cacheWrite: 0 });

    // sub:report 暂存汇报全文（state 仍 running）
    subSpawn({ sessionId: "s1", runId: "r1", subId: "sub-2", role: "reviewer" });
    subReport({ sessionId: "s1", subId: "sub-2", report: "<report>结论</report>" });
    expect(list("s1")[1]!.report).toBe("<report>结论</report>");
    expect(list("s1")[1]!.state).toBe("running");

    // sub:done 四象限收口（report + partial/cancelled/no_report 循环）
    subDone({ sessionId: "s1", subId: "sub-2", outcome: "report" });
    expect(list("s1")[1]!.state).toBe("report");
    for (const [i, outcome] of ["partial", "cancelled", "no_report"].entries()) {
      const id = `sub-x-${i}`;
      subSpawn({ sessionId: "s1", runId: "r1", subId: id, role: "tester" });
      subDone({ sessionId: "s1", subId: id, outcome });
      expect(list("s1").find((row) => row.subId === id)!.state).toBe(outcome);
    }
    // 未知 outcome 归 no_report（防御收口）
    subSpawn({ sessionId: "s1", runId: "r1", subId: "sub-w", role: "explore" });
    subDone({ sessionId: "s1", subId: "sub-w", outcome: "weird" });
    expect(list("s1").find((row) => row.subId === "sub-w")!.state).toBe("no_report");

    // sub:error 带错误说明（code: message 形态）
    subSpawn({ sessionId: "s2", runId: "r2", subId: "sub-e", role: "planner" });
    subError({ sessionId: "s2", subId: "sub-e", code: "E_INTERNAL", message: "boom" });
    expect(list("s2")[0]!.state).toBe("no_report");
    expect(list("s2")[0]!.error).toBe("E_INTERNAL: boom");

    // outcomeLabel 四象限文案（与收口态一一对应，Inspector/工具卡共用）
    expect(outcomeLabel("running")).toBe("运行中");
    expect(outcomeLabel("report")).toBe("已完成");
    expect(outcomeLabel("partial")).toBe("预算耗尽");
    expect(outcomeLabel("cancelled")).toBe("已取消");
    expect(outcomeLabel("no_report")).toBe("无汇报");
  });

  it("帧分发剧本（subStarted/subFrame/subReport/subDone，实时与重放共用）：重放兜底建行、双通道幂等去重、turn 推进、收口", () => {
    // subStarted 以 callId/role 建行（重放先于事件到达的兜底）；与事件通道按 subId 幂等
    applySubagentFrame("s1", { type: "subStarted", callId: "call_9", subId: "sub-1", role: "r" });
    subSpawn({ sessionId: "s1", runId: "r1", subId: "sub-1", role: "r" });
    expect(list("s1")).toHaveLength(1);
    expect(list("s1")[0]!.callId).toBe("call_9");
    expect(list("s1")[0]!.role).toBe("r");
    expect(list("s1")[0]!.state).toBe("running");

    // subFrame(stepStarted) 推进轮次；非 step 子帧不改状态
    applySubagentFrame("s1", {
      type: "subFrame",
      subId: "sub-1",
      frame: { type: "stepStarted", turn: 5 },
    });
    expect(list("s1")[0]!.turn).toBe(5);
    applySubagentFrame("s1", {
      type: "subFrame",
      subId: "sub-1",
      frame: { type: "textDelta", turn: 5, delta: "x" },
    });
    expect(list("s1")[0]!.turn).toBe(5);

    // subReport/subDone 帧收口（重复到达不产生第二行）
    applySubagentFrame("s1", { type: "subReport", subId: "sub-1", report: "汇报" });
    applySubagentFrame("s1", { type: "subDone", subId: "sub-1", outcome: "partial" });
    expect(list("s1")).toHaveLength(1);
    expect(list("s1")[0]!.report).toBe("汇报");
    expect(list("s1")[0]!.state).toBe("partial");

    // 非子代理帧不进 store
    applySubagentFrame("s1", { type: "runStarted", runId: "r", model: "m" });
    applySubagentFrame("s1", { type: "textDelta", turn: 1, delta: "x" });
    expect(list("s1")).toHaveLength(1);
  });

  it("抽屉开合与历史缓存：setOpenSub 开合；markHistoryLoading/setHistoryLoaded 记录缓存", () => {
    setOpenSub("sub-1");
    expect(useSubagentsStore.getState().openSubId).toBe("sub-1");
    setOpenSub(null);
    expect(useSubagentsStore.getState().openSubId).toBeNull();

    markHistoryLoading("sub-1");
    expect(useSubagentsStore.getState().historyBySub["sub-1"]?.loading).toBe(true);
    setHistoryLoaded("sub-1", [{ id: "m1" }], null);
    const history = useSubagentsStore.getState().historyBySub["sub-1"]!;
    expect(history.loading).toBe(false);
    expect(history.messages).toHaveLength(1);
  });
});
