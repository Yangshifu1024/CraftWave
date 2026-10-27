/**
 * 帧消费层（§3.4 帧 + §10.2 累积器语义）。
 *
 * ChatFrame 契约类型已归位 `ipc/types.ts`（§3.4 与 Rust backend/frames.rs 镜像，
 * 本文件 re-export 保持既有引用不破坏）。技术设计文档 §10.2：channel.onmessage
 * 把帧推入 FrameBuffer，消费侧以 64ms 节流快照迭代器取全量累计快照 yield 给
 * assistant-ui（替换语义）。本文件同时承担 NFR-02 前端侧延迟埋点（帧到达 →
 * 快照产出）。
 */
import type { ChatFrame } from "@/ipc/types";

export type { ChatFrame };

/** 一次工具调用的累积态（镜像工具卡渲染面，§10.3）。 */
export interface ToolCallState {
  callId: string;
  name: string;
  /** 参数 JSON 片段累积；半截 JSON 原样保留（卡片以「参数准备中」渲染）。 */
  argsJson: string;
  result?: { ok: boolean; uiSummary: string };
}

/** run 起手信息（runStarted 帧；runMetrics 与折叠分组锚点用）。 */
export interface RunInfo {
  runId: string;
  model: string;
}

/** 全量累计快照（assistant-ui 语义：替换而非增量）。 */
export interface FrameSnapshot {
  text: string;
  reasoning: string;
  toolCalls: ToolCallState[];
  started: boolean;
  ended: null | {
    reason: "done" | "error" | "cancelled";
    durationMs: number;
    toolCount: number;
  };
  /** runStarted 帧信息（帧到达前为 null）。 */
  run: RunInfo | null;
  /** usage 帧累计（§10.5 runMetrics 口径：运行内累计）。 */
  usage: { input: number; output: number; cacheRead: number; cacheWrite: number };
  /** 最近一次 usage 帧的输出速率（tok/s；无则 null）。 */
  rate: number | null;
}

/** 单帧到达的埋点记录（NFR-02 前端侧）。 */
export interface FrameArrival {
  seq: number;
  /** 帧离开 Rust 侧的时刻（帧负载 ts；缺省 0）。 */
  rustAt: number;
  /** 前端收到帧的时刻（performance.now()）。 */
  arrivedAt: number;
}

/**
 * 帧累积器：parts 数组 + 乱序丢弃（§5.6 runEpoch 语义）+ 埋点。
 */
export class FrameBuffer {
  private text = "";
  private reasoning = "";
  private tools = new Map<string, ToolCallState>();
  private started = false;
  private ended: FrameSnapshot["ended"] = null;
  private run: RunInfo | null = null;
  private usage: FrameSnapshot["usage"] = {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
  };
  private rate: number | null = null;
  private maxSeq = -1;
  /** 乱序/重复帧丢弃计数（对账协议的健康度）。 */
  droppedSeq = 0;
  readonly arrivals: FrameArrival[] = [];

  constructor(private readonly now: () => number = () => performance.now()) {}

  push(frame: ChatFrame): void {
    // §5.6：前端持有 runEpoch，seq 必须 > 已见最大，否则丢弃
    if (frame.seq <= this.maxSeq) {
      this.droppedSeq += 1;
      return;
    }
    this.maxSeq = frame.seq;
    this.arrivals.push({ seq: frame.seq, rustAt: frame.ts ?? 0, arrivedAt: this.now() });
    switch (frame.type) {
      case "runStarted":
        this.started = true;
        this.run = { runId: frame.runId, model: frame.model };
        break;
      case "textDelta":
        this.text += frame.delta;
        break;
      case "reasoningDelta":
        this.reasoning += frame.delta;
        break;
      case "toolStarted": {
        this.tools.set(frame.callId, { callId: frame.callId, name: frame.name, argsJson: "" });
        break;
      }
      case "toolArgs": {
        const tool = this.tools.get(frame.callId);
        if (tool) tool.argsJson += frame.chunk;
        break;
      }
      case "toolResult": {
        const tool = this.tools.get(frame.callId);
        if (tool) tool.result = { ok: frame.ok, uiSummary: frame.uiSummary };
        break;
      }
      case "usage": {
        this.usage = { ...frame.usage };
        this.rate = frame.rate ?? this.rate;
        break;
      }
      case "runEnded":
        this.ended = {
          reason: frame.reason,
          durationMs: frame.durationMs,
          toolCount: frame.toolCount,
        };
        break;
      default:
        break;
    }
  }

  /** 参数是否仍在流式累积（半截 JSON 判定：解析失败即「参数准备中」）。 */
  static argsReady(argsJson: string): boolean {
    try {
      const parsed: unknown = JSON.parse(argsJson);
      return typeof parsed === "object" && parsed !== null;
    } catch {
      return false;
    }
  }

  snapshot(): FrameSnapshot {
    return {
      text: this.text,
      reasoning: this.reasoning,
      toolCalls: [...this.tools.values()].map((tool) => ({ ...tool })),
      started: this.started,
      ended: this.ended ? { ...this.ended } : null,
      run: this.run ? { ...this.run } : null,
      usage: { ...this.usage },
      rate: this.rate,
    };
  }

  /**
   * 64ms 节流快照迭代器（§10.2）：帧推送由外部 onmessage 驱动，本迭代器
   * 每 intervalMs 产出一个全量快照，直到 runEnded 且窗口排空。
   */
  async *snapshots(intervalMs = 64): AsyncGenerator<FrameSnapshot> {
    const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    while (true) {
      await sleep(intervalMs);
      const snap = this.snapshot();
      if (snap.ended) {
        yield snap;
        return;
      }
      yield snap;
    }
  }
}

// ---- toParts：快照 → assistant-ui content parts（§10.2/§10.3，adapter 直写 result） ----

import type { ToolCallMessagePart } from "@assistant-ui/react";

/** assistant-ui 消息 part 的可序列化子集（ChatModelRunResult.content 用）。 */
export type ChatPart =
  | { type: "text"; text: string }
  | { type: "reasoning"; text: string }
  | ToolCallMessagePart;

/** argsText 是否已是完整 JSON（「参数准备中」→「运行中」的判定）。 */
export function isArgsTextComplete(argsText: string): boolean {
  try {
    JSON.parse(argsText);
    return true;
  } catch {
    return false;
  }
}

/**
 * 快照 → content parts：text/reasoning → 对应 part；toolCalls → 镜像工具的
 * tool-call part（S1 结论：adapter 把 toolResult 帧直接写进 result/isError，
 * local runtime 无 execute 管线）。顺序 = 帧到达序（工具按 toolStarted 出现序，
 * 文本/思考在前）。
 */
export function toParts(snapshot: FrameSnapshot): ChatPart[] {
  const parts: ChatPart[] = [];
  if (snapshot.reasoning.length > 0) {
    parts.push({ type: "reasoning", text: snapshot.reasoning });
  }
  if (snapshot.text.length > 0) {
    parts.push({ type: "text", text: snapshot.text });
  }
  for (const tool of snapshot.toolCalls) {
    parts.push({
      type: "tool-call",
      toolCallId: tool.callId,
      toolName: tool.name,
      argsText: tool.argsJson,
      args: parsePartialJsonObject(tool.argsJson),
      ...(tool.result
        ? {
            result: { ok: tool.result.ok, summary: tool.result.uiSummary },
            isError: !tool.result.ok,
          }
        : {}),
    });
  }
  return parts;
}

/**
 * 半截 JSON 的宽松解析（「参数准备中」态产出含已解析前缀的 args）。
 * assistant-stream 的官方解析器（S1 实证：含 partial-parse 元数据，
 * useToolArgsStatus 依赖之；ui 已显式声明 assistant-stream 依赖）。
 * 返回 ReadonlyJSONObject（assistant-ui tool-call part 的 args 类型面）。
 */
import {
  parsePartialJsonObject as parsePartial,
  type ReadonlyJSONObject,
} from "assistant-stream/utils";

/** 半截 JSON 的宽松解析（空/不可解析 → 空对象，「参数准备中」态）。 */
export function parsePartialJsonObject(argsText: string): ReadonlyJSONObject {
  const parsed = parsePartial(argsText);
  if (typeof parsed === "object" && parsed !== null) {
    return parsed;
  }
  return {};
}
