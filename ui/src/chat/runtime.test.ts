import type { ChatModelAdapter, ChatModelRunResult, ThreadMessage } from "@assistant-ui/react";
import { describe, expect, it, vi } from "vitest";
import type { RunConfig, SequencedFrame } from "@/ipc/types";
import { createTauriChatAdapter, sessionRunConfig, toWire } from "./runtime";
import { useSessionModelStore } from "./session-model";
import { useThinkingStore } from "./thinking-level";

/**
 * tauriChatModel 契约测试：
 * - toWire：assistant-ui 消息 → wire 中立消息（toolCall+toolResult 拆分）；
 * - adapter：start_chat 帧流 → 64ms 快照全量 yield → runEnded 终态 status；
 *   abortSignal → cancel_run 桥（fire-and-forget）；
 * - 发送参数面：发送前树同步（syncTree 收原文）、发送合成引用段、
 *   会话内存态 runConfig 随发送生效。
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
    messages: [msg("u1", "user", [{ type: "text", text: "hi" }]) as unknown as ThreadMessage],
    runConfig: {},
    abortSignal: signal,
    context: { tools: [], system: "" },
  } as unknown as Parameters<ReturnType<typeof createTauriChatAdapter>["run"]>[0];
}

/** 帧序号自增分配器：给无 seq 的帧载荷编号（对齐真实后端行为）。 */
let seq = 0;
type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
function f(frame: DistributiveOmit<SequencedFrame, "seq">): SequencedFrame {
  return { seq: seq++, ...frame } as SequencedFrame;
}

/** 剧本化 startChat fake：收到的 onmessage 按序喂帧。 */
function fakeStartChat(frames: SequencedFrame[]) {
  return vi.fn(async (args: { onFrame: (frame: SequencedFrame) => void }) => {
    for (const frame of frames) {
      args.onFrame(frame);
    }
    return { runId: "run-fake" };
  });
}

describe("runtime（tauriChatModel 契约）", () => {
  it("toWire：文本/思考原样，tool-call 拆为 toolCall+toolResult 双 part，isError→ok=false，空 content 兜底", () => {
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
    // 失败工具结果 isError → ok=false；空 content 兜底空文本 part
    const failing = toWire([
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
    expect(failing[0]!.parts[1]).toMatchObject({ type: "toolResult", callId: "call_9", ok: false });
    expect(failing[1]!.parts).toEqual([{ type: "text", text: "" }]);
  });

  it("帧消费管线：帧流→快照全量 yield（打字机交错时间线），runEnded 映射终态（done/cancelled/零帧 reject 错误）", async () => {
    // ---- 帧流 → 快照全量 yield，runEnded 后以终态 status 收口 ----
    const frames: SequencedFrame[] = [
      f({ type: "runStarted", runId: "run-1", model: "gpt-4o-mock" }),
      f({ type: "stepStarted", turn: 1 }),
      f({ type: "reasoningDelta", turn: 1, delta: "想一下" }),
      f({ type: "textDelta", turn: 1, delta: "你好" }),
      f({ type: "toolStarted", callId: "call_1", name: "read" }),
      f({ type: "toolArgs", callId: "call_1", chunk: '{"path":"a.txt"}' }),
      f({ type: "toolResult", callId: "call_1", ok: true, uiSummary: "共 3 行" }),
      f({ type: "textDelta", turn: 1, delta: "读完了。" }),
      f({ type: "runEnded", reason: "done", durationMs: 640, toolCount: 1, turn: 1 }),
    ];
    const adapter = createTauriChatAdapter({
      sessionId: "sess-test",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      startChat: fakeStartChat(frames),
      cancelRun: vi.fn(async () => true),
      syncTree: async () => {}, // 发送前树同步注入 no-op（不触 IPC）
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
    // 交错时间线：正文两段按帧到达序分布在工具卡两侧（你好 → 工具 → 读完了。），
    // 不再按类型归堆成单段 text
    expect(parts.map((p) => p.type)).toEqual(["reasoning", "text", "tool-call", "text"]);
    expect(parts.filter((p) => p.type === "text").map((p) => p.text)).toEqual(["你好", "读完了。"]);
    const reasoning = parts.find((p) => p.type === "reasoning");
    expect(reasoning?.text).toBe("想一下");
    const tool = parts.find((p) => p.type === "tool-call");
    expect(tool?.toolName).toBe("read");
    expect(tool?.result).toEqual({ ok: true, summary: "共 3 行" });

    // ---- runEnded(cancelled)：剧本直发（无 abort）——取消桥的存在性由
    // bindCancelToRun 测试覆盖（S5），这里断言终态 status 映射 ----
    const cancelAdapter = createTauriChatAdapter({
      sessionId: "sess-test",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      startChat: fakeStartChat([
        f({ type: "runStarted", runId: "run-2", model: "m" }),
        f({ type: "textDelta", turn: 1, delta: "写到一半" }),
        f({ type: "runEnded", reason: "cancelled", durationMs: 30, toolCount: 0, turn: 1 }),
      ]),
      cancelRun: vi.fn(async () => true),
      syncTree: async () => {},
    });
    const cancelledYields: { status?: unknown }[] = [];
    for await (const result of asAsync(cancelAdapter.run(runOptions()))) {
      cancelledYields.push(result as { status?: unknown });
    }
    expect(cancelledYields.at(-1)!.status).toEqual({ type: "incomplete", reason: "cancelled" });

    // ---- 命令面失败（如未配模型）：零帧 reject → 合成错误终态帧收口，不挂死 ----
    const failingAdapter = createTauriChatAdapter({
      sessionId: "sess-test",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      // 复刻 resolve_model 前置失败：注册 run 前即 reject，一帧不发
      startChat: vi.fn(async () => {
        throw {
          code: "E_INVALID_ARG",
          message: "未指定模型：RunConfig.modelId 与 activeModelId 均为空",
        };
      }),
      cancelRun: vi.fn(async () => true),
      syncTree: async () => {},
    });
    const errorYields: { status?: unknown }[] = [];
    for await (const result of asAsync(failingAdapter.run(runOptions()))) {
      errorYields.push(result as { status?: unknown });
    }
    expect(errorYields.length).toBeGreaterThanOrEqual(1);
    // 错误文案随 status.error 透出（线程错误态显示原文，而非 "An error occurred"）
    expect(errorYields.at(-1)!.status).toEqual({
      type: "incomplete",
      reason: "error",
      error: "E_INVALID_ARG: 未指定模型：RunConfig.modelId 与 activeModelId 均为空",
    });
  });

  it("发送参数面：失败后的新消息完整送到后端（syncTree 收原文）、发送合成引用段、sessionRunConfig 会话覆盖生效", async () => {
    // ---- 失败后的新消息完整送到后端：发送前同步与实际请求保持同一条消息链 ----
    const messages = [
      msg("u1", "user", [{ type: "text", text: "检查配置" }]),
      msg("a1", "assistant", []),
      msg("u2", "user", [{ type: "text", text: "继续检查" }]),
    ];
    const syncTree = vi.fn(async () => {});
    const startChat = vi.fn(async (args: { onFrame: (frame: SequencedFrame) => void }) => {
      args.onFrame(f({ type: "runEnded", reason: "done", durationMs: 0, toolCount: 0, turn: 1 }));
      return { runId: "run-next" };
    });
    const chainAdapter = createTauriChatAdapter({
      sessionId: "sess-test",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      startChat,
      cancelRun: async () => true,
      syncTree,
    });
    for await (const _ of asAsync(chainAdapter.run({ ...runOptions(), messages }))) void _;
    expect(syncTree).toHaveBeenCalledExactlyOnceWith(messages);
    expect(startChat).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ messages: toWire(messages) }),
    );

    // ---- 发送合成引用段：wire 面最后一条 user 尾部追加
    // [引用文件] 段；树同步（syncTree）与输入框原文不受影响 ----
    const refMessages = [
      msg("u1", "user", [{ type: "text", text: '看 @src/main.rs 和 @"a b/c.md" 说明了什么' }]),
    ];
    const sent: unknown[][] = [];
    const refStartChat = vi.fn(
      async (args: { messages: unknown[]; onFrame: (frame: SequencedFrame) => void }) => {
        sent.push(args.messages);
        args.onFrame(f({ type: "runEnded", reason: "done", durationMs: 0, toolCount: 0, turn: 1 }));
        return { runId: "run-refs" };
      },
    );
    const refSyncTree = vi.fn(async () => {});
    const refAdapter = createTauriChatAdapter({
      sessionId: "sess-refs",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      startChat: refStartChat,
      cancelRun: async () => true,
      syncTree: refSyncTree,
    });
    for await (const _ of asAsync(refAdapter.run({ ...runOptions(), messages: refMessages })))
      void _;
    const refWire = sent[0] as Array<{ parts: Array<{ type: string; text?: string }> }>;
    const lastText = refWire[0].parts
      .filter((p) => p.type === "text")
      .map((p) => p.text)
      .join("");
    expect(lastText).toBe(
      '看 @src/main.rs 和 @"a b/c.md" 说明了什么\n\n[引用文件]\n- @src/main.rs\n- @a b/c.md',
    );
    // 树同步收原始 messages（原文落盘，引用段只在 wire 面）
    expect(refSyncTree).toHaveBeenCalledExactlyOnceWith(refMessages);
    // 绝对路径 token 原样保留不进引用段（拖拽形态，read 照走授权链）
    const absMessages = [msg("u1", "user", [{ type: "text", text: "看 @/tmp/x.txt 和 @ok.txt" }])];
    for await (const _ of asAsync(refAdapter.run({ ...runOptions(), messages: absMessages })))
      void _;
    const absWire = sent[1] as Array<{ parts: Array<{ type: string; text?: string }> }>;
    expect(
      absWire[0].parts
        .filter((p) => p.type === "text")
        .map((p) => p.text)
        .join(""),
    ).toBe("看 @/tmp/x.txt 和 @ok.txt\n\n[引用文件]\n- @ok.txt");
    // 无 @token 消息 wire 面原样（与 toWire 基线一致）
    const plainMessages = [msg("u1", "user", [{ type: "text", text: "普通消息没有引用" }])];
    for await (const _ of asAsync(refAdapter.run({ ...runOptions(), messages: plainMessages })))
      void _;
    expect(sent[2]).toEqual(toWire(plainMessages));

    // ---- sessionRunConfig：会话内存态 runConfig ----
    // 全部未设置：只有 DEFAULT_RUN_CONFIG 基线（modelId/thinking 键省略）
    const base = sessionRunConfig("sess-rc");
    expect(base.modelId).toBeUndefined();
    expect(base.thinking).toBeUndefined();
    expect(base.tier).toBe("autoEdit");
    // 设置模型与 thinking：两字段都出现
    useSessionModelStore.getState().setModel("sess-rc", "model-xyz");
    useThinkingStore.getState().setLevel("sess-rc", "high");
    const covered = sessionRunConfig("sess-rc");
    expect(covered.modelId).toBe("model-xyz");
    expect(covered.thinking).toBe("high");
    // 显式 off 也要带上（off 是用户明确选择，区别于未设置的回落）
    useThinkingStore.getState().setLevel("sess-rc", "off");
    expect(sessionRunConfig("sess-rc").thinking).toBe("off");
    useSessionModelStore.getState().setModel("sess-rc", undefined);
    useThinkingStore.getState().setLevel("sess-rc", undefined);
    expect(sessionRunConfig("sess-rc").modelId).toBeUndefined();
    expect(sessionRunConfig("sess-rc").thinking).toBeUndefined();

    // ---- 会话覆盖的模型 id 在发送时带入 start_chat；未覆盖时省略 modelId ----
    const sentArgs: { runConfig: RunConfig }[] = [];
    const runConfigFake = vi.fn(
      async (args: { runConfig: RunConfig; onFrame: (frame: SequencedFrame) => void }) => {
        sentArgs.push({ runConfig: args.runConfig });
        for (const frame of [
          f({ type: "runStarted", runId: "run-1", model: "m" }),
          f({ type: "runEnded", reason: "done", durationMs: 10, toolCount: 0, turn: 1 }),
        ]) {
          args.onFrame(frame);
        }
        return { runId: "run-fake" };
      },
    );
    // 直接注入 fake 的方式与生产 sessionAdapter 无缝衔接：经 deps 替换 startChat
    const runConfigAdapter = createTauriChatAdapter({
      sessionId: "sess-model",
      // 与 sessionAdapter 相同的惰性取值口径（生产函数直用）
      runConfig: () => sessionRunConfig("sess-model"),
      startChat: runConfigFake,
      cancelRun: async () => true,
      syncTree: async () => {},
    });
    // 未覆盖：不带 modelId；覆盖后：带上会话选择
    for await (const _ of asAsync(runConfigAdapter.run(runOptions()))) void _;
    expect(sentArgs.at(-1)!.runConfig.modelId).toBeUndefined();
    useSessionModelStore.getState().setModel("sess-model", "model-xyz");
    for await (const _ of asAsync(runConfigAdapter.run(runOptions()))) void _;
    expect(sentArgs.at(-1)!.runConfig.modelId).toBe("model-xyz");
    useSessionModelStore.getState().setModel("sess-model", undefined);
  });
});
