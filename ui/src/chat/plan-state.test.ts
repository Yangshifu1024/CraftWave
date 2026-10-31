/**
 * 计划状态 store 单测（25 号票，§3.5 plan:update 全量计划态）：applyUpdate
 * 整份替换、会话隔离、forgetSession 清桶——PlanCard/Inspector 基线标记与
 * 范围展示的数据面。
 */

import { beforeEach, describe, expect, it } from "vitest";
import { EMPTY_PLAN_STATE, usePlanStateStore } from "./plan-state";

/** 样例计划态：已批准 + 基线 2 条。 */
function sampleState(approved = false) {
  return {
    items: [{ id: "p1", text: "写 a", status: "in_progress" as const }],
    paths: ["src/a.rs", "tests/"],
    approved,
    baselinePaths: approved ? ["src/a.rs", "tests/"] : [],
  };
}

beforeEach(() => {
  usePlanStateStore.setState({ bySession: {} });
});

describe("plan-state store（25 号票全量计划态）", () => {
  it("applyUpdate 整份替换并按会话隔离、forgetSession 清桶、EMPTY_PLAN_STATE 稳定空态", () => {
    usePlanStateStore.getState().applyUpdate("s1", sampleState(true));
    usePlanStateStore.getState().applyUpdate("s2", sampleState(false));
    const s1 = usePlanStateStore.getState().bySession.s1;
    const s2 = usePlanStateStore.getState().bySession.s2;
    expect(s1.approved).toBe(true);
    expect(s1.baselinePaths).toEqual(["src/a.rs", "tests/"]);
    expect(s2.approved).toBe(false);
    expect(s2.baselinePaths).toEqual([]);
    // 同会话再次更新：整份替换（不残留旧字段）
    usePlanStateStore.getState().applyUpdate("s1", {
      items: [],
      paths: [],
      approved: true,
      baselinePaths: ["docs/"],
    });
    expect(usePlanStateStore.getState().bySession.s1).toEqual({
      items: [],
      paths: [],
      approved: true,
      baselinePaths: ["docs/"],
    });
    // forgetSession 清桶
    usePlanStateStore.getState().forgetSession("s1");
    expect(usePlanStateStore.getState().bySession.s1).toBeUndefined();
    // EMPTY_PLAN_STATE 是稳定空态（selector 缺桶兜底）
    expect(EMPTY_PLAN_STATE.items).toEqual([]);
    expect(EMPTY_PLAN_STATE.approved).toBe(false);
    expect(EMPTY_PLAN_STATE.baselinePaths).toEqual([]);
  });
});
