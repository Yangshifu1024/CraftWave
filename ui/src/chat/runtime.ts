import type { ChatModelAdapter, ChatModelRunResult, ThreadMessage } from "@assistant-ui/react";
import * as client from "@/ipc/client";
import type { ChatFrame, MessageEntry, RunConfig } from "@/ipc/types";
import { syncTreeBeforeSend, useSessionPersistStore } from "@/stores/session";
import { bindCancelToRun } from "./cancel-bridge";
import { type ChatPart, FrameBuffer, toParts } from "./frames";
import { useRunMetricsStore } from "./run-metrics";
import { readRunMarker } from "./tree-persistence";

// ---- S6 骨架回声适配器（无后端 dev fallback，原样保留） ----

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

// ---- tauri 后端适配器（§10.2：start_chat + Channel 帧 → assistant-ui） ----

/**
 * 单会话调试常量（S6 骨架遗留）：生产路径已由 sessions store 提供
 * 真实会话 id（16 号票）；保留供测试与 ?mirror 入口。
 */
export const LOCAL_SESSION_ID = "local";

/** M1 默认运行配置（auto_edit 档直写 roots；maxTurns 文档默认 24）。 */
export const DEFAULT_RUN_CONFIG: RunConfig = {
  tier: "autoEdit",
  maxTurns: 24,
  steer: false,
};

/**
 * assistant-ui 消息 → wire 中立消息（MessageEntry 形态，§3.2/§6.1）。
 * 取活动路径线性序（assistant-ui 传入序）；工具调用 part 为已合并形态
 * （tool-call 带 result），拆回 toolCall + toolResult 双 part（§6.2 配对面）。
 * status/runId 原样透传——canonical 条目往返（import → save）字段集一致，
 * 否则 save_session_tree 内容比对漂移（§6.3，15 号票）。
 */
export function toWire(messages: readonly ThreadMessage[]): MessageEntry[] {
  return messages.map((message) => {
    const parts: MessageEntry["parts"] = [];
    // runMarker 节点（收养历史里的 system 空消息，custom 载荷保真）：parts 还原
    // 为 RunMarker part——否则空 parts 条目与 Rust 侧同 id 条目漂移（E_SESSION_REV）
    const marker = readRunMarker(message);
    if (marker !== null) {
      return {
        id: message.id,
        role: "system",
        parts: [{ type: "runMarker", ...marker }],
        createdAt: message.createdAt.getTime(),
      } satisfies MessageEntry;
    }
    for (const part of message.content) {
      switch (part.type) {
        case "text":
          if (part.text.length > 0) parts.push({ type: "text", text: part.text });
          break;
        case "reasoning":
          if (part.text.length > 0) parts.push({ type: "reasoning", text: part.text });
          break;
        case "tool-call": {
          const argsJson = JSON.stringify(part.args ?? {});
          parts.push({
            type: "toolCall",
            callId: part.toolCallId,
            name: part.toolName,
            argsJson,
          });
          if (part.result !== undefined) {
            const result = part.result as { ok?: boolean; summary?: string };
            parts.push({
              type: "toolResult",
              callId: part.toolCallId,
              ok: result.ok ?? !part.isError,
              summary:
                result.summary ??
                (typeof part.result === "string" ? part.result : JSON.stringify(part.result)),
            });
          }
          break;
        }
        default:
          // image 等 M2 part 不在 M1 链路面
          break;
      }
    }
    if (parts.length === 0) parts.push({ type: "text", text: "" });
    const entry: MessageEntry = {
      // assistant-ui 的 message.id 随树稳定（重生成分支各有 id），直接采用
      id: message.id,
      role:
        message.role === "assistant" ? "assistant" : message.role === "system" ? "system" : "user",
      parts,
      createdAt: message.createdAt.getTime(),
    };
    if (message.role === "assistant") {
      if (message.status !== undefined) entry.status = message.status;
      if (message.metadata?.runId !== undefined) entry.runId = message.metadata.runId;
    }
    return entry;
  });
}

/** adapter 依赖（生产 = ipc/client；测试注入 fake）。 */
export interface TauriAdapterDeps {
  sessionId: string;
  runConfig: RunConfig;
  startChat: typeof client.startChat;
  cancelRun: (sessionId: string) => Promise<boolean>;
  /** 帧旁路（生产：runMetrics.ingest；§10.2 onmessage 双写） */
  onFrame?: (frame: ChatFrame) => void;
  /** 发送前树同步（15 号票；测试注入 no-op 绕开 IPC）。 */
  syncTree?: (messages: readonly ThreadMessage[]) => Promise<void>;
}

export const defaultDeps: TauriAdapterDeps = {
  sessionId: LOCAL_SESSION_ID,
  runConfig: DEFAULT_RUN_CONFIG,
  startChat: client.startChat,
  cancelRun: client.cancelRun,
};

/** runEnded reason → assistant-ui 终态 status（done=complete；cancelled/error=incomplete）。 */
function endStatus(reason: "done" | "error" | "cancelled"): ChatModelRunResult["status"] {
  switch (reason) {
    case "cancelled":
      return { type: "incomplete", reason: "cancelled" };
    case "error":
      return { type: "incomplete", reason: "error" };
    default:
      return { type: "complete", reason: "stop" };
  }
}

/** 生产 adapter（§10.2 伪代码的产品化落地，S6 骨架遗留单例形态）。 */
export const tauriChatModel: ChatModelAdapter = createTauriChatAdapter({
  ...defaultDeps,
  onFrame: (frame) => useRunMetricsStore.getState().ingest(defaultDeps.sessionId, frame),
});

/**
 * 按会话产出生产 adapter（16 号票多会话）：每 Tab 一个实例——帧入本会话的
 * metrics 桶，运行态节点记账记到本会话的 persist 分桶。
 */
export function sessionAdapter(sessionId: string): ChatModelAdapter {
  return createTauriChatAdapter({
    ...defaultDeps,
    sessionId,
    onFrame: (frame) => useRunMetricsStore.getState().ingest(sessionId, frame),
  });
}

/**
 * 可注入 adapter 工厂：start_chat 带 channel → FrameBuffer 累积 → 64ms 节流
 * 快照全量 yield（打字机，§10.2）→ runEnded 快照以终态 status 收口；abortSignal
 * 桥 cancel_run（S5：fire-and-forget，终态以 runEnded 帧为权威）。
 *
 * 15 号票双写协议：run() 开头先 save（新 user 拓扑先落盘）再 start_chat；
 * 运行态 assistant 节点 id 记账（save 载荷剔除与引用改写用，§6.3）。
 */
export function createTauriChatAdapter(deps: TauriAdapterDeps): ChatModelAdapter {
  return {
    async *run({ messages, abortSignal, unstable_assistantMessageId }) {
      // 发送前树同步（失败不阻断发送：Rust 侧 user 兜底 + 下次 save 自愈）
      const syncTree =
        deps.syncTree ??
        ((msgs: readonly ThreadMessage[]) => syncTreeBeforeSend(deps.sessionId, msgs));
      try {
        await syncTree(messages);
      } catch (err) {
        console.warn("[session] 发送前树同步失败（继续发送）", err);
      }
      const buffer = new FrameBuffer();
      const signal = abortSignal ?? new AbortController().signal;
      const unbind = bindCancelToRun(signal, deps.sessionId, (cmd, args) => {
        if (cmd === "cancel_run") {
          return deps.cancelRun((args as { sessionId: string }).sessionId);
        }
        return Promise.reject(new Error(`非桥接命令：${cmd}`));
      });
      try {
        const invokePromise = deps.startChat({
          sessionId: deps.sessionId,
          messages: toWire(messages),
          runConfig: deps.runConfig,
          onFrame: (frame: ChatFrame) => {
            if (import.meta.env.DEV && frame.type === "runEnded" && frame.reason === "error") {
              console.error("[run] 错误终态：", frame.error);
            }
            if (
              unstable_assistantMessageId !== undefined &&
              frame.type === "runStarted" &&
              !useSessionPersistStore
                .getState()
                .scopes[deps.sessionId]?.runtimeNodes.has(unstable_assistantMessageId)
            ) {
              useSessionPersistStore
                .getState()
                .noteRuntimeNode(deps.sessionId, unstable_assistantMessageId, frame.runId);
            }
            buffer.push(frame);
            deps.onFrame?.(frame);
          },
        });
        // 命令面失败在 dev 模式打原始错误（UI 只显示通用错误；排障线索进
        // webview console + tauri dev 终端，2026-10-28 优化项）
        if (import.meta.env.DEV) {
          invokePromise.catch((err) => console.error("[start_chat] 命令失败：", err));
        }
        // 64ms 节流快照迭代：runEnded 快照 yield 后迭代自然收口（§10.2）
        for await (const snapshot of buffer.snapshots()) {
          if (signal.aborted && !snapshot.ended) {
            // 本地 abort 已触发而终态帧未到（命令面异常路径）：停止产出，
            // 终态仍以 runEnded 帧为权威（S5 语义）。
            break;
          }
          const content = toParts(snapshot);
          yield {
            content: content.length > 0 ? content : [{ type: "text", text: "" }],
            ...(snapshot.ended ? { status: endStatus(snapshot.ended.reason) } : {}),
          };
        }
        await invokePromise;
      } finally {
        unbind();
      }
    },
  };
}

// 保持 ChatPart 类型被导出（测试断言 content 形状用）
export type { ChatPart };
