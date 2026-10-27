import type { ChatModelAdapter, ChatModelRunResult } from "@assistant-ui/react";

/**
 * S4 spike 用脚本化假流后端（ScriptedBackend）：
 * 不依赖真实模型与 API key，按预置剧本逐段累积 yield 模拟流式。
 * 剧本队列先进先出——第 n 次 run 消费第 n 个剧本，供多轮对话测试编排。
 */
export type ReplyScript = {
  /** 先输出的 reasoning 增量段（可选，用于验证 reasoning part 的 roundtrip） */
  reasoning?: string[];
  /** 随后输出的 text 增量段（每段非空，累积语义） */
  text: string[];
};

/** 把一段文本切成 n 个非空增量（测试辅助） */
export function splitChunks(text: string, parts = 2): string[] {
  const size = Math.max(1, Math.ceil(text.length / parts));
  const chunks: string[] = [];
  for (let i = 0; i < text.length; i += size) {
    chunks.push(text.slice(i, i + size));
  }
  return chunks;
}

export function createScriptedAdapter(scripts: readonly ReplyScript[]): ChatModelAdapter {
  const queue = [...scripts];
  return {
    async *run({ abortSignal }) {
      const script = queue.shift();
      if (!script) {
        throw new Error("ScriptedBackend：剧本耗尽（run 次数多于预置剧本数）");
      }
      // reasoning 段：累积 yield
      let reasoningAcc = "";
      for (const chunk of script.reasoning ?? []) {
        if (abortSignal.aborted) return;
        reasoningAcc += chunk;
        const result: ChatModelRunResult = {
          content: [{ type: "reasoning", text: reasoningAcc }],
        };
        yield result;
      }
      // text 段：累积 yield（与 reasoning 合并输出，贴近真实流的 part 序）
      let textAcc = "";
      for (const chunk of script.text) {
        if (abortSignal.aborted) return;
        textAcc += chunk;
        const content: ChatModelRunResult["content"] = reasoningAcc
          ? [
              { type: "reasoning", text: reasoningAcc },
              { type: "text", text: textAcc },
            ]
          : [{ type: "text", text: textAcc }];
        const result: ChatModelRunResult = { content };
        yield result;
      }
    },
  };
}
