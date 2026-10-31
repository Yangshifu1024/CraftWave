/**
 * 【S1 spike 原型，随时可弃】镜像工具桥接的 runtime 侧：
 * ChatModelAdapter 消费 mock 帧源 → FrameBuffer 累积 → 全量累计快照 yield。
 *
 * 对齐「每次 yield 为全量累计快照（替换而非增量）」语义；
 * 64ms 节流属 S2/S3 优化，原型按帧节拍直出。
 */
import type { ChatModelAdapter, ChatModelRunResult } from "@assistant-ui/react";
import type { MirrorPart } from "./frames";
import { FrameBuffer } from "./frames";
import { createFrameSource } from "./mockSource";

/** parts 包一层 ChatModelRunResult（assistant-ui run 的产出形态）。 */
function toRunResult(parts: MirrorPart[]): ChatModelRunResult {
  return { content: parts };
}

/** 镜像原型 adapter：每收一帧就产出一个全量累计快照（原型按帧节拍直出，
 *  不做 64ms 节流）；剧本播完后显式产出 complete 终态。 */
export const mirrorAdapter: ChatModelAdapter = {
  async *run({ abortSignal }) {
    const buffer = new FrameBuffer();
    for await (const frame of createFrameSource(abortSignal)) {
      if (abortSignal.aborted) return;
      buffer.push(frame);
      yield toRunResult(buffer.toParts());
    }
    // 剧本播完：显式 complete，避免运行时按 running 兜底推断
    yield { ...toRunResult(buffer.toParts()), status: { type: "complete", reason: "stop" } };
  },
};
