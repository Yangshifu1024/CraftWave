/**
 * runMetrics store（§10.1 zustand `runMetrics`：速率/轮次/会话累计，帧驱动）。
 *
 * §10.5 + C-9 口径：
 * - rate = usage 帧的 rate（Rust 侧按「本轮输出 tok/s」计算）；无 rate 帧时保持上次值；
 * - turnUsage = 当前运行内 usage 累计（runEnded 后冻结）；
 * - sessionUsage = 会话累计：帧累计兜底，tokens:update 事件（Rust 会话累计
 *   全量，含 suggest 后台小调用）到达时权威 replace（14 号票接线）。
 *
 * 16 号票多会话：全部状态按 sessionId 分桶（跨 Tab 并行 run 各自累计互不串扰）。
 */

import { create } from "zustand";
import type { ChatFrame, UsageTotal } from "@/ipc/types";

/** 单会话的三数与终态摘要。 */
export interface SessionMetrics {
  /** 输出速率（tok/s；usage 帧驱动） */
  rate: number | null;
  /** 当前/最近一次运行的 usage 累计（runEnded 后冻结） */
  turnUsage: UsageTotal;
  /** 会话累计（帧累计兜底；tokens:update 事件到达时权威校正，§10.5/14 号票） */
  sessionUsage: UsageTotal;
  /** 进行中 run 的标识（runStarted 写入；runEnded 摘要回填用，17 号票） */
  currentRunId: string | null;
  /** runEnded 的终态摘要（计数条定格用；null = 无运行） */
  lastRun: null | {
    runId: string;
    reason: "done" | "error" | "cancelled";
    durationMs: number;
    toolCount: number;
  };
}

const zeroUsage = (): UsageTotal => ({
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
});

const emptyMetrics = (): SessionMetrics => ({
  rate: null,
  turnUsage: zeroUsage(),
  sessionUsage: zeroUsage(),
  currentRunId: null,
  lastRun: null,
});

function addUsage(a: UsageTotal, b: UsageTotal): UsageTotal {
  return {
    input: a.input + b.input,
    output: a.output + b.output,
    cacheRead: a.cacheRead + b.cacheRead,
    cacheWrite: a.cacheWrite + b.cacheWrite,
  };
}

export interface RunMetricsStore {
  bySession: Record<string, SessionMetrics>;
  /** 帧入口：adapter 的 channel.onmessage 转发（帧不带 sessionId，入口补） */
  ingest: (sessionId: string, frame: ChatFrame) => void;
  /** tokens:update 事件入口（Rust 会话累计全量；replace 语义，含后台小调用，C-9） */
  applyTokensUpdate: (sessionId: string, usage: UsageTotal) => void;
  /** 会话删除后清桶。 */
  forgetSession: (sessionId: string) => void;
}

function updateSession(
  bySession: Record<string, SessionMetrics>,
  sessionId: string,
  f: (m: SessionMetrics) => SessionMetrics,
): Record<string, SessionMetrics> {
  const current = bySession[sessionId] ?? emptyMetrics();
  return { ...bySession, [sessionId]: f(current) };
}

export const useRunMetricsStore = create<RunMetricsStore>((set) => ({
  bySession: {},

  ingest: (sessionId, frame) => {
    switch (frame.type) {
      case "runStarted":
        // 新 run：轮次清零（§10.5 turnUsage 按 runId 清零累计），会话累计保留
        set((s) => ({
          bySession: updateSession(s.bySession, sessionId, (m) => ({
            ...m,
            turnUsage: zeroUsage(),
            currentRunId: frame.runId,
            lastRun: null,
          })),
        }));
        break;
      case "usage":
        set((s) => ({
          bySession: updateSession(s.bySession, sessionId, (m) => ({
            ...m,
            turnUsage: addUsage(m.turnUsage, frame.usage),
            sessionUsage: addUsage(m.sessionUsage, frame.usage),
            rate: frame.rate ?? m.rate,
          })),
        }));
        break;
      case "runEnded":
        set((s) => ({
          bySession: updateSession(s.bySession, sessionId, (m) => ({
            ...m,
            currentRunId: null,
            lastRun: {
              runId: m.currentRunId ?? "",
              reason: frame.reason,
              durationMs: frame.durationMs,
              toolCount: frame.toolCount,
            },
          })),
        }));
        break;
      default:
        break;
    }
  },

  applyTokensUpdate: (sessionId, usage) => {
    // 权威校正（§10.5 sessionUsage 以 tokens:update 为准）：Rust 侧会话累计
    // 全量 replace（含 suggest 等后台小调用的计入，C-9）；帧累计兜底防丢事件。
    set((s) => ({
      bySession: updateSession(s.bySession, sessionId, (m) => ({
        ...m,
        sessionUsage: { ...usage },
      })),
    }));
  },

  forgetSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.bySession)) return s;
      const bySession = { ...s.bySession };
      delete bySession[sessionId];
      return { bySession };
    }),
}));

/** 零值快照（模块级常量：selector 缺桶时返回稳定引用，防渲染循环）。 */
const EMPTY_METRICS: SessionMetrics = emptyMetrics();

/** 响应式取单会话桶（无记录时返回零值快照，渲染恒有定义）。 */
export function useSessionMetrics(sessionId: string): SessionMetrics {
  return useRunMetricsStore((s) => s.bySession[sessionId]) ?? EMPTY_METRICS;
}

/** 上下文进度（§10.5：sessionUsage 折算当前模型 contextWindow；返回占比 0–1）。 */
export function contextProgress(usage: UsageTotal, contextWindow: number): number {
  if (contextWindow <= 0) return 0;
  const total = usage.input + usage.output;
  return Math.min(1, total / contextWindow);
}
