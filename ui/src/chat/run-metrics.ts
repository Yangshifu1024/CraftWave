/**
 * runMetrics store（§10.1 zustand `runMetrics`：速率/轮次/会话累计，帧驱动）。
 *
 * §10.5 + C-9 口径：
 * - rate = usage 帧的 rate（Rust 侧按「本轮输出 tok/s」计算）；无 rate 帧时保持上次值；
 * - turnUsage = 当前运行内 usage 累计（runEnded 后冻结）；
 * - sessionUsage = 会话累计（M1 无后台小调用源——suggest/标题生成属 M2，
 *   C-9「后台小调用计会话累计、不计轮次」的口径天然满足；tokens:update 事件
 *   与 SessionMeta.totalTokens 重启回填随对应票接入）。
 */

import { create } from "zustand";
import type { ChatFrame, UsageTotal } from "@/ipc/types";

export interface RunMetricsState {
  /** 输出速率（tok/s；usage 帧驱动） */
  rate: number | null;
  /** 当前/最近一次运行的 usage 累计（runEnded 后冻结） */
  turnUsage: UsageTotal;
  /** 会话累计（M1 = 运行累计；tokens:update 随会话域票接入） */
  sessionUsage: UsageTotal;
  /** runEnded 的终态摘要（计数条定格用；null = 无运行） */
  lastRun: null | { reason: "done" | "error" | "cancelled"; durationMs: number; toolCount: number };
  /** 帧入口：adapter 的 channel.onmessage 转发 */
  ingest: (frame: ChatFrame) => void;
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
}));

/** 上下文进度（§10.5：sessionUsage 折算当前模型 contextWindow；返回占比 0–1）。 */
export function contextProgress(usage: UsageTotal, contextWindow: number): number {
  if (contextWindow <= 0) return 0;
  const total = usage.input + usage.output;
  return Math.min(1, total / contextWindow);
}
