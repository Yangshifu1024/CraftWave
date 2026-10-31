import { describe, expect, it } from "vitest";
import { bindCancelToRun, type InvokeFn } from "./cancel-bridge";
import { FrameBuffer } from "./frames";

/**
 * 【S5 Spike】取消传播前端挂接测试：
 * 1. abort → 恰一次 invoke("cancel_run")、参数带 sessionId、不 await（fire-and-forget）；
 * 2. 已 abort 的 signal 立即触发；解绑后 abort 不再 invoke；
 * 3. runEnded(cancelled) 帧驱动 FrameBuffer 终态（前端不自行收尾，权威终态来自 Rust）；
 * 4. 取消批的帧序：已完成工具保留 + 被收割工具 E_CANCELLED + 终态收尾。
 */

describe("cancel-bridge（S5 取消传播）", () => {
  it("abort 触发恰一次 cancel_run（once、已 aborted 立即触发、解绑失效），runEnded(cancelled) 帧驱动终态与取消批帧序", async () => {
    // ---- abort 触发恰一次 cancel_run，参数带 sessionId（fire-and-forget：invoke 已同步发起）----
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

    // ---- signal 已 aborted 时立即触发（run 中途 abort 的窗口）----
    const alreadyCalls: Array<{ cmd: string }> = [];
    const aborted = new AbortController();
    aborted.abort();
    bindCancelToRun(aborted.signal, "s2", (cmd) => {
      alreadyCalls.push({ cmd });
      return Promise.resolve(true);
    });
    await Promise.resolve();
    expect(alreadyCalls).toEqual([{ cmd: "cancel_run" }]);

    // ---- 解绑后 abort 不再 invoke ----
    const disposedCalls: Array<{ cmd: string }> = [];
    const late = new AbortController();
    const lateDispose = bindCancelToRun(late.signal, "s3", (cmd) => {
      disposedCalls.push({ cmd });
      return Promise.resolve(true);
    });
    lateDispose();
    late.abort();
    await Promise.resolve();
    expect(disposedCalls).toEqual([]);

    // ---- 全链前端语义：cancel_run 发起后 runEnded(cancelled) 帧驱动终态（run 状态机）----
    const invokeCalls: Array<{ cmd: string; args?: unknown }> = [];
    const chainController = new AbortController();
    const buffer = new FrameBuffer();
    const chainDispose = bindCancelToRun(chainController.signal, "s4", (cmd, args) => {
      invokeCalls.push({ cmd, args });
      return Promise.resolve(true);
    });

    // 模拟 Rust 侧帧流：run 中 → 用户点停 → Rust 收割后回发 runEnded(cancelled)
    buffer.push({ seq: 0, type: "runStarted", runId: "run-1", model: "gpt-4o-mock" });
    buffer.push({ seq: 1, type: "textDelta", turn: 1, delta: "读到一半…" });
    chainController.abort();
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
    chainDispose();

    // ---- 取消批的帧序：已完成工具保留 + 被收割工具 E_CANCELLED + 终态收尾（镜像工具卡消费面）----
    const batch = new FrameBuffer();
    batch.push({ seq: 0, type: "runStarted", runId: "run-2", model: "gpt-4o-mock" });
    batch.push({ seq: 1, type: "toolStarted", callId: "call_1", name: "read" });
    batch.push({ seq: 2, type: "toolStarted", callId: "call_2", name: "command" });
    // call_1 已完成：结果保留（不回滚）
    batch.push({
      seq: 3,
      type: "toolResult",
      callId: "call_1",
      ok: true,
      uiSummary: "[read] 共 12 行：port = 8087",
    });
    // call_2 被收割：ok=false（error 面在 ui 类型外，帧透传不破坏解析）
    batch.push({
      seq: 4,
      type: "toolResult",
      callId: "call_2",
      ok: false,
      uiSummary: "E_CANCELLED: 取消：进程组已收割",
    });
    batch.push({
      seq: 5,
      type: "runEnded",
      reason: "cancelled",
      durationMs: 96,
      toolCount: 2,
      turn: 1,
    });

    const batchSnap = batch.snapshot();
    expect(batchSnap.toolCalls).toHaveLength(2);
    expect(batchSnap.toolCalls[0]?.result).toEqual({
      ok: true,
      uiSummary: "[read] 共 12 行：port = 8087",
    });
    expect(batchSnap.toolCalls[1]?.result?.ok).toBe(false);
    expect(batchSnap.toolCalls[1]?.result?.uiSummary).toContain("E_CANCELLED");
    expect(batchSnap.ended?.reason).toBe("cancelled");
    expect(batchSnap.ended?.toolCount).toBe(2);
  });
});
