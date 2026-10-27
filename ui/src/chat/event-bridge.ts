/**
 * 后端事件桥（§3.5，14 号票）：App 级订阅 4 键事件分发到 store。
 *
 * - ask:opened / ask:closed → ask store（AskPanel 渲染源）；
 * - run:suggestions → askHint（Composer ghost，§10.4）；
 * - tokens:update → runMetrics.sessionUsage 权威校正（§10.5/C-9）。
 *
 * plan:update 本票不订阅（右栏常驻计划面板属桌面体验票；消息内 plan 卡由
 * 帧 face 驱动）。listen 依赖可注入（测试）。
 */

import { useEffect } from "react";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { EVENT_KEYS, listenEvent } from "@/ipc/client";
import type {
  AskClosedEvent,
  AskOpenedEvent,
  RunSuggestionsEvent,
  TokensUpdateEvent,
} from "@/ipc/types";
import { useAskStore } from "@/stores/ask";

/** 事件桥依赖面：listen 函数可注入（生产用 ipc/client 的 listenEvent，测试换假实现）。
 *  handler 参数标成 never 是为了让各种带具体类型的事件处理函数都能直接传入
 *  （TypeScript 函数参数逆变放行，无需再写一次类型断言）。 */
export interface EventBridgeDeps {
  listen: (key: string, handler: (payload: never) => void) => Promise<() => void>;
}

/** 生产依赖：用 ipc/client 的 listenEvent 真实订阅 Tauri 事件。 */
export const defaultEventBridgeDeps: EventBridgeDeps = {
  listen: (key, handler) => listenEvent(key, handler as (payload: never) => void),
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
    ];
    return () => {
      for (const unlisten of unlistens) {
        void unlisten.then((fn) => fn()).catch(() => {});
      }
    };
  }, [deps]);
}
