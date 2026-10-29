/**
 * 后端事件桥（§3.5，14 号票）：App 级订阅事件分发到 store。
 *
 * - ask:opened / ask:closed → ask store（AskPanel 渲染源）；
 * - run:suggestions → askHint（Composer ghost，§10.4）；
 * - tokens:update → runMetrics.sessionUsage 权威校正（§10.5/C-9）；
 * - plan:update → planState（Inspector 计划页，参考样式改版）；
 * - run:compacting / run:compacted / run:compact_failed → compaction store
 *   （04 号票：Composer 指示条 + 边界投影刷新 + 行内失败提示）。
 *
 * listen 依赖可注入（测试）。
 */

import { useEffect } from "react";
import { useCompactionStore } from "@/chat/compaction-store";
import { usePlanStateStore } from "@/chat/plan-state";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { EVENT_KEYS, listenEvent, loadSessionTree } from "@/ipc/client";
import type {
  AskClosedEvent,
  AskOpenedEvent,
  PlanUpdateEvent,
  RunCompactedEvent,
  RunCompactFailedEvent,
  RunCompactingEvent,
  RunSuggestionsEvent,
  StoredCompactionEntry,
  TokensUpdateEvent,
} from "@/ipc/types";
import { useAskStore } from "@/stores/ask";

/** 事件桥依赖面：listen 函数可注入（生产用 ipc/client 的 listenEvent，测试换假实现）。
 *  handler 参数标成 never 是为了让各种带具体类型的事件处理函数都能直接传入
 *  （TypeScript 函数参数逆变放行，无需再写一次类型断言）。 */
export interface EventBridgeDeps {
  listen: (key: string, handler: (payload: never) => void) => Promise<() => void>;
  /** 压缩收口后的边界投影刷新源（生产 = loadSessionTree；测试注入 no-op）。
   *  返回该会话的最新 compactions 投影（失败 → null，保留旧投影）。 */
  reloadCompactions?: (sessionId: string) => Promise<unknown>;
}

/** 生产依赖：用 ipc/client 的 listenEvent 真实订阅 Tauri 事件。 */
export const defaultEventBridgeDeps: EventBridgeDeps = {
  listen: (key, handler) => listenEvent(key, handler as (payload: never) => void),
  reloadCompactions: (sessionId) => loadSessionTree(sessionId),
};

/** 挂载即订阅（App 生命周期一份），卸载时全部取消。 */
export function useBackendEvents(deps: EventBridgeDeps = defaultEventBridgeDeps): void {
  useEffect(() => {
    const unlistens = [
      deps.listen(EVENT_KEYS.askOpened, (p: AskOpenedEvent) => {
        useAskStore.getState().open(p.sessionId, p.askId, p.payload);
      }),
      deps.listen(EVENT_KEYS.askClosed, (p: AskClosedEvent) => {
        // ask:closed 载荷无 sessionId（§3.5），按 askId 全桶定位关闭
        for (const sessionId of Object.keys(useAskStore.getState().bySession)) {
          useAskStore.getState().close(sessionId, p.askId);
        }
      }),
      deps.listen(EVENT_KEYS.runSuggestions, (p: RunSuggestionsEvent) => {
        useAskStore.getState().setHint(p.sessionId, p.text);
      }),
      deps.listen(EVENT_KEYS.tokensUpdate, (p: TokensUpdateEvent) => {
        useRunMetricsStore.getState().applyTokensUpdate(p.sessionId, p.usage);
      }),
      deps.listen(EVENT_KEYS.planUpdate, (p: PlanUpdateEvent) => {
        usePlanStateStore.getState().applyUpdate(p.sessionId, p.items);
      }),
      deps.listen(EVENT_KEYS.runCompacting, (p: RunCompactingEvent) => {
        useCompactionStore.getState().setPhase(p.sessionId, "compacting");
      }),
      deps.listen(EVENT_KEYS.runCompacted, (p: RunCompactedEvent) => {
        useCompactionStore.getState().setPhase(p.sessionId, null);
        // 边界投影即时刷新（压缩已落盘；拉取失败保留旧投影，下次收养自愈）
        void deps
          .reloadCompactions?.(p.sessionId)
          ?.then((tree) => {
            const compactions = (tree as { compactions?: StoredCompactionEntry[] } | null)
              ?.compactions;
            if (compactions !== undefined) {
              useCompactionStore.getState().setCompactions(p.sessionId, compactions);
            }
          })
          .catch(() => {});
      }),
      deps.listen(EVENT_KEYS.runCompactFailed, (p: RunCompactFailedEvent) => {
        useCompactionStore.getState().setPhase(p.sessionId, "failed");
      }),
    ];
    return () => {
      for (const unlisten of unlistens) {
        void unlisten.then((fn) => fn()).catch(() => {});
      }
    };
  }, [deps]);
}
