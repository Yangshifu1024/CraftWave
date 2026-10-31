/**
 * runMetrics store（zustand：速率/轮次/会话累计，帧驱动）。
 *
 * 口径：
 * - rate = usage 帧的 rate（Rust 侧按「本轮输出 tok/s」计算）；无 rate 帧时保持上次值；
 * - turnUsage = 当前运行内 usage 累计（runEnded 后冻结）；
 * - sessionUsage = 会话累计：帧累计兜底，tokens:update 事件（Rust 会话累计
 *   全量，含 suggest 后台小调用）到达时权威 replace。
 *
 * 多会话：全部状态按 sessionId 分桶（跨 Tab 并行 run 各自累计互不串扰）。
 */

import { create } from "zustand";
import type { SequencedFrame, UsageTotal } from "@/ipc/types";

/** 单会话的三数与终态摘要。 */
export interface SessionMetrics {
  /** 输出速率（tok/s；usage 帧驱动） */
  rate: number | null;
  /** 当前/最近一次运行的 usage 累计（runEnded 后冻结） */
  turnUsage: UsageTotal;
  /** 会话累计（帧累计兜底；tokens:update 事件到达时权威校正） */
  sessionUsage: UsageTotal;
  /** 进行中 run 的标识（runStarted 写入；runEnded 摘要回填用） */
  currentRunId: string | null;
  /** 思考计时（参考样式 A5）：reasoningDelta 首帧时刻（epoch ms）；模型转出
   *  正文/开始工具即置 null——「🧠 思考中…Ns」状态条的驱动面。 */
  thinkingSince: number | null;
  /** runEnded 的终态摘要（计数条定格用；null = 无运行） */
  lastRun: null | {
    runId: string;
    reason: "done" | "error" | "cancelled";
    durationMs: number;
    toolCount: number;
  };
}

/** 四项计数全 0 的用量对象。 */
const zeroUsage = (): UsageTotal => ({
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
});

/** 新会话桶的初始空指标（无速率、零用量、无运行记录）。 */
const emptyMetrics = (): SessionMetrics => ({
  rate: null,
  turnUsage: zeroUsage(),
  sessionUsage: zeroUsage(),
  currentRunId: null,
  thinkingSince: null,
  lastRun: null,
});

/** 两份用量逐项相加（帧累计用）。 */
function addUsage(a: UsageTotal, b: UsageTotal): UsageTotal {
  return {
    input: a.input + b.input,
    output: a.output + b.output,
    cacheRead: a.cacheRead + b.cacheRead,
    cacheWrite: a.cacheWrite + b.cacheWrite,
  };
}

/** runMetrics store 的状态与方法面。 */
export interface RunMetricsStore {
  /** 按会话分桶的指标表（sessionId → 指标），跨 Tab 并行运行互不串扰。 */
  bySession: Record<string, SessionMetrics>;
  /** 帧入口：adapter 的 channel.onmessage 转发（帧不带 sessionId，入口补） */
  ingest: (sessionId: string, frame: SequencedFrame) => void;
  /** tokens:update 事件入口（Rust 会话累计全量；replace 语义，含后台小调用） */
  applyTokensUpdate: (sessionId: string, usage: UsageTotal) => void;
  /** 会话删除后清桶。 */
  forgetSession: (sessionId: string) => void;
}

/** 桶级更新辅助：读桶（缺桶先补空指标）→ 变换 → 整表浅拷贝替换（保证
 *  zustand 依赖比较能感知变化，同时不改动其他会话的桶）。 */
function updateSession(
  bySession: Record<string, SessionMetrics>,
  sessionId: string,
  f: (m: SessionMetrics) => SessionMetrics,
): Record<string, SessionMetrics> {
  const current = bySession[sessionId] ?? emptyMetrics();
  return { ...bySession, [sessionId]: f(current) };
}

/** runMetrics zustand store：帧与事件两个入口写桶，按会话分桶存指标。 */
export const useRunMetricsStore = create<RunMetricsStore>((set) => ({
  bySession: {},

  ingest: (sessionId, frame) => {
    switch (frame.type) {
      case "runStarted":
        // 新 run：轮次清零（turnUsage 按 runId 清零累计），会话累计保留
        set((s) => ({
          bySession: updateSession(s.bySession, sessionId, (m) => ({
            ...m,
            turnUsage: zeroUsage(),
            currentRunId: frame.runId,
            thinkingSince: null,
            lastRun: null,
          })),
        }));
        break;
      case "reasoningDelta":
        // 思考计时起点（A5）：首个思考帧记录时刻，转出正文/工具后清零
        set((s) => ({
          bySession: updateSession(s.bySession, sessionId, (m) =>
            m.thinkingSince === null ? { ...m, thinkingSince: Date.now() } : m,
          ),
        }));
        break;
      case "textDelta":
      case "toolStarted":
        // 模型转入正文或开始动手：思考段结束
        set((s) => ({
          bySession: updateSession(s.bySession, sessionId, (m) =>
            m.thinkingSince === null ? m : { ...m, thinkingSince: null },
          ),
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
            thinkingSince: null,
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
    // 权威校正（sessionUsage 以 tokens:update 为准）：Rust 侧会话累计
    // 全量 replace（含 suggest 等后台小调用的计入）；帧累计兜底防丢事件。
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

/** 上下文进度（sessionUsage 折算当前模型 contextWindow；返回占比 0–1）。 */
export function contextProgress(usage: UsageTotal, contextWindow: number): number {
  if (contextWindow <= 0) return 0;
  const total = usage.input + usage.output;
  return Math.min(1, total / contextWindow);
}
