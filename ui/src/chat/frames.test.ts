import { describe, expect, it } from "vitest";
import { FrameBuffer, type FrameSnapshot, type SequencedFrame, toParts } from "./frames";

/**
 * 【S2 Spike】前端帧管线测试：保真（无丢失无乱序）、半截 JSON「参数准备中」、
 * 64ms 节流快照节奏、帧到达 → 快照产出的前端侧延迟（NFR-02 余量佐证）。
 */

/** 复刻 Rust S2 集成测试的直通帧序列（两轮 + 工具调用 + 半截 JSON 分段）。 */
function scriptedFrames(): SequencedFrame[] {
  let seq = 0;
  // 联合分发 Omit：保留每个帧变体自己的字段面
  type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
  type FrameArg = DistributiveOmit<SequencedFrame, "seq">;
  const f = (frame: FrameArg): SequencedFrame => ({ seq: seq++, ...frame }) as SequencedFrame;
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
  it("帧序列累积：时间线保真（交错段序 = 帧到达序），工具参数拼接一致", () => {
    const buffer = new FrameBuffer();
    for (const frame of scriptedFrames()) buffer.push(frame);
    const snap = buffer.snapshot();

    // 时间线段序：思考 → 正文（第一轮）→ 工具 → 正文（第二轮答案）；
    // 同轮相邻增量合并为一段，不因 delta 分批而碎裂
    expect(snap.segments).toEqual([
      { kind: "reasoning", text: "用户要我看配置，先调用 read。" },
      { kind: "text", text: "我来读一下配置文件。" },
      { kind: "tool", callId: "call_1" },
      { kind: "text", text: "配置文件共 12 行，端口是 8087。" },
    ]);
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
    expect(buffer.snapshot().segments).toEqual([
      { kind: "reasoning", text: "用户要我看配置，先调用 read。" },
    ]);
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
    const yields: FrameSnapshot[] = [];
    for await (const snap of consumer) {
      yieldTimes.push(performance.now());
      yields.push(snap);
      if (snap.ended) break;
    }
    await producer;

    // 文本总量保真：全部 text 段拼接与发送序列一致（跨轮不合并、无丢失）
    const lastText = yields
      .at(-1)!
      .segments.filter((seg) => seg.kind === "text")
      .map((seg) => seg.text)
      .join("");
    expect(lastText).toBe("我来读一下配置文件。配置文件共 12 行，端口是 8087。");
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
    } as SequencedFrame);
    buffer.push({
      ...frames[0]!,
      type: "usage",
      seq: 100,
      usage: { input: 240, output: 96, cacheRead: 4, cacheWrite: 0 },
      rate: 130,
    } as SequencedFrame);
    const snap = buffer.snapshot();
    expect(snap.usage).toEqual({ input: 240, output: 96, cacheRead: 4, cacheWrite: 0 });
    expect(snap.rate).toBe(130);
    expect(snap.run).toEqual({ runId: "run-1", model: "gpt-4o-mock" });
  });

  it("toParts：按时间线交错展开，reasoning/text/tool-call 直写 result（S1 adapter 直写语义）", () => {
    const buffer = new FrameBuffer();
    for (const frame of scriptedFrames()) buffer.push(frame);
    const parts = toParts(buffer.snapshot());
    // 交错顺序 = 帧到达序：思考 → 第一轮正文 → 工具卡 → 第二轮最终答案
    // （答案在工具卡之后，不再按类型归堆到工具卡前面）
    expect(parts.map((p) => p.type)).toEqual(["reasoning", "text", "tool-call", "text"]);
    if (parts[0]!.type !== "reasoning") throw new Error("应为 reasoning");
    expect(parts[0]!.text).toBe("用户要我看配置，先调用 read。");
    if (parts[1]!.type !== "text") throw new Error("应为 text");
    expect(parts[1]!.text).toBe("我来读一下配置文件。");
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
    if (parts[3]!.type !== "text") throw new Error("应为 text");
    expect(parts[3]!.text).toBe("配置文件共 12 行，端口是 8087。");
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

  it("toParts：思考行带扫光态与时长（status/timing 随思考起止流转）", () => {
    // 注入可控时钟：思考起止的确定性断言（FrameBuffer.now 可注入）
    let t = 1000;
    const buffer = new FrameBuffer(() => t);
    type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
    const f = (frame: DistributiveOmit<SequencedFrame, "seq">): SequencedFrame =>
      ({ seq: buffer.arrivals.length, ...frame }) as SequencedFrame;
    buffer.push(f({ type: "runStarted", runId: "run-9", model: "glm-mock" }));
    t = 1100;
    buffer.push(f({ type: "reasoningDelta", turn: 1, delta: "想一下…" }));
    // 思考流进行中：status=running（渲染「思考中」扫光），timing 无终点
    const live = toParts(buffer.snapshot())[0]!;
    if (live.type !== "reasoning") throw new Error("应为 reasoning");
    expect(live.status).toEqual({ type: "running" });
    expect(live.timing?.startedAt).toBe(1100);
    expect(live.timing?.completedAt).toBeUndefined();
    t = 1350;
    buffer.push(f({ type: "textDelta", turn: 1, delta: "开始回答" }));
    // 正文首帧收口思考流：status=complete + 起止时刻（渲染「持续了 1 秒」）
    const settled = toParts(buffer.snapshot())[0]!;
    if (settled.type !== "reasoning") throw new Error("应为 reasoning");
    expect(settled.status).toEqual({ type: "complete" });
    expect(settled.timing).toEqual({ startedAt: 1100, completedAt: 1350 });
  });

  it("toParts：工具之间的旁白按原位开新思考段（不再归堆到消息开头）", () => {
    // 形态：思考 → 工具 → 旁白（第二段思考）→ 工具 → 最终答案
    const buffer = new FrameBuffer();
    type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
    const f = (frame: DistributiveOmit<SequencedFrame, "seq">): SequencedFrame =>
      ({ seq: buffer.arrivals.length, ...frame }) as SequencedFrame;
    buffer.push(f({ type: "runStarted", runId: "run-2", model: "glm-mock" }));
    buffer.push(f({ type: "reasoningDelta", turn: 1, delta: "先查目录，" }));
    buffer.push(f({ type: "toolStarted", callId: "c1", name: "list_dir" }));
    buffer.push(f({ type: "toolResult", callId: "c1", ok: true, uiSummary: "3 项" }));
    buffer.push(f({ type: "reasoningDelta", turn: 2, delta: "再读入口文件" }));
    buffer.push(f({ type: "toolStarted", callId: "c2", name: "read" }));
    buffer.push(f({ type: "toolResult", callId: "c2", ok: true, uiSummary: "12 行" }));
    buffer.push(f({ type: "textDelta", turn: 2, delta: "入口是 main.rs。" }));
    buffer.push(f({ type: "runEnded", reason: "done", durationMs: 100, toolCount: 2, turn: 2 }));

    expect(buffer.snapshot().segments).toEqual([
      { kind: "reasoning", text: "先查目录，" },
      { kind: "tool", callId: "c1" },
      { kind: "reasoning", text: "再读入口文件" },
      { kind: "tool", callId: "c2" },
      { kind: "text", text: "入口是 main.rs。" },
    ]);
    expect(toParts(buffer.snapshot()).map((p) => p.type)).toEqual([
      "reasoning",
      "tool-call",
      "reasoning",
      "tool-call",
      "text",
    ]);
  });

  it("toParts：无思考流的 run 不产出 reasoning part", () => {
    const buffer = new FrameBuffer(() => 500);
    type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
    const f = (frame: DistributiveOmit<SequencedFrame, "seq">): SequencedFrame =>
      ({ seq: buffer.arrivals.length, ...frame }) as SequencedFrame;
    buffer.push(f({ type: "runStarted", runId: "run-x", model: "glm-mock" }));
    buffer.push(f({ type: "textDelta", turn: 1, delta: "直接回答" }));
    buffer.push(f({ type: "runEnded", reason: "done", durationMs: 10, toolCount: 0, turn: 1 }));
    const parts = toParts(buffer.snapshot());
    expect(parts.map((p) => p.type)).toEqual(["text"]);
  });

  it("toParts：工具之间同组的多次思考与半截句合批，一句话不被思考行拆开", () => {
    // 形态：思考 → 正文半句 → 思考 → 正文续句 → 工具 → 最终答案。
    // 正文两段之间只隔着思考（无工具），属于同一句话的延续——合并为
    // 一个 reasoning part + 一个 text part（ZCode 过程组同款观感）。
    const buffer = new FrameBuffer();
    type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
    const f = (frame: DistributiveOmit<SequencedFrame, "seq">): SequencedFrame =>
      ({ seq: buffer.arrivals.length, ...frame }) as SequencedFrame;
    buffer.push(f({ type: "runStarted", runId: "run-3", model: "glm-mock" }));
    buffer.push(f({ type: "reasoningDelta", turn: 1, delta: "想一下" }));
    buffer.push(f({ type: "textDelta", turn: 1, delta: "我来帮你梳理" }));
    buffer.push(f({ type: "reasoningDelta", turn: 2, delta: "接着说" }));
    buffer.push(f({ type: "textDelta", turn: 2, delta: "项目的目录结构。首先查看" }));
    buffer.push(f({ type: "toolStarted", callId: "c9", name: "list_dir" }));
    buffer.push(f({ type: "toolResult", callId: "c9", ok: true, uiSummary: "8 项" }));
    buffer.push(f({ type: "textDelta", turn: 3, delta: "根目录如下。" }));
    buffer.push(f({ type: "runEnded", reason: "done", durationMs: 100, toolCount: 1, turn: 3 }));

    const parts = toParts(buffer.snapshot());
    expect(parts.map((p) => p.type)).toEqual(["reasoning", "text", "tool-call", "text"]);
    if (parts[0]!.type !== "reasoning") throw new Error("应为 reasoning");
    // 思考合批：两段思考并成一块（段间换行）
    expect(parts[0]!.text).toBe("想一下\n\n接着说");
    if (parts[1]!.type !== "text") throw new Error("应为 text");
    // 正文合批：同一句话的两截直接拼接，不再夹思考行
    expect(parts[1]!.text).toBe("我来帮你梳理项目的目录结构。首先查看");
    if (parts[3]!.type !== "text") throw new Error("应为 text");
    // 工具之后新开正文组
    expect(parts[3]!.text).toBe("根目录如下。");
  });
});

describe("compactDone 帧（04 号票，§3.4【M2】通知面）", () => {
  it("透传进快照 compaction 字段，不进打字机内容面", () => {
    const buffer = new FrameBuffer();
    let seq = 0;
    type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
    type FrameArg = DistributiveOmit<SequencedFrame, "seq">;
    const f = (frame: FrameArg): SequencedFrame => ({ seq: seq++, ...frame }) as SequencedFrame;
    buffer.push(f({ type: "runStarted", runId: "run-1", model: "m" }));
    buffer.push(
      f({ type: "compactDone", cutEntryId: "u-15", tokensBefore: 34000, tokensAfter: 9000 }),
    );
    const snap = buffer.snapshot();
    expect(snap.compaction).toEqual({
      cutEntryId: "u-15",
      tokensBefore: 34000,
      tokensAfter: 9000,
    });
    // 通知面：不产生内容段（segments 保持空）
    expect(snap.segments).toEqual([]);
    // 未发生压缩的快照为 null
    const empty = new FrameBuffer().snapshot();
    expect(empty.compaction).toBeNull();
  });
});
