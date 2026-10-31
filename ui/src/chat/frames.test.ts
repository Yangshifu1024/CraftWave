import { describe, expect, it } from "vitest";
import { FrameBuffer, type FrameSnapshot, type SequencedFrame, toParts } from "./frames";

/**
 * 前端帧管线测试：保真（无丢失无乱序）、半截 JSON「参数准备中」、
 * 64ms 节流快照节奏、帧到达 → 快照产出的前端侧延迟佐证、
 * toParts 展开面与并发工具批。
 */

/** 联合分发 Omit：保留每个帧变体自己的字段面。 */
type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
type FrameArg = DistributiveOmit<SequencedFrame, "seq">;

/** 帧构造简写：seq 按 buffer 已收帧数自动分配（对齐真实后端自增行为）。 */
function sequenced(buffer: FrameBuffer, frame: FrameArg): SequencedFrame {
  return { seq: buffer.arrivals.length, ...frame } as SequencedFrame;
}

/** 复刻 Rust S2 集成测试的直通帧序列（两轮 + 工具调用 + 半截 JSON 分段）。 */
function scriptedFrames(): SequencedFrame[] {
  let seq = 0;
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

describe("FrameBuffer（S2 帧管线）", () => {
  it("保真剧本：时间线段序与工具参数拼接、半截 JSON ready 流转、乱序/重复丢弃、usage/compactDone 透传", () => {
    // ---- 全帧累积：交错段序 = 帧到达序，同轮相邻增量合并为一段 ----
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
    for (const frame of frames) buffer.push(frame);
    const snap = buffer.snapshot();
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

    // ---- 半截 JSON 判定：累积中不 ready，收流后 ready（参数准备中态）----
    const partial = new FrameBuffer();
    for (const frame of frames.slice(0, 8)) partial.push(frame);
    expect(FrameBuffer.argsReady(partial.snapshot().toolCalls[0]!.argsJson)).toBe(false);
    partial.push(frames[8]!); // 参数收尾片段
    const ready = partial.snapshot().toolCalls[0]!;
    expect(FrameBuffer.argsReady(ready.argsJson)).toBe(true);
    expect(JSON.parse(ready.argsJson)).toEqual({ path: "/etc/app/config.toml", offset: 0 });

    // ---- 乱序/重复 seq 丢弃（帧序自愈语义）且不影响后续累积 ----
    const outOfOrder = new FrameBuffer();
    outOfOrder.push(frames[0]!); // seq0 runStarted
    outOfOrder.push(frames[2]!); // seq2 第一段思考
    outOfOrder.push({ ...frames[2]!, seq: 2 }); // 重复帧 → 丢弃
    outOfOrder.push({ ...frames[3]!, seq: 1 }); // 倒退帧 → 丢弃
    outOfOrder.push(frames[3]!); // seq3 第二段思考 → 收
    expect(outOfOrder.droppedSeq).toBe(2);
    expect(outOfOrder.snapshot().segments).toEqual([
      { kind: "reasoning", text: "用户要我看配置，先调用 read。" },
    ]);

    // ---- usage 帧记录运行累计与速率；runStarted 记录 run/model 锚点 ----
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
    const withUsage = buffer.snapshot();
    expect(withUsage.usage).toEqual({ input: 240, output: 96, cacheRead: 4, cacheWrite: 0 });
    expect(withUsage.rate).toBe(130);
    expect(withUsage.run).toEqual({ runId: "run-1", model: "gpt-4o-mock" });

    // ---- compactDone 帧：透传进 compaction 字段，不进打字机内容面 ----
    let seq = 0;
    const f = (frame: FrameArg): SequencedFrame => ({ seq: seq++, ...frame }) as SequencedFrame;
    const compactionBuffer = new FrameBuffer();
    compactionBuffer.push(f({ type: "runStarted", runId: "run-1", model: "m" }));
    compactionBuffer.push(
      f({ type: "compactDone", cutEntryId: "u-15", tokensBefore: 34000, tokensAfter: 9000 }),
    );
    const compactSnap = compactionBuffer.snapshot();
    expect(compactSnap.compaction).toEqual({
      cutEntryId: "u-15",
      tokensBefore: 34000,
      tokensAfter: 9000,
    });
    expect(compactSnap.segments).toEqual([]); // 通知面：不产生内容段
    expect(new FrameBuffer().snapshot().compaction).toBeNull(); // 未发生压缩为 null
  });

  it("64ms 节流快照与前端延迟埋点：帧处理亚毫秒，快照间隔不小于节流窗口", async () => {
    // ---- 前端侧延迟：帧到达 → 快照可读，处理开销亚毫秒级 ----
    const latencyBuffer = new FrameBuffer();
    const latencies: number[] = [];
    for (const frame of scriptedFrames()) {
      const t0 = performance.now();
      latencyBuffer.push(frame);
      const snap = latencyBuffer.snapshot(); // 消费侧读取全量快照（替换语义）
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
    // 全链 <100ms：Rust 侧实测 TTFT≈0.4ms + 节流 ≤64ms；
    // 前端 buffer+快照处理亚毫秒，给 React 渲染留出 >35ms 余量。
    expect(max).toBeLessThan(5);

    // ---- 64ms 节流快照：帧按 20ms 间隔到达（快于节流窗口，模拟 Tauri Channel）----
    const buffer = new FrameBuffer();
    const frames = scriptedFrames();
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

  it("toParts 展开面：交错时间线与 result 直写、失败工具与半截参数、思考行 timing、旁白开新段、合批、并发批不串卡", () => {
    // ---- 交错展开：按时间线交错，reasoning/text/tool-call 直写 result（S1 adapter 直写语义）----
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

    // ---- 失败工具 isError=true；半截参数以 argsText 原文保留 ----
    const pending = new FrameBuffer();
    for (const frame of scriptedFrames().slice(0, 9)) pending.push(frame); // 到 toolArgs 为止
    const pendingParts = toParts(pending.snapshot());
    const pendingTool = pendingParts.at(-1)!;
    if (pendingTool.type !== "tool-call") throw new Error("应为 tool-call");
    expect(pendingTool.result).toBeUndefined();
    expect(pendingTool.argsText.endsWith("}")).toBe(true);
    expect(() => JSON.parse(pendingTool.argsText)).not.toThrow();

    // ---- 思考行带扫光态与时长（status/timing 随思考起止流转）----
    // 注入可控时钟：思考起止的确定性断言（FrameBuffer.now 可注入）
    let t = 1000;
    const clocked = new FrameBuffer(() => t);
    clocked.push(sequenced(clocked, { type: "runStarted", runId: "run-9", model: "glm-mock" }));
    t = 1100;
    clocked.push(sequenced(clocked, { type: "reasoningDelta", turn: 1, delta: "想一下…" }));
    // 思考流进行中：status=running（渲染「思考中」扫光），timing 无终点
    const live = toParts(clocked.snapshot())[0]!;
    if (live.type !== "reasoning") throw new Error("应为 reasoning");
    expect(live.status).toEqual({ type: "running" });
    expect(live.timing?.startedAt).toBe(1100);
    expect(live.timing?.completedAt).toBeUndefined();
    t = 1350;
    clocked.push(sequenced(clocked, { type: "textDelta", turn: 1, delta: "开始回答" }));
    // 正文首帧收口思考流：status=complete + 起止时刻（渲染「持续了 1 秒」）
    const settled = toParts(clocked.snapshot())[0]!;
    if (settled.type !== "reasoning") throw new Error("应为 reasoning");
    expect(settled.status).toEqual({ type: "complete" });
    expect(settled.timing).toEqual({ startedAt: 1100, completedAt: 1350 });

    // ---- 工具之间的旁白按原位开新思考段（不再归堆到消息开头）----
    const asides = new FrameBuffer();
    asides.push(sequenced(asides, { type: "runStarted", runId: "run-2", model: "glm-mock" }));
    asides.push(sequenced(asides, { type: "reasoningDelta", turn: 1, delta: "先查目录，" }));
    asides.push(sequenced(asides, { type: "toolStarted", callId: "c1", name: "list_dir" }));
    asides.push(
      sequenced(asides, { type: "toolResult", callId: "c1", ok: true, uiSummary: "3 项" }),
    );
    asides.push(sequenced(asides, { type: "reasoningDelta", turn: 2, delta: "再读入口文件" }));
    asides.push(sequenced(asides, { type: "toolStarted", callId: "c2", name: "read" }));
    asides.push(
      sequenced(asides, { type: "toolResult", callId: "c2", ok: true, uiSummary: "12 行" }),
    );
    asides.push(sequenced(asides, { type: "textDelta", turn: 2, delta: "入口是 main.rs。" }));
    asides.push(
      sequenced(asides, {
        type: "runEnded",
        reason: "done",
        durationMs: 100,
        toolCount: 2,
        turn: 2,
      }),
    );
    expect(asides.snapshot().segments).toEqual([
      { kind: "reasoning", text: "先查目录，" },
      { kind: "tool", callId: "c1" },
      { kind: "reasoning", text: "再读入口文件" },
      { kind: "tool", callId: "c2" },
      { kind: "text", text: "入口是 main.rs。" },
    ]);
    expect(toParts(asides.snapshot()).map((p) => p.type)).toEqual([
      "reasoning",
      "tool-call",
      "reasoning",
      "tool-call",
      "text",
    ]);

    // ---- 无思考流的 run 不产出 reasoning part ----
    const plain = new FrameBuffer(() => 500);
    plain.push(sequenced(plain, { type: "runStarted", runId: "run-x", model: "glm-mock" }));
    plain.push(sequenced(plain, { type: "textDelta", turn: 1, delta: "直接回答" }));
    plain.push(
      sequenced(plain, { type: "runEnded", reason: "done", durationMs: 10, toolCount: 0, turn: 1 }),
    );
    expect(toParts(plain.snapshot()).map((p) => p.type)).toEqual(["text"]);

    // ---- 工具之间同组的多次思考与半截句合批：一句话不被思考行拆开 ----
    // 形态：思考 → 正文半句 → 思考 → 正文续句 → 工具 → 最终答案。
    // 正文两段之间只隔着思考（无工具），属于同一句话的延续——合并为
    // 一个 reasoning part + 一个 text part（ZCode 过程组同款观感）。
    const batched = new FrameBuffer();
    batched.push(sequenced(batched, { type: "runStarted", runId: "run-3", model: "glm-mock" }));
    batched.push(sequenced(batched, { type: "reasoningDelta", turn: 1, delta: "想一下" }));
    batched.push(sequenced(batched, { type: "textDelta", turn: 1, delta: "我来帮你梳理" }));
    batched.push(sequenced(batched, { type: "reasoningDelta", turn: 2, delta: "接着说" }));
    batched.push(
      sequenced(batched, { type: "textDelta", turn: 2, delta: "项目的目录结构。首先查看" }),
    );
    batched.push(sequenced(batched, { type: "toolStarted", callId: "c9", name: "list_dir" }));
    batched.push(
      sequenced(batched, { type: "toolResult", callId: "c9", ok: true, uiSummary: "8 项" }),
    );
    batched.push(sequenced(batched, { type: "textDelta", turn: 3, delta: "根目录如下。" }));
    batched.push(
      sequenced(batched, {
        type: "runEnded",
        reason: "done",
        durationMs: 100,
        toolCount: 1,
        turn: 3,
      }),
    );
    const batchParts = toParts(batched.snapshot());
    expect(batchParts.map((p) => p.type)).toEqual(["reasoning", "text", "tool-call", "text"]);
    if (batchParts[0]!.type !== "reasoning") throw new Error("应为 reasoning");
    // 思考合批：两段思考并成一块（段间换行）
    expect(batchParts[0]!.text).toBe("想一下\n\n接着说");
    if (batchParts[1]!.type !== "text") throw new Error("应为 text");
    // 正文合批：同一句话的两截直接拼接，不再夹思考行
    expect(batchParts[1]!.text).toBe("我来帮你梳理项目的目录结构。首先查看");
    if (batchParts[3]!.type !== "text") throw new Error("应为 text");
    // 工具之后新开正文组
    expect(batchParts[3]!.text).toBe("根目录如下。");

    // ---- 并发工具批：同批多卡各自推进，结果与自己的 callId 配对 ----
    // 帧序（Rust 侧受控并发的真实交错形态）：同批三个工具的 started/args 流式
    // 帧交错到达，结果帧按源序（B 先完成、A 最慢）。
    const parallel = new FrameBuffer();
    parallel.push(
      sequenced(parallel, { type: "runStarted", runId: "run-p", model: "gpt-4o-mock" }),
    );
    parallel.push(sequenced(parallel, { type: "stepStarted", turn: 1 }));
    parallel.push(sequenced(parallel, { type: "textDelta", turn: 1, delta: "一次交代三件事。" }));
    parallel.push(
      sequenced(parallel, { type: "toolStarted", callId: "call_part_1_1", name: "read" }),
    );
    parallel.push(
      sequenced(parallel, { type: "toolStarted", callId: "call_part_1_2", name: "grep" }),
    );
    parallel.push(
      sequenced(parallel, { type: "toolStarted", callId: "call_part_1_3", name: "read" }),
    );
    parallel.push(
      sequenced(parallel, {
        type: "toolArgs",
        callId: "call_part_1_1",
        chunk: '{"path": "a.txt"}',
      }),
    );
    parallel.push(
      sequenced(parallel, {
        type: "toolArgs",
        callId: "call_part_1_2",
        chunk: '{"pattern": "foo"',
      }),
    );
    parallel.push(
      sequenced(parallel, {
        type: "toolArgs",
        callId: "call_part_1_3",
        chunk: '{"path": "b.txt"}',
      }),
    );
    parallel.push(
      sequenced(parallel, { type: "toolArgs", callId: "call_part_1_2", chunk: ', "path": "src"}' }),
    );
    // 结果按源序（B 最快完成也不能插到 A 前——A 是源序第一）
    parallel.push(
      sequenced(parallel, {
        type: "toolResult",
        callId: "call_part_1_2",
        ok: true,
        uiSummary: "[grep] 3 处命中",
      }),
    );
    parallel.push(
      sequenced(parallel, {
        type: "toolResult",
        callId: "call_part_1_3",
        ok: true,
        uiSummary: "[read] B 文件",
      }),
    );
    parallel.push(
      sequenced(parallel, {
        type: "toolResult",
        callId: "call_part_1_1",
        ok: false,
        uiSummary: "未执行：等待用户回答后重新决定",
      }),
    );
    parallel.push(
      sequenced(parallel, {
        type: "runEnded",
        reason: "done",
        durationMs: 420,
        toolCount: 3,
        turn: 1,
      }),
    );

    const parallelSnap = parallel.snapshot();
    // 三张工具卡独立入列（toolStarted 位置即时间线位置，与到达序一致）
    expect(parallelSnap.segments.filter((s) => s.kind === "tool").map((s) => s.callId)).toEqual([
      "call_part_1_1",
      "call_part_1_2",
      "call_part_1_3",
    ]);
    const byId = new Map(parallelSnap.toolCalls.map((tc) => [tc.callId, tc]));
    expect(byId.size).toBe(3);
    // 各卡参数只进自己的卡片：grep 的半截 JSON 不串到 read 卡上
    expect(byId.get("call_part_1_1")!.argsJson).toBe('{"path": "a.txt"}');
    expect(byId.get("call_part_1_2")!.argsJson).toBe('{"pattern": "foo", "path": "src"}');
    expect(byId.get("call_part_1_3")!.argsJson).toBe('{"path": "b.txt"}');
    // 各卡结果与自己的 callId 配对（B 先完成不影响 A/C 的状态归属）
    expect(byId.get("call_part_1_1")!.result).toEqual({
      ok: false,
      uiSummary: "未执行：等待用户回答后重新决定",
    });
    expect(byId.get("call_part_1_2")!.result?.uiSummary).toBe("[grep] 3 处命中");
    expect(byId.get("call_part_1_3")!.result?.uiSummary).toBe("[read] B 文件");
    expect(parallelSnap.ended?.toolCount).toBe(3);
    // toParts：并发批的工具卡各自带自己的 args/result（isError 按 ok 归属）
    const toolParts = toParts(parallelSnap).filter((p) => p.type === "tool-call");
    expect(toolParts).toHaveLength(3);
    const first = toolParts[0]!;
    expect(first).toMatchObject({
      type: "tool-call",
      toolCallId: "call_part_1_1",
      isError: true,
    });
    expect(first.argsText).toBe('{"path": "a.txt"}');
    const grep = toolParts.find((p) => p.toolCallId === "call_part_1_2")!;
    expect(grep).toMatchObject({ isError: false });
    expect(grep.argsText).toBe('{"pattern": "foo", "path": "src"}');
  });
});
