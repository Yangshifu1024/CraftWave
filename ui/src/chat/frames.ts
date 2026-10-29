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

/**
 * 快照中的一段时间线内容（按帧到达顺序排列）。模型的真实输出是交错的
 * （思考 → 工具 → 旁白 → 工具 → 最终答案），旧实现把所有文本/思考各拼进
 * 一个字符串，交错顺序在此丢失——显示上旁白全堆到消息开头、最终答案
 * 永远排在工具卡之前。改为段列表后，每类增量只在「上一段同为该类型」
 * 时续接，否则新开一段，快照即真实时间线。
 * reasoning/text 段携带各自的累积文本；tool 段只记出现位置，调用状态在
 * toolCalls 表（按 callId 关联）。
 */
export type SnapshotSegment =
  | { kind: "reasoning"; text: string }
  | { kind: "text"; text: string }
  | { kind: "tool"; callId: string };

/** 全量累计快照（assistant-ui 语义：替换而非增量）。 */
export interface FrameSnapshot {
  /** 内容时间线（帧到达序）：toParts 按此展开成交错 parts。 */
  segments: SnapshotSegment[];
  toolCalls: ToolCallState[];
  started: boolean;
  ended: null | {
    reason: "done" | "error" | "cancelled";
    durationMs: number;
    toolCount: number;
    /** 错误终态的错误载荷（run:error 帧 / 命令面合成帧），线程错误态展示用。 */
    error?: { code: string; message: string };
  };
  /** runStarted 帧信息（帧到达前为 null）。 */
  run: RunInfo | null;
  /** usage 帧累计（§10.5 runMetrics 口径：运行内累计）。 */
  usage: { input: number; output: number; cacheRead: number; cacheWrite: number };
  /** 最近一次 usage 帧的输出速率（tok/s；无则 null）。 */
  rate: number | null;
  /**
   * 思考流的起止时刻（performance.now() 毫秒）：首帧 reasoningDelta 记起点，
   * 首帧 textDelta / toolStarted / runEnded 记终点——null = 本 run 没有思考流。
   * 思考行「思考中」扫光态与「持续了 N 秒」的数据面。
   */
  thinking: { startedAt: number; endedAt: number | null } | null;
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
  /** 内容时间线：text/reasoning 增量续接同类末段或新开段，工具按 toolStarted 位置入列。 */
  private segments: SnapshotSegment[] = [];
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
  private thinkingStartedAt: number | null = null;
  private thinkingEndedAt: number | null = null;
  private maxSeq = -1;
  /** 乱序/重复帧丢弃计数（对账协议的健康度）。 */
  droppedSeq = 0;
  readonly arrivals: FrameArrival[] = [];

  constructor(private readonly now: () => number = () => performance.now()) {}

  /**
   * 收一帧并入累积态。乱序保护：帧序号 seq 必须大于已见最大序号，
   * 重复或倒退的帧直接丢弃（后端一次只发一个 run 的帧，序号单调递增）。
   * 每帧同时记录到达时刻埋点（供延迟统计）。
   */
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
      case "textDelta": {
        this.appendSegment("text", frame.delta);
        // 首个正文帧 = 思考流结束点（帧序：思考 → 工具/正文）
        if (this.thinkingStartedAt !== null && this.thinkingEndedAt === null) {
          this.thinkingEndedAt = this.now();
        }
        break;
      }
      case "reasoningDelta": {
        this.appendSegment("reasoning", frame.delta);
        // 首帧思考 = 思考流起点
        if (this.thinkingStartedAt === null) {
          this.thinkingStartedAt = this.now();
        }
        break;
      }
      case "toolStarted": {
        this.tools.set(frame.callId, { callId: frame.callId, name: frame.name, argsJson: "" });
        this.segments.push({ kind: "tool", callId: frame.callId });
        // 首个工具帧同样收口思考流（思考后先调工具再出正文）
        if (this.thinkingStartedAt !== null && this.thinkingEndedAt === null) {
          this.thinkingEndedAt = this.now();
        }
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
          error: frame.error,
        };
        // 只有思考没有正文/工具的 run：思考流在 runEnded 收口
        if (this.thinkingStartedAt !== null && this.thinkingEndedAt === null) {
          this.thinkingEndedAt = this.now();
        }
        break;
      default:
        break;
    }
  }

  /**
   * 增量并入时间线：末段同为该类型则续接（同一发言块不拆段），否则新开一段。
   * 空 delta 直接忽略，避免产出空段。
   */
  private appendSegment(kind: "text" | "reasoning", delta: string): void {
    if (delta.length === 0) return;
    const last = this.segments.at(-1);
    if (last !== undefined && last.kind === kind) {
      last.text += delta;
    } else {
      this.segments.push({ kind, text: delta });
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

  /** 深拷贝当前累积态为一份完整快照（替换语义：每次都是全量内容）。 */
  snapshot(): FrameSnapshot {
    return {
      segments: this.segments.map((seg) => ({ ...seg })),
      toolCalls: [...this.tools.values()].map((tool) => ({ ...tool })),
      started: this.started,
      ended: this.ended ? { ...this.ended } : null,
      run: this.run ? { ...this.run } : null,
      usage: { ...this.usage },
      rate: this.rate,
      thinking:
        this.thinkingStartedAt === null
          ? null
          : { startedAt: this.thinkingStartedAt, endedAt: this.thinkingEndedAt },
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

/** assistant-ui 消息 part 的可序列化子集（ChatModelRunResult.content 用）。
 *  reasoning 的 status/timing 对齐 ReasoningMessagePart 可选字段（思考行
 *  「思考中」扫光态与「持续了 N 秒」的数据面）。 */
export type ChatPart =
  | {
      type: "text";
      text: string;
    }
  | {
      type: "reasoning";
      text: string;
      status?: { type: "running" | "complete" };
      timing?: { startedAt: number; completedAt?: number };
    }
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
 * 快照 → content parts：以工具为界把时间线分段——两次工具之间的 reasoning
 * 全部并入组头思考段、text 全部并入组正文段（按到达序直接拼接）。
 *
 * 为什么合批：模型在两次工具之间会「想一下、说半句」反复多次，逐段展开的
 * 话一句话会被思考行拦腰拆成几截（「我来帮你梳理」/「…首先查看」是同一句），
 * 过程区全是碎片；同组合批后就是一条思考行 + 一句连贯的话 + 工具行。
 * 顺序保持帧到达序，工具永远在两组之间作为分界。
 */
export function toParts(snapshot: FrameSnapshot): ChatPart[] {
  const toolById = new Map(snapshot.toolCalls.map((tool) => [tool.callId, tool]));
  // 思考行状态面（run 级思考起止，合并后的思考段共用）：流式中 status=running
  // （渲染「思考中」扫光），结束后 complete + 起止时刻（渲染「持续了 N 秒」）
  const thinking = snapshot.thinking;
  const reasoningState = {
    status:
      thinking !== null && thinking.endedAt === null ? { type: "running" } : { type: "complete" },
    ...(thinking !== null
      ? {
          timing: {
            startedAt: thinking.startedAt,
            ...(thinking.endedAt !== null ? { completedAt: thinking.endedAt } : {}),
          },
        }
      : {}),
  } satisfies Pick<Extract<ChatPart, { type: "reasoning" }>, "status" | "timing">;
  const parts: ChatPart[] = [];
  let groupReasoning: string[] = [];
  let groupText: string[] = [];
  const flushGroup = () => {
    const reasoningText = groupReasoning.join("\n\n");
    if (reasoningText.length > 0) {
      parts.push({ type: "reasoning", text: reasoningText, ...reasoningState });
    }
    const groupTextJoined = groupText.join("");
    if (groupTextJoined.length > 0) {
      parts.push({ type: "text", text: groupTextJoined });
    }
    groupReasoning = [];
    groupText = [];
  };
  for (const seg of snapshot.segments) {
    if (seg.kind === "reasoning") {
      if (seg.text.length > 0) groupReasoning.push(seg.text);
    } else if (seg.kind === "text") {
      if (seg.text.length > 0) groupText.push(seg.text);
    } else {
      flushGroup();
      const tool = toolById.get(seg.callId);
      if (tool) {
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
    }
  }
  flushGroup();
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
