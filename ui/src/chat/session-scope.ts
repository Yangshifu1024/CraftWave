/**
 * 会话作用域（16 号票多会话）：SessionTab 以 Provider 注入本 Tab 的
 * sessionId；Thread/ToolCards/AskPanel 经 useSessionScope 消费——
 * 工具卡边车恢复、ask 面板、ghost hint、计数条都按所属会话取数。
 */

import { createContext, useContext } from "react";

export const SessionScopeContext = createContext<string>("local");

export function useSessionScope(): string {
  return useContext(SessionScopeContext);
}
