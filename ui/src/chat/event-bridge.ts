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

export interface EventBridgeDeps {
  /** handler 参数以 never 收宽（各事件的显式类型经逆变放行）。 */
  listen: (key: string, handler: (payload: never) => void) => Promise<() => void>;
}

export const defaultEventBridgeDeps: EventBridgeDeps = {
  listen: (key, handler) => listenEvent(key, handler as (payload: never) => void),
};

/** 挂载即订阅（App 生命周期一份），卸载时全部取消。 */
export function useBackendEvents(deps: EventBridgeDeps = defaultEventBridgeDeps): void {
  useEffect(() => {
    const unlistens = [
      deps.listen(EVENT_KEYS.askOpened, (p: AskOpenedEvent) => {
        useAskStore.getState().open(p.askId, p.payload);
      }),
      deps.listen(EVENT_KEYS.askClosed, (p: AskClosedEvent) => {
        useAskStore.getState().close(p.askId);
      }),
      deps.listen(EVENT_KEYS.runSuggestions, (p: RunSuggestionsEvent) => {
        useAskStore.getState().setHint(p.text);
      }),
      deps.listen(EVENT_KEYS.tokensUpdate, (p: TokensUpdateEvent) => {
        useRunMetricsStore.getState().applyTokensUpdate(p.usage);
      }),
    ];
    return () => {
      for (const unlisten of unlistens) {
        void unlisten.then((fn) => fn()).catch(() => {});
      }
    };
  }, [deps]);
}
