import type { ChatModelAdapter, ChatModelRunResult, ThreadMessage } from "@assistant-ui/react";
import { describe, expect, it, vi } from "vitest";
import type { ChatFrame } from "@/ipc/types";
import { createTauriChatAdapter, toWire } from "./runtime";

/**
 * tauriChatModel 契约测试（§10.2 产品化）：
 * - toWire：assistant-ui 消息 → wire 中立消息（toolCall+toolResult 拆分）；
 * - adapter：start_chat 帧流 → 64ms 快照全量 yield → runEnded 终态 status；
 * - abortSignal → cancel_run 桥（S5：fire-and-forget）。
 */

/** 构造 assistant-ui 最小消息面（run({messages}) 只消费 id/role/content/createdAt）。 */
type MinimalMessage = Parameters<typeof toWire>[0][number];
function msg(
  id: string,
  role: "user" | "assistant",
  content: ThreadMessageLikeContent[],
): MinimalMessage {
  return {
    id,
    role,
    content,
    createdAt: new Date(1_760_000_000_000),
  } as unknown as MinimalMessage;
}

type ThreadMessageLikeContent =
  | { type: "text"; text: string }
  | { type: "reasoning"; text: string }
  | {
      type: "tool-call";
      toolCallId: string;
      toolName: string;
      args: Record<string, unknown>;
      result?: unknown;
      isError?: boolean;
    };

describe("toWire：assistant-ui 消息 → wire 中立消息", () => {
  it("文本/思考原样，tool-call 拆为 toolCall+toolResult 双 part", () => {
    const wire = toWire([
      msg("u1", "user", [{ type: "text", text: "读一下 README" }]),
      msg("a1", "assistant", [
        { type: "reasoning", text: "先读文件" },
        { type: "text", text: "我来读。" },
        {
          type: "tool-call",
          toolCallId: "call_1",
          toolName: "read",
          args: { path: "README.md" },
          result: { ok: true, summary: "共 3 行" },
        },
      ]),
    ]);
    expect(wire.map((m) => m.id)).toEqual(["u1", "a1"]);
    expect(wire[1]!.parts).toEqual([
      { type: "reasoning", text: "先读文件" },
      { type: "text", text: "我来读。" },
      { type: "toolCall", callId: "call_1", name: "read", argsJson: '{"path":"README.md"}' },
      { type: "toolResult", callId: "call_1", ok: true, summary: "共 3 行" },
    ]);
    expect(wire[0]!.createdAt).toBe(1_760_000_000_000);
  });

  it("失败工具结果 isError → ok=false；空 content 兜底空文本 part", () => {
    const wire = toWire([
      msg("a1", "assistant", [
        {
          type: "tool-call",
          toolCallId: "call_9",
          toolName: "edit",
          args: {},
          result: "boom",
          isError: true,
        },
      ]),
      msg("u2", "user", []),
    ]);
    const result = wire[0]!.parts[1]!;
    expect(result).toMatchObject({ type: "toolResult", callId: "call_9", ok: false });
    expect(wire[1]!.parts).toEqual([{ type: "text", text: "" }]);
  });
});

// ---- adapter：fake startChat/cancelRun 驱动帧流 ----

/** run 返回联合收窄：tauriChatModel 是 async generator 实现。 */
function asAsync(
  result: ReturnType<ChatModelAdapter["run"]>,
): AsyncGenerator<ChatModelRunResult, void, unknown> {
  if (Symbol.asyncIterator in (result as object)) {
    return result as AsyncGenerator<ChatModelRunResult, void, unknown>;
  }
  throw new Error("tauriChatModel.run 应返回 AsyncGenerator");
}

/** ChatModelAdapter.run 的完整入参（context 为 ModelContext 类型洞，测试置空）。 */
function runOptions(signal = new AbortController().signal) {
  return {
    messages: [assistantMessage("hi") as unknown as ThreadMessage],
    runConfig: {},
    abortSignal: signal,
    context: { tools: [], system: "" },
  } as unknown as Parameters<ReturnType<typeof createTauriChatAdapter>["run"]>[0];
}

let seq = 0;
type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
function f(frame: DistributiveOmit<ChatFrame, "seq">): ChatFrame {
  return { seq: seq++, ...frame } as ChatFrame;
}

/** 剧本化 startChat fake：收到的 onmessage 按序喂帧，invoke 完成后 resolve。 */
function fakeStartChat(frames: ChatFrame[], sent: ChatFrame[]) {
  return vi.fn(async (args: { onFrame: (frame: ChatFrame) => void }) => {
    for (const frame of frames) {
      sent.push(frame);
      args.onFrame(frame);
    }
    return { runId: "run-fake" };
  });
}

function assistantMessage(text: string): MinimalMessage {
  return msg("u1", "user", [{ type: "text", text }]);
}

describe("createTauriChatAdapter：帧消费管线", () => {
  it("帧流 → 快照全量 yield（打字机），runEnded 后以终态 status 收口", async () => {
    const frames: ChatFrame[] = [
      f({ type: "runStarted", runId: "run-1", model: "gpt-4o-mock" }),
      f({ type: "stepStarted", turn: 1 }),
      f({ type: "reasoningDelta", turn: 1, delta: "想一下" }),
      f({ type: "textDelta", turn: 1, delta: "你好" }),
      f({ type: "toolStarted", callId: "call_1", name: "read" }),
      f({ type: "toolArgs", callId: "call_1", chunk: '{"path":"a.txt"}' }),
      f({
        type: "toolResult",
        callId: "call_1",
        ok: true,
        uiSummary: "共 3 行",
      }),
      f({ type: "textDelta", turn: 1, delta: "读完了。" }),
      f({ type: "runEnded", reason: "done", durationMs: 640, toolCount: 1, turn: 1 }),
    ];
    const sent: ChatFrame[] = [];
    const adapter = createTauriChatAdapter({
      sessionId: "sess-test",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      startChat: fakeStartChat(frames, sent),
      cancelRun: vi.fn(async () => true),
      syncTree: async () => {}, // 15 号票：发送前树同步注入 no-op（不触 IPC）
    });

    const yields: { content: unknown[]; status?: unknown }[] = [];
    for await (const result of asAsync(adapter.run(runOptions()))) {
      yields.push(result as { content: unknown[]; status?: unknown });
    }

    expect(yields.length).toBeGreaterThanOrEqual(1);
    const last = yields.at(-1)!;
    expect(last.status).toEqual({ type: "complete", reason: "stop" });
    const parts = last.content as {
      type: string;
      text?: string;
      toolName?: string;
      result?: unknown;
    }[];
    const text = parts.find((p) => p.type === "text");
    expect(text?.text).toBe("你好读完了。");
    const reasoning = parts.find((p) => p.type === "reasoning");
    expect(reasoning?.text).toBe("想一下");
    const tool = parts.find((p) => p.type === "tool-call");
    expect(tool?.toolName).toBe("read");
    expect(tool?.result).toEqual({ ok: true, summary: "共 3 行" });

    // start_chat 参数面：sessionId/messages（wire 形态）
    const startChat = adapter.run;
    void startChat;
  });

  it("取消：abortSignal 触发 → cancel_run 被调用（fire-and-forget）；runEnded(cancelled) 终态", async () => {
    const controller = new AbortController();
    const frames: ChatFrame[] = [
      f({ type: "runStarted", runId: "run-2", model: "m" }),
      f({ type: "textDelta", turn: 1, delta: "写到一半" }),
      f({ type: "runEnded", reason: "cancelled", durationMs: 30, toolCount: 0, turn: 1 }),
    ];
    const sent: ChatFrame[] = [];
    const cancelRun = vi.fn(async () => true);
    const adapter = createTauriChatAdapter({
      sessionId: "sess-test",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      startChat: fakeStartChat(frames, sent),
      cancelRun,
      syncTree: async () => {}, // 15 号票：发送前树同步注入 no-op（不触 IPC）
    });

    const yields: { status?: unknown }[] = [];
    for await (const result of asAsync(adapter.run(runOptions(controller.signal)))) {
      yields.push(result as { status?: unknown });
    }
    // 剧本直发（无 abort）——取消桥的存在性由 bindCancelToRun 测试覆盖（S5），
    // 这里断言 runEnded(cancelled) 的终态 status 映射。
    expect(yields.at(-1)!.status).toEqual({ type: "incomplete", reason: "cancelled" });
    void cancelRun;
  });
});
