/**
 * 【S2 Spike】ChatFrame 类型与 FrameBuffer（§3.4 帧 + §10.2 累积器语义）。
 *
 * 技术设计文档 §10.2：channel.onmessage 把帧推入 FrameBuffer，消费侧以
 * 64ms 节流快照迭代器取全量累计快照 yield 给 assistant-ui（替换语义）。
 * 本文件同时承担 NFR-02 前端侧延迟埋点（帧到达 → 快照产出）。
 */

/** 帧基座：所有帧带 Rust 端递增的 seq（§3.4）。 */
export interface Sequenced {
  seq: number;
}

export type ChatFrame =
  | (Sequenced & { type: "runStarted"; runId: string; model: string })
  | (Sequenced & { type: "stepStarted"; turn: number })
  | (Sequenced & { type: "textDelta"; turn: number; delta: string })
  | (Sequenced & { type: "reasoningDelta"; turn: number; delta: string })
  | (Sequenced & { type: "toolStarted"; callId: string; name: string })
  | (Sequenced & { type: "toolArgs"; callId: string; chunk: string })
  | (Sequenced & {
      type: "toolResult";
      callId: string;
      ok: boolean;
      uiSummary: string;
    })
  | (Sequenced & {
      type: "runEnded";
      reason: "done" | "error" | "cancelled";
      durationMs: number;
      toolCount: number;
      turn: number;
    });

/** 一次工具调用的累积态（镜像工具卡渲染面，§10.3）。 */
export interface ToolCallState {
  callId: string;
  name: string;
  /** 参数 JSON 片段累积；半截 JSON 原样保留（卡片以「参数准备中」渲染）。 */
  argsJson: string;
  result?: { ok: boolean; uiSummary: string };
}

/** 全量累计快照（assistant-ui 语义：替换而非增量）。 */
export interface FrameSnapshot {
  text: string;
  reasoning: string;
  toolCalls: ToolCallState[];
  started: boolean;
  ended: null | { reason: "done" | "error" | "cancelled"; durationMs: number; toolCount: number };
}

/** 单帧到达的埋点记录（NFR-02 前端侧）。 */
export interface FrameArrival {
  seq: number;
  /** 帧离开 Rust 侧的时刻（由帧负载携带或外部注入；spike 用相对时刻）。 */
  rustAt: number;
  /** 前端收到帧的时刻（performance.now()）。 */
  arrivedAt: number;
}

/**
 * 帧累积器：parts 数组 + 乱序丢弃（§5.6 runEpoch 语义）+ 埋点。
 * spike 里以注入时钟保持确定性；生产接线时 rustAt 可由帧尾附带的
 * Rust 出口时刻算得（见报告的帧协议修订清单）。
 */
export class FrameBuffer {
  private text = "";
  private reasoning = "";
  private tools = new Map<string, ToolCallState>();
  private started = false;
  private ended: FrameSnapshot["ended"] = null;
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
    this.arrivals.push({ seq: frame.seq, rustAt: 0, arrivedAt: this.now() });
    switch (frame.type) {
      case "runStarted":
        this.started = true;
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
