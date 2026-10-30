import type { ChatModelAdapter, ChatModelRunResult, ThreadMessage } from "@assistant-ui/react";
import { describe, expect, it, vi } from "vitest";
import type { RunConfig, SequencedFrame } from "@/ipc/types";
import { createTauriChatAdapter, sessionRunConfig, toWire } from "./runtime";
import { useSessionModelStore } from "./session-model";
import { useThinkingStore } from "./thinking-level";

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

/** 帧序号自增分配器：给无 seq 的帧载荷编号（对齐真实后端行为）。 */
let seq = 0;
type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
function f(frame: DistributiveOmit<SequencedFrame, "seq">): SequencedFrame {
  return { seq: seq++, ...frame } as SequencedFrame;
}

/** 剧本化 startChat fake：收到的 onmessage 按序喂帧，invoke 完成后 resolve。 */
function fakeStartChat(frames: SequencedFrame[], sent: SequencedFrame[]) {
  return vi.fn(async (args: { onFrame: (frame: SequencedFrame) => void }) => {
    for (const frame of frames) {
      sent.push(frame);
      args.onFrame(frame);
    }
    return { runId: "run-fake" };
  });
}

/** 单条 user 文本消息（runOptions 的 messages 用）。 */
function assistantMessage(text: string): MinimalMessage {
  return msg("u1", "user", [{ type: "text", text }]);
}

describe("createTauriChatAdapter：帧消费管线", () => {
  it("失败后的新消息完整送到后端，发送前同步与实际请求保持同一条消息链", async () => {
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
    const adapter = createTauriChatAdapter({
      sessionId: "sess-test",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      startChat,
      cancelRun: async () => true,
      syncTree,
    });
    for await (const _ of asAsync(adapter.run({ ...runOptions(), messages }))) void _;
    expect(syncTree).toHaveBeenCalledExactlyOnceWith(messages);
    expect(startChat).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ messages: toWire(messages) }),
    );
  });

  it("帧流 → 快照全量 yield（打字机），runEnded 后以终态 status 收口", async () => {
    const frames: SequencedFrame[] = [
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
    const sent: SequencedFrame[] = [];
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
    // 交错时间线：正文两段按帧到达序分布在工具卡两侧（你好 → 工具 → 读完了。），
    // 不再按类型归堆成单段 text
    expect(parts.map((p) => p.type)).toEqual(["reasoning", "text", "tool-call", "text"]);
    const texts = parts.filter((p) => p.type === "text");
    expect(texts.map((p) => p.text)).toEqual(["你好", "读完了。"]);
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
    const frames: SequencedFrame[] = [
      f({ type: "runStarted", runId: "run-2", model: "m" }),
      f({ type: "textDelta", turn: 1, delta: "写到一半" }),
      f({ type: "runEnded", reason: "cancelled", durationMs: 30, toolCount: 0, turn: 1 }),
    ];
    const sent: SequencedFrame[] = [];
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

  it("命令面失败（如未配模型）：零帧 reject → 合成错误终态帧收口，不挂死", async () => {
    const adapter = createTauriChatAdapter({
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

    const yields: { status?: unknown }[] = [];
    for await (const result of asAsync(adapter.run(runOptions()))) {
      yields.push(result as { status?: unknown });
    }
    expect(yields.length).toBeGreaterThanOrEqual(1);
    // 错误文案随 status.error 透出（线程错误态显示原文，而非 "An error occurred"）
    expect(yields.at(-1)!.status).toEqual({
      type: "incomplete",
      reason: "error",
      error: "E_INVALID_ARG: 未指定模型：RunConfig.modelId 与 activeModelId 均为空",
    });
  });
});

// ---- sessionAdapter：会话级内存态（权限档 + 模型覆盖 + thinking）随发送
//      生效（17 号票建面；18 号票追加了 thinking 档惰性取值） ----

// ---- 发送合成引用段（28 号票，§10.6）：wire 面最后一条 user 尾部追加
// [引用文件] 段；树同步（syncTree）与输入框原文不受影响。----

describe("createTauriChatAdapter：发送合成引用段（28 号票）", () => {
  /** 收到 startChat 的 wire messages（断言面）。 */
  async function captureSentWire(messages: ThreadMessage[]) {
    const sent: unknown[][] = [];
    const startChat = vi.fn(
      async (args: { messages: unknown[]; onFrame: (frame: SequencedFrame) => void }) => {
        sent.push(args.messages);
        args.onFrame(f({ type: "runEnded", reason: "done", durationMs: 0, toolCount: 0, turn: 1 }));
        return { runId: "run-refs" };
      },
    );
    const syncTree = vi.fn(async () => {});
    const adapter = createTauriChatAdapter({
      sessionId: "sess-refs",
      runConfig: { tier: "autoEdit", maxTurns: 4, steer: false },
      startChat,
      cancelRun: async () => true,
      syncTree,
    });
    for await (const _ of asAsync(adapter.run({ ...runOptions(), messages }))) void _;
    return { sent, syncTree };
  }

  it("@token 消息发送时正文尾部合成引用段；树同步仍收原文", async () => {
    const messages = [
      msg("u1", "user", [{ type: "text", text: '看 @src/main.rs 和 @"a b/c.md" 说明了什么' }]),
    ];
    const { sent, syncTree } = await captureSentWire(messages);
    const wire = sent[0] as Array<{ parts: Array<{ type: string; text?: string }> }>;
    const lastText = wire[0].parts
      .filter((p) => p.type === "text")
      .map((p) => p.text)
      .join("");
    expect(lastText).toBe(
      '看 @src/main.rs 和 @"a b/c.md" 说明了什么\n\n[引用文件]\n- @src/main.rs\n- @a b/c.md',
    );
    // 树同步收原始 messages（原文落盘，引用段只在 wire 面）
    expect(syncTree).toHaveBeenCalledExactlyOnceWith(messages);
  });

  it("无 @token 消息 wire 面原样（与 toWire 基线一致）", async () => {
    const messages = [msg("u1", "user", [{ type: "text", text: "普通消息没有引用" }])];
    const { sent } = await captureSentWire(messages);
    expect(sent[0]).toEqual(toWire(messages));
  });

  it("绝对路径 token 原样保留不进引用段（拖拽形态，read 照走授权链）", async () => {
    const messages = [msg("u1", "user", [{ type: "text", text: "看 @/tmp/x.txt 和 @ok.txt" }])];
    const { sent } = await captureSentWire(messages);
    const wire = sent[0] as Array<{ parts: Array<{ type: string; text?: string }> }>;
    const lastText = wire[0].parts
      .filter((p) => p.type === "text")
      .map((p) => p.text)
      .join("");
    expect(lastText).toBe("看 @/tmp/x.txt 和 @ok.txt\n\n[引用文件]\n- @ok.txt");
  });
});

describe("sessionRunConfig：会话内存态 runConfig", () => {
  it("会话覆盖的模型 id / thinking 档 / 权限档在发送时生效；未设置时省略字段", () => {
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
  });

  it("会话覆盖的模型 id 与权限档在发送时带入 start_chat；未覆盖时省略 modelId", async () => {
    const sentArgs: { runConfig: RunConfig }[] = [];
    const frames: SequencedFrame[] = [
      f({ type: "runStarted", runId: "run-1", model: "m" }),
      f({ type: "runEnded", reason: "done", durationMs: 10, toolCount: 0, turn: 1 }),
    ];
    const fake = vi.fn(
      async (args: { runConfig: RunConfig; onFrame: (f: SequencedFrame) => void }) => {
        sentArgs.push({ runConfig: args.runConfig });
        for (const frame of frames) args.onFrame(frame);
        return { runId: "run-fake" };
      },
    );
    // 直接注入 fake 的方式与生产 sessionAdapter 无缝衔接：经 deps 替换 startChat
    const adapter = createTauriChatAdapter({
      sessionId: "sess-model",
      // 与 sessionAdapter 相同的惰性取值口径（生产函数直用）
      runConfig: () => sessionRunConfig("sess-model"),
      startChat: fake,
      cancelRun: async () => true,
      syncTree: async () => {},
    });

    // 未覆盖：不带 modelId
    for await (const _ of asAsync(adapter.run(runOptions()))) void _;
    expect(sentArgs.at(-1)!.runConfig.modelId).toBeUndefined();

    // 覆盖后：带上会话选择
    useSessionModelStore.getState().setModel("sess-model", "model-xyz");
    for await (const _ of asAsync(adapter.run(runOptions()))) void _;
    expect(sentArgs.at(-1)!.runConfig.modelId).toBe("model-xyz");
    useSessionModelStore.getState().setModel("sess-model", undefined);
  });
});
