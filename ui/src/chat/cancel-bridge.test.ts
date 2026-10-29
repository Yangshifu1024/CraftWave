import { describe, expect, it } from "vitest";
import { bindCancelToRun, type InvokeFn } from "./cancel-bridge";
import { FrameBuffer } from "./frames";

/**
 * 【S5 Spike】取消传播前端挂接测试：
 * 1. abort → 恰一次 invoke("cancel_run")、参数带 sessionId、不 await（fire-and-forget）；
 * 2. runEnded(cancelled) 帧驱动 FrameBuffer 终态（前端不自行收尾，权威终态来自 Rust）；
 * 3. 已 abort 的 signal 立即触发；解绑后 abort 不再 invoke。
 */

describe("cancel-bridge", () => {
  it("abort 触发恰一次 cancel_run，参数带 sessionId", async () => {
    const calls: Array<{ cmd: string; args?: unknown }> = [];
    const invoke: InvokeFn = (cmd, args) => {
      calls.push({ cmd, args });
      return Promise.resolve(true);
    };
    const controller = new AbortController();
    const dispose = bindCancelToRun(controller.signal, "session-1", invoke);

    controller.abort();
    // fire-and-forget：invoke 已同步发起（微任务内 resolve）
    await Promise.resolve();
    expect(calls).toEqual([{ cmd: "cancel_run", args: { sessionId: "session-1" } }]);

    // 重复 abort 不再触发（once）
    controller.abort();
    await Promise.resolve();
    expect(calls).toHaveLength(1);

    dispose();
  });

  it("signal 已 aborted 时立即触发（run 中途 abort 的窗口）", async () => {
    const calls: Array<{ cmd: string }> = [];
    const controller = new AbortController();
    controller.abort();
    bindCancelToRun(controller.signal, "s2", (cmd) => {
      calls.push({ cmd });
      return Promise.resolve(true);
    });
    await Promise.resolve();
    expect(calls).toEqual([{ cmd: "cancel_run" }]);
  });

  it("解绑后 abort 不再 invoke", async () => {
    const calls: Array<{ cmd: string }> = [];
    const controller = new AbortController();
    const dispose = bindCancelToRun(controller.signal, "s3", (cmd) => {
      calls.push({ cmd });
      return Promise.resolve(true);
    });
    dispose();
    controller.abort();
    await Promise.resolve();
    expect(calls).toEqual([]);
  });

  it("全链前端语义：cancel_run 发起后 runEnded(cancelled) 帧驱动终态（§5.3 run 状态机）", async () => {
    const invokeCalls: Array<{ cmd: string; args?: unknown }> = [];
    const controller = new AbortController();
    const buffer = new FrameBuffer();
    const dispose = bindCancelToRun(controller.signal, "s4", (cmd, args) => {
      invokeCalls.push({ cmd, args });
      return Promise.resolve(true);
    });

    // 模拟 Rust 侧帧流：run 中 → 用户点停 → Rust 收割后回发 runEnded(cancelled)
    buffer.push({ seq: 0, type: "runStarted", runId: "run-1", model: "gpt-4o-mock" });
    buffer.push({ seq: 1, type: "textDelta", turn: 1, delta: "读到一半…" });
    controller.abort();
    await Promise.resolve();
    buffer.push({
      seq: 2,
      type: "runEnded",
      reason: "cancelled",
      durationMs: 87,
      toolCount: 0,
      turn: 1,
    });

    const snap = buffer.snapshot();
    expect(snap.started).toBe(true);
    expect(snap.segments).toEqual([{ kind: "text", text: "读到一半…" }]);
    expect(snap.ended?.reason).toBe("cancelled");
    expect(snap.ended?.durationMs).toBe(87);
    expect(invokeCalls).toEqual([{ cmd: "cancel_run", args: { sessionId: "s4" } }]);
    dispose();
  });

  it("取消批的帧序：已完成工具保留 + 被收割工具 E_CANCELLED + 终态收尾（镜像工具卡消费面）", () => {
    const buffer = new FrameBuffer();
    buffer.push({ seq: 0, type: "runStarted", runId: "run-2", model: "gpt-4o-mock" });
    buffer.push({ seq: 1, type: "toolStarted", callId: "call_1", name: "read" });
    buffer.push({ seq: 2, type: "toolStarted", callId: "call_2", name: "command" });
    // call_1 已完成：结果保留（不回滚）
    buffer.push({
      seq: 3,
      type: "toolResult",
      callId: "call_1",
      ok: true,
      uiSummary: "[read] 共 12 行：port = 8087",
    });
    // call_2 被收割：ok=false（§3.4 error 面在 ui 类型外，帧透传不破坏解析）
    buffer.push({
      seq: 4,
      type: "toolResult",
      callId: "call_2",
      ok: false,
      uiSummary: "E_CANCELLED: 取消：进程组已收割",
    });
    buffer.push({
      seq: 5,
      type: "runEnded",
      reason: "cancelled",
      durationMs: 96,
      toolCount: 2,
      turn: 1,
    });

    const snap = buffer.snapshot();
    expect(snap.toolCalls).toHaveLength(2);
    expect(snap.toolCalls[0]?.result).toEqual({
      ok: true,
      uiSummary: "[read] 共 12 行：port = 8087",
    });
    expect(snap.toolCalls[1]?.result?.ok).toBe(false);
    expect(snap.toolCalls[1]?.result?.uiSummary).toContain("E_CANCELLED");
    expect(snap.ended?.reason).toBe("cancelled");
    expect(snap.ended?.toolCount).toBe(2);
  });
});
