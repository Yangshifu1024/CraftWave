/**
 * runMetrics store（§10.1 zustand `runMetrics`：速率/轮次/会话累计，帧驱动）。
 *
 * §10.5 + C-9 口径：
 * - rate = usage 帧的 rate（Rust 侧按「本轮输出 tok/s」计算）；无 rate 帧时保持上次值；
 * - turnUsage = 当前运行内 usage 累计（runEnded 后冻结）；
 * - sessionUsage = 会话累计：帧累计兜底，tokens:update 事件（Rust 会话累计
 *   全量，含 suggest 后台小调用）到达时权威 replace（14 号票接线）。
 */

import { create } from "zustand";
import type { ChatFrame, UsageTotal } from "@/ipc/types";

export interface RunMetricsState {
  /** 输出速率（tok/s；usage 帧驱动） */
  rate: number | null;
  /** 当前/最近一次运行的 usage 累计（runEnded 后冻结） */
  turnUsage: UsageTotal;
  /** 会话累计（帧累计兜底；tokens:update 事件到达时权威校正，§10.5/14 号票） */
  sessionUsage: UsageTotal;
  /** runEnded 的终态摘要（计数条定格用；null = 无运行） */
  lastRun: null | { reason: "done" | "error" | "cancelled"; durationMs: number; toolCount: number };
  /** 帧入口：adapter 的 channel.onmessage 转发 */
  ingest: (frame: ChatFrame) => void;
  /** tokens:update 事件入口（Rust 会话累计全量；replace 语义，含后台小调用，C-9） */
  applyTokensUpdate: (usage: UsageTotal) => void;
  /** 重置 turnUsage（新 run 起点由 runStarted 帧触发） */
  resetTurn: (runId: string) => void;
}

const zeroUsage = (): UsageTotal => ({
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
});

function addUsage(a: UsageTotal, b: UsageTotal): UsageTotal {
  return {
    input: a.input + b.input,
    output: a.output + b.output,
    cacheRead: a.cacheRead + b.cacheRead,
    cacheWrite: a.cacheWrite + b.cacheWrite,
  };
}

export const useRunMetricsStore = create<RunMetricsState>((set, get) => ({
  rate: null,
  turnUsage: zeroUsage(),
  sessionUsage: zeroUsage(),
  lastRun: null,

  ingest: (frame) => {
    switch (frame.type) {
      case "runStarted":
        // 新 run：轮次清零（§10.5 turnUsage 按 runId 清零累计），会话累计保留
        set({ turnUsage: zeroUsage(), lastRun: null });
        break;
      case "usage": {
        const { turnUsage, sessionUsage, rate } = get();
        set({
          turnUsage: addUsage(turnUsage, frame.usage),
          sessionUsage: addUsage(sessionUsage, frame.usage),
          rate: frame.rate ?? rate,
        });
        break;
      }
      case "runEnded":
        set({
          lastRun: {
            reason: frame.reason,
            durationMs: frame.durationMs,
            toolCount: frame.toolCount,
          },
        });
        break;
      default:
        break;
    }
  },

  resetTurn: (runId) => {
    void runId;
    set({ turnUsage: zeroUsage(), lastRun: null });
  },

  applyTokensUpdate: (usage) => {
    // 权威校正（§10.5 sessionUsage 以 tokens:update 为准）：Rust 侧会话累计
    // 全量 replace（含 suggest 等后台小调用的计入，C-9）；帧累计兜底防丢事件。
    set({ sessionUsage: { ...usage } });
  },
}));

/** 上下文进度（§10.5：sessionUsage 折算当前模型 contextWindow；返回占比 0–1）。 */
export function contextProgress(usage: UsageTotal, contextWindow: number): number {
  if (contextWindow <= 0) return 0;
  const total = usage.input + usage.output;
  return Math.min(1, total / contextWindow);
}
