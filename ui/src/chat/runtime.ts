import type { ChatModelAdapter, ThreadMessage } from "@assistant-ui/react";

/**
 * S6 骨架用回声适配器：不接真实模型（rig 流式接入属 S2/S3 票），
 * 但以两段累积 yield 模拟流式渲染，验证 assistant-ui 流水线可用。
 */
export const REPLY_PREFIX = "【骨架回声】";

/** 把回复切成 n 段（每段非空），供分批累积 yield。 */
export function splitReply(reply: string, parts = 2): string[] {
  const size = Math.max(1, Math.ceil(reply.length / parts));
  const chunks: string[] = [];
  for (let i = 0; i < reply.length; i += size) {
    chunks.push(reply.slice(i, i + size));
  }
  return chunks;
}

export function promptFrom(messages: readonly ThreadMessage[]): string {
  const last = messages.at(-1);
  const text = last?.content.map((part) => (part.type === "text" ? part.text : "")).join("") ?? "";
  return text.length > 0 ? text : "（空消息）";
}

export async function* echoStream(
  prompt: string,
  signal: AbortSignal,
): AsyncGenerator<{ content: [{ type: "text"; text: string }] }> {
  const reply = `${REPLY_PREFIX}${prompt}`;
  let acc = "";
  for (const chunk of splitReply(reply)) {
    if (signal.aborted) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 120));
    acc += chunk;
    yield { content: [{ type: "text", text: acc }] };
  }
}

export const echoAdapter: ChatModelAdapter = {
  async *run({ messages, abortSignal }) {
    yield* echoStream(promptFrom(messages), abortSignal);
  },
};
