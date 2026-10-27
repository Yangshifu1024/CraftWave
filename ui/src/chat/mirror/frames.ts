/**
 * 【S1 spike 原型，随时可弃】镜像工具桥接的帧协议与累积器。
 *
 * 帧类型对齐技术设计文档 §10.2 的 Channel 帧（ToolStart / ToolProgress / ToolResult），
 * 这里只取镜像工具验证所需的最小子集。真实帧协议字段以 S2/S3 票定稿为准。
 */
import { parsePartialJsonObject, type ReadonlyJSONObject } from "assistant-stream/utils";

/** ToolStart：后端宣布开始一次工具调用（参数可能还没到齐）。 */
export type ToolStartFrame = {
  type: "tool-start";
  seq: number;
  toolCallId: string;
  toolName: string;
};

/**
 * ToolProgress：参数增量（半截 JSON 文本，累积后才是完整 argsText）。
 * 对齐设计文档「含半截 JSON 参数增量」。
 */
export type ToolProgressFrame = {
  type: "tool-progress";
  seq: number;
  toolCallId: string;
  argsTextDelta: string;
};

/** ToolResult：后端执行完成（isError 区分错误态）。 */
export type ToolResultFrame = {
  type: "tool-result";
  seq: number;
  toolCallId: string;
  result: unknown;
  isError?: boolean;
};

/** 文本增量（让剧本里工具调用前后有正文，贴近真实对话）。 */
export type TextDeltaFrame = {
  type: "text-delta";
  seq: number;
  delta: string;
};

/** run 结束标记。 */
export type RunEndedFrame = {
  type: "run-ended";
  seq: number;
};

export type MirrorFrame =
  | ToolStartFrame
  | ToolProgressFrame
  | ToolResultFrame
  | TextDeltaFrame
  | RunEndedFrame;

/**
 * assistant-ui tool-call part 的可序列化子集（ChatModelRunResult.content 用）。
 * result 出现即触发卡片「完成/错误」态；args 嵌 partial-parse 元数据，
 * useToolArgsStatus 由此区分「参数准备中」与「运行中」。
 */
export type MirrorToolPart = {
  type: "tool-call";
  toolCallId: string;
  toolName: string;
  argsText: string;
  args: ReadonlyJSONObject;
  result?: unknown;
  isError?: boolean;
};

export type MirrorTextPart = {
  type: "text";
  text: string;
};

export type MirrorPart = MirrorToolPart | MirrorTextPart;

/**
 * 帧累积器：收帧 → 内部累积 → toParts() 输出全量累计快照。
 * assistant-ui 语义：每次 yield 替换整个 content，parts 顺序按首次出现定，
 * 以 toolCallId 对齐增量——这正是「不重建」的前提（React key 稳定）。
 */
export class FrameBuffer {
  private text = "";
  private readonly tools = new Map<string, MirrorToolPart>();
  private readonly order: string[] = [];

  push(frame: MirrorFrame): void {
    switch (frame.type) {
      case "text-delta": {
        this.text += frame.delta;
        break;
      }
      case "tool-start": {
        if (!this.tools.has(frame.toolCallId)) {
          this.tools.set(frame.toolCallId, {
            type: "tool-call",
            toolCallId: frame.toolCallId,
            toolName: frame.toolName,
            argsText: "",
            args: {},
          });
          this.order.push(frame.toolCallId);
        }
        break;
      }
      case "tool-progress": {
        const part = this.tools.get(frame.toolCallId);
        if (part) {
          part.argsText += frame.argsTextDelta;
          // 半截 JSON 也解析：args 带上 partial 元数据（complete/partial），
          // 供 useToolArgsStatus 区分「参数准备中」与「运行中」。
          part.args = parsePartialJsonObject(part.argsText) ?? {};
        }
        break;
      }
      case "tool-result": {
        const part = this.tools.get(frame.toolCallId);
        if (part) {
          // result 兜底前把 args 定稿（半截时按原文补一个完整解析尝试）。
          part.args = parsePartialJsonObject(part.argsText) ?? {};
          part.result = frame.result;
          part.isError = frame.isError;
        }
        break;
      }
      case "run-ended":
        break;
    }
  }

  /** 全量累计快照：文本 part 在前（若有），工具 part 按出现顺序。 */
  toParts(): MirrorPart[] {
    const parts: MirrorPart[] = [];
    if (this.text.length > 0) {
      parts.push({ type: "text", text: this.text });
    }
    for (const id of this.order) {
      const part = this.tools.get(id);
      if (part) parts.push(part);
    }
    return parts;
  }
}

/** argsText 是否已是完整 JSON（对齐 ToolInvocationTracker 的 isArgsTextComplete 判据）。 */
export function isArgsTextComplete(argsText: string): boolean {
  try {
    JSON.parse(argsText);
    return true;
  } catch {
    return false;
  }
}
