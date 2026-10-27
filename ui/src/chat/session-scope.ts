/**
 * 会话作用域（16 号票多会话）：SessionTab 以 Provider 注入本 Tab 的
 * sessionId；Thread/ToolCards/AskPanel 经 useSessionScope 消费——
 * 工具卡边车恢复、ask 面板、ghost hint、计数条都按所属会话取数。
 *
 * 17 号票追加：RunSummariesContext 注入本会话的 run 收口摘要表
 * （折叠分组摘要行 + 崩溃中断态的数据面，见 tree-persistence 的
 * deriveRunSummaries）。默认空表（无历史收养时）。
 */

import { createContext, useContext } from "react";
import type { RunSummary } from "./tree-persistence";

export const SessionScopeContext = createContext<string>("local");

export function useSessionScope(): string {
  return useContext(SessionScopeContext);
}

/** 本会话 run 收口摘要表（runId → 摘要）；SessionTab 注入，缺省空表。 */
export const RunSummariesContext = createContext<Map<string, RunSummary>>(new Map());

export function useRunSummaries(): Map<string, RunSummary> {
  return useContext(RunSummariesContext);
}
