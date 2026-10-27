import { describe, expect, it } from "vitest";
import { type ChatFrame, FrameBuffer, toParts } from "./frames";

/**
 * 【S2 Spike】前端帧管线测试：保真（无丢失无乱序）、半截 JSON「参数准备中」、
 * 64ms 节流快照节奏、帧到达 → 快照产出的前端侧延迟（NFR-02 余量佐证）。
 */

/** 复刻 Rust S2 集成测试的直通帧序列（两轮 + 工具调用 + 半截 JSON 分段）。 */
function scriptedFrames(): ChatFrame[] {
  let seq = 0;
  // 联合分发 Omit：保留每个帧变体自己的字段面
  type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
  type FrameArg = DistributiveOmit<ChatFrame, "seq">;
  const f = (frame: FrameArg): ChatFrame => ({ seq: seq++, ...frame }) as ChatFrame;
  return [
    f({ type: "runStarted", runId: "run-1", model: "gpt-4o-mock" }),
    f({ type: "stepStarted", turn: 1 }),
    f({ type: "reasoningDelta", turn: 1, delta: "用户要我看配置，" }),
    f({ type: "reasoningDelta", turn: 1, delta: "先调用 read。" }),
    f({ type: "textDelta", turn: 1, delta: "我来读一下" }),
    f({ type: "textDelta", turn: 1, delta: "配置文件。" }),
    f({ type: "toolStarted", callId: "call_1", name: "read" }),
    f({ type: "toolArgs", callId: "call_1", chunk: '{"path": "/etc/app' }),
    // 半截 JSON：此刻 argsJson 不是合法 JSON（卡片「参数准备中」）
    f({ type: "toolArgs", callId: "call_1", chunk: '/config.toml", "offset": 0}' }),
    f({
      type: "toolResult",
      callId: "call_1",
      ok: true,
      uiSummary: "[read] 共 12 行：port = 8087",
    }),
    f({ type: "stepStarted", turn: 2 }),
    f({ type: "textDelta", turn: 2, delta: "配置文件共 12 行，" }),
    f({ type: "textDelta", turn: 2, delta: "端口是 8087。" }),
    f({ type: "runEnded", reason: "done", durationMs: 812, toolCount: 1, turn: 2 }),
  ];
}

describe("FrameBuffer 保真", () => {
  it("帧序列累积：文本/思考/工具参数拼接与发送序列一致", () => {
    const buffer = new FrameBuffer();
    for (const frame of scriptedFrames()) buffer.push(frame);
    const snap = buffer.snapshot();

    expect(snap.text).toBe("我来读一下配置文件。配置文件共 12 行，端口是 8087。");
    expect(snap.reasoning).toBe("用户要我看配置，先调用 read。");
    expect(snap.toolCalls).toHaveLength(1);
    expect(snap.toolCalls[0]!.name).toBe("read");
    expect(snap.toolCalls[0]!.argsJson).toBe('{"path": "/etc/app/config.toml", "offset": 0}');
    expect(snap.toolCalls[0]!.result?.uiSummary).toContain("port = 8087");
    expect(snap.ended?.reason).toBe("done");
    expect(buffer.droppedSeq).toBe(0);
  });

  it("半截 JSON 判定：累积中不 ready，收流后 ready（参数准备中态）", () => {
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
    for (const frame of frames.slice(0, 8)) buffer.push(frame);
    const partial = buffer.snapshot().toolCalls[0]!;
    expect(FrameBuffer.argsReady(partial.argsJson)).toBe(false);

    buffer.push(frames[8]!); // 参数收尾片段
    const ready = buffer.snapshot().toolCalls[0]!;
    expect(FrameBuffer.argsReady(ready.argsJson)).toBe(true);
    expect(JSON.parse(ready.argsJson)).toEqual({ path: "/etc/app/config.toml", offset: 0 });
  });

  it("乱序/重复 seq 丢弃（§5.6 runEpoch 语义）且不影响后续累积", () => {
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
    buffer.push(frames[0]!); // seq0 runStarted
    buffer.push(frames[2]!); // seq2 第一段思考
    buffer.push({ ...frames[2]!, seq: 2 }); // 重复帧 → 丢弃
    buffer.push({ ...frames[3]!, seq: 1 }); // 倒退帧 → 丢弃
    buffer.push(frames[3]!); // seq3 第二段思考 → 收
    expect(buffer.droppedSeq).toBe(2);
    expect(buffer.snapshot().reasoning).toBe("用户要我看配置，先调用 read。");
  });
});

describe("FrameBuffer 快照节流与延迟埋点", () => {
  it("64ms 节流快照：帧间隔不小于节流窗口，末帧含 runEnded", async () => {
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
    // 模拟 Tauri Channel onmessage：帧按 20ms 间隔到达（快于节流窗口）
    const producer = (async () => {
      for (const frame of frames) {
        buffer.push(frame);
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    })();
    const consumer = buffer.snapshots(64);

    const yieldTimes: number[] = [];
    const yields: Awaited<ReturnType<typeof consumer.next>>["value"][] = [];
    for await (const snap of consumer) {
      yieldTimes.push(performance.now());
      yields.push(snap);
      if (snap.ended) break;
    }
    await producer;

    expect(yields.at(-1)!.text).toBe("我来读一下配置文件。配置文件共 12 行，端口是 8087。");
    expect(yields.at(-1)!.ended?.reason).toBe("done");
    // 快照节奏：相邻产出间隔 ≥ 64ms（64ms 窗口 ± 调度抖动）
    const gaps = yieldTimes.slice(1).map((t, i) => t - yieldTimes[i]!);
    for (const gap of gaps) {
      expect(gap).toBeGreaterThanOrEqual(60);
    }
    // 帧率上界：约 400ms 流长产出 ≤ 7 帧（64ms 窗口），IPC 压力可控
    expect(yields.length).toBeLessThanOrEqual(7);
    console.log("S2[前端] 快照间隔(ms):", gaps.map((g) => g.toFixed(1)).join(", "));
  });

  it("前端侧延迟：帧到达 → 快照可读，处理开销亚毫秒级（NFR-02 余量）", () => {
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
    const latencies: number[] = [];
    for (const frame of frames) {
      const t0 = performance.now();
      buffer.push(frame);
      const snap = buffer.snapshot(); // 消费侧读取全量快照（替换语义）
      if (frame.type === "runStarted" && !snap.started) {
        throw new Error("runStarted 后应可见 started 标志");
      }
      latencies.push(performance.now() - t0);
    }
    const max = Math.max(...latencies);
    const avg = latencies.reduce((a, b) => a + b, 0) / latencies.length;
    console.log(
      `S2[前端] 帧到达→快照可读：平均=${avg.toFixed(3)}ms 最大=${max.toFixed(3)}ms（共 ${latencies.length} 帧）`,
    );
    // NFR-02 全链 <100ms：Rust 侧实测 TTFT≈0.4ms + 节流 ≤64ms；
    // 前端 buffer+快照处理亚毫秒，给 React 渲染留出 >35ms 余量。
    expect(max).toBeLessThan(5);
  });

  it("消费侧挂载 toolArgs 半截 JSON 的「参数准备中」标志", () => {
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
    for (const frame of frames.slice(0, 8)) buffer.push(frame);
    const snap = buffer.snapshot();
    const [tool] = snap.toolCalls;
    // 卡片渲染判定（§10.2：半截 JSON 由 toolkit 卡片以「参数准备中」渲染）
    expect(FrameBuffer.argsReady(tool!.argsJson)).toBe(false);
  });
});

describe("FrameBuffer 本票增量（13 号票）", () => {
  it("usage 帧记录运行累计与速率；runStarted 记录 run/model 锚点", () => {
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
    for (const frame of frames) buffer.push(frame);
    // scriptedFrames 无 usage 帧：补发（Rust 侧每 turn 收口一帧）
    buffer.push({
      ...frames[0]!,
      type: "usage",
      seq: 99,
      usage: { input: 120, output: 48, cacheRead: 0, cacheWrite: 0 },
      rate: 121,
    } as ChatFrame);
    buffer.push({
      ...frames[0]!,
      type: "usage",
      seq: 100,
      usage: { input: 240, output: 96, cacheRead: 4, cacheWrite: 0 },
      rate: 130,
    } as ChatFrame);
    const snap = buffer.snapshot();
    expect(snap.usage).toEqual({ input: 240, output: 96, cacheRead: 4, cacheWrite: 0 });
    expect(snap.rate).toBe(130);
    expect(snap.run).toEqual({ runId: "run-1", model: "gpt-4o-mock" });
  });

  it("toParts：reasoning/text/tool-call 直写 result（S1 adapter 直写语义）", () => {
    const buffer = new FrameBuffer();
    for (const frame of scriptedFrames()) buffer.push(frame);
    const parts = toParts(buffer.snapshot());
    expect(parts.map((p) => p.type)).toEqual(["reasoning", "text", "tool-call"]);
    const tool = parts[2]!;
    if (tool.type !== "tool-call") throw new Error("应为 tool-call");
    expect(tool.toolCallId).toBe("call_1");
    expect(tool.toolName).toBe("read");
    expect(tool.argsText).toBe('{"path": "/etc/app/config.toml", "offset": 0}');
    // args 含 partial-parse 元数据 Symbol（assistant-stream 官方解析器，S1 实证）——
    // 比对数据键值，元数据由 useToolArgsStatus 消费。
    expect(tool.args).toMatchObject({ path: "/etc/app/config.toml", offset: 0 });
    // result 直写（local runtime 无 execute 管线，S1 修正）
    expect(tool.result).toEqual({ ok: true, summary: "[read] 共 12 行：port = 8087" });
    expect(tool.isError).toBe(false);
  });

  it("toParts：失败工具 isError=true；半截参数以 argsText 原文保留", () => {
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
    for (const frame of frames.slice(0, 9)) buffer.push(frame); // 到 toolArgs 为止（无 result）
    const parts = toParts(buffer.snapshot());
    const tool = parts.at(-1)!;
    if (tool.type !== "tool-call") throw new Error("应为 tool-call");
    expect(tool.result).toBeUndefined();
    expect(tool.argsText.endsWith("}")).toBe(true);
    expect(() => JSON.parse(tool.argsText)).not.toThrow();
  });
});
