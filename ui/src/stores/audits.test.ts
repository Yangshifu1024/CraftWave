// @vitest-environment happy-dom
/**
 * 灰区命令审计 store 单元测试（toolAudit 帧 → audits store）：
 * 帧分发写入、五态覆盖、会话分桶隔离、非审计帧忽略。
 */

import { beforeEach, describe, expect, it } from "vitest";
import type { AuditState } from "@/ipc/generated/AuditState";
import type { ChatFrame } from "@/ipc/generated/ChatFrame";
import { applyAuditFrame, auditOf, useAuditsStore } from "./audits";

/** 造 toolAudit 帧。 */
function auditFrame(callId: string, state: AuditState, reason?: string): ChatFrame {
  return { type: "toolAudit", callId, state, reason };
}

beforeEach(() => {
  useAuditsStore.setState({ bySession: {} });
});

describe("audits store：toolAudit 帧分发", () => {
  it("剧本：running→allow 同 callId 覆盖推进、五态全量写入、会话分桶隔离、非 toolAudit 帧忽略", () => {
    // running → allow 状态推进（同 callId 覆盖）
    applyAuditFrame("s1", auditFrame("call_part_1_0", "running"));
    expect(auditOf(useAuditsStore.getState().bySession, "s1", "call_part_1_0")).toEqual({
      state: "running",
      reason: undefined,
    });
    applyAuditFrame("s1", auditFrame("call_part_1_0", "allow", "常规构建"));
    expect(auditOf(useAuditsStore.getState().bySession, "s1", "call_part_1_0")).toEqual({
      state: "allow",
      reason: "常规构建",
    });

    // 五态全量写入（ask/manual 随 reason）
    for (const [state, reason] of [
      ["ask", "范围不明"],
      ["manual", "审计超时"],
      ["block", "数据外传"],
    ] as const) {
      applyAuditFrame("s1", auditFrame(`c-${state}`, state, reason));
      expect(auditOf(useAuditsStore.getState().bySession, "s1", `c-${state}`)).toEqual({
        state,
        reason,
      });
    }

    // 会话分桶隔离（同 callId 不同会话互不影响）
    applyAuditFrame("s1", auditFrame("call_part_1_0", "allow", "放行"));
    applyAuditFrame("s2", auditFrame("call_part_1_0", "block", "拒绝"));
    expect(auditOf(useAuditsStore.getState().bySession, "s1", "call_part_1_0")?.state).toBe(
      "allow",
    );
    expect(auditOf(useAuditsStore.getState().bySession, "s2", "call_part_1_0")?.state).toBe(
      "block",
    );

    // 非 toolAudit 帧忽略
    applyAuditFrame("s1", { type: "runStarted", runId: "r1", model: "m" });
    applyAuditFrame("s1", {
      type: "toolResult",
      callId: "c1",
      ok: true,
      uiSummary: "ok",
    });
    expect(auditOf(useAuditsStore.getState().bySession, "s1", "c1")).toBeUndefined();
  });
});
