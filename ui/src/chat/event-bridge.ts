/**
 * 后端事件桥：App 级订阅事件分发到 store。
 *
 * - ask:opened / ask:closed → ask store（AskPanel 渲染源）；
 * - run:suggestions → askHint（Composer ghost）；
 * - tokens:update → runMetrics.sessionUsage 权威校正；
 * - plan:update → planState（Inspector 计划页，参考样式改版）；
 * - run:compacting / run:compacted / run:compact_failed → compaction store
 *   （Composer 指示条 + 边界投影刷新 + 行内失败提示）；
 * - run:inject → followUp store 插话节点（时间线「插话」徽标）；
 * - run:queue → sessions store 队列态（队列条增删/暂停态/取消
 *   返还草稿，后端 queue.json 为投递权威）；
 * - run:start → 非运行态收到即后端自动投递的新 run，经 attach 观察态接入
 *   续流；
 * - session:title → sessions store 标题就地更新（自动起名落盘后
 *   事件驱动，左栏列表与卡头随 store 即时刷新）。
 *
 * listen 依赖可注入（测试）。
 */

import { useEffect } from "react";
import { useCompactionStore } from "@/chat/compaction-store";
import { useFollowUpStore } from "@/chat/followup-store";
import { usePlanStateStore } from "@/chat/plan-state";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { reconnectIfNeeded } from "@/chat/run-reconnect";
import { usageDelta, useUsageStatsStore } from "@/chat/usage-stats";
import { t } from "@/i18n/core";
import { EVENT_KEYS, listenEvent, loadSessionTree } from "@/ipc/client";
import type {
  AskClosedEvent,
  AskOpenedEvent,
  PlanUpdateEvent,
  RunCompactedEvent,
  RunCompactFailedEvent,
  RunCompactingEvent,
  RunInjectEvent,
  RunQueueEvent,
  RunStartEvent,
  RunSuggestionsEvent,
  SessionTitleEvent,
  StoredCompactionEntry,
  SubDoneEvent,
  SubErrorEvent,
  SubReportEvent,
  SubSpawnEvent,
  SubStepEvent,
  SubUsageEvent,
  TokensUpdateEvent,
} from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { useSessionsStore } from "@/stores/sessions";
import { subDone, subError, subReport, subSpawn, subStep, subUsage } from "@/stores/subagents";
import { pushToast } from "@/stores/toast";

/** 事件桥依赖面：listen 函数可注入（生产用 ipc/client 的 listenEvent，测试换假实现）。
 *  handler 参数标成 never 是为了让各种带具体类型的事件处理函数都能直接传入
 *  （TypeScript 函数参数逆变放行，无需再写一次类型断言）。 */
export interface EventBridgeDeps {
  listen: (key: string, handler: (payload: never) => void) => Promise<() => void>;
  /** 压缩收口后的边界投影刷新源（生产 = loadSessionTree；测试注入 no-op）。
   *  返回该会话的最新 compactions 投影（失败 → null，保留旧投影）。 */
  reloadCompactions?: (sessionId: string) => Promise<unknown>;
  /** run:start 驱动的观察态接入（自动投递续流；生产 = reconnectIfNeeded，
   *  其内部先查快照确认 run 仍活着再 attach——事件与投递竞态下幂等）。 */
  autoDeliverReconnect?: (sessionId: string) => Promise<boolean>;
}

/** 生产依赖：用 ipc/client 的 listenEvent 真实订阅 Tauri 事件。 */
export const defaultEventBridgeDeps: EventBridgeDeps = {
  listen: (key, handler) => listenEvent(key, handler as (payload: never) => void),
  reloadCompactions: (sessionId) => loadSessionTree(sessionId),
  autoDeliverReconnect: (sessionId) => reconnectIfNeeded(sessionId),
};

/** run:queue 事件的本地队列条校正：本地数组比后端权威 size 长
 *  时裁尾（投递消费后的事件与本地入队的竞态差），短则保留（重启恢复场景
 *  明细以后端为准但事件不带明细，等下一次入队/删除对齐）。 */
export function reconcileQueueSize(sessionId: string, size: number): void {
  const store = useSessionsStore.getState();
  const local = store.queues[sessionId] ?? [];
  if (local.length > size) {
    store.setQueue(sessionId, local.slice(0, size));
  }
}

/** 挂载即订阅（App 生命周期一份），卸载时全部取消。 */
export function useBackendEvents(deps: EventBridgeDeps = defaultEventBridgeDeps): void {
  useEffect(() => {
    const unlistens = [
      deps.listen(EVENT_KEYS.askOpened, (p: AskOpenedEvent) => {
        useAskStore.getState().open(p.sessionId, p.askId, p.payload);
      }),
      deps.listen(EVENT_KEYS.askClosed, (p: AskClosedEvent) => {
        // ask:closed 载荷无 sessionId，按 askId 全桶定位关闭
        for (const sessionId of Object.keys(useAskStore.getState().bySession)) {
          useAskStore.getState().close(sessionId, p.askId);
        }
      }),
      deps.listen(EVENT_KEYS.runSuggestions, (p: RunSuggestionsEvent) => {
        useAskStore.getState().setHint(p.sessionId, p.text);
      }),
      deps.listen(EVENT_KEYS.tokensUpdate, (p: TokensUpdateEvent) => {
        // 本地增量差分：先取旧会话累计再 replace——Inspector 统计
        // 区块据此对今日/该模型条目做本地增量修正（不等轮询）
        const prev = useRunMetricsStore.getState().bySession[p.sessionId]?.sessionUsage;
        useRunMetricsStore.getState().applyTokensUpdate(p.sessionId, p.usage);
        if (prev) {
          useUsageStatsStore.getState().applyTokensDelta(usageDelta(prev, p.usage), p.modelId);
        }
      }),
      deps.listen(EVENT_KEYS.planUpdate, (p: PlanUpdateEvent) => {
        // 全量计划态（条目 + 写范围声明 + 批准态与冻结基线）
        usePlanStateStore.getState().applyUpdate(p.sessionId, {
          items: p.items,
          paths: p.paths,
          approved: p.approved,
          baselinePaths: p.baselinePaths,
        });
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
        // 失败反馈改走顶部 Toast，行内
        // 常驻提示撤除——phase 复位（清除可能残留的 compacting 指示条）
        useCompactionStore.getState().setPhase(p.sessionId, null);
        pushToast("error", t("toasts.compactFailed"), t("toasts.compactFailedDetail"));
      }),
      deps.listen(EVENT_KEYS.runInject, (p: RunInjectEvent) => {
        // steer 被消费：时间线本地插入插话节点（canonical 由 Rust 同 id 落盘，
        // 树重载收养后按 entryId 去重）
        useFollowUpStore.getState().addSteerNode(p.sessionId, {
          entryId: p.entryId,
          text: p.text,
          acceptedAtTurn: p.acceptedAtTurn,
        });
      }),
      deps.listen(EVENT_KEYS.runQueue, (p: RunQueueEvent) => {
        const store = useSessionsStore.getState();
        store.setQueuePaused(p.sessionId, p.paused);
        if (p.returned !== undefined && p.returned.length > 0) {
          // 取消/失败返还：清队列 + 文本回填草稿（returned 为后端权威交还面）
          store.returnQueueToDraft(p.sessionId, p.returned);
        } else {
          reconcileQueueSize(p.sessionId, p.size);
        }
      }),
      deps.listen(EVENT_KEYS.runStart, (p: RunStartEvent) => {
        // 非运行态收到 run:start = 后端自动投递的新 run（手动发起时 adapter
        // 已在运行态、帧走自身 channel）：attach 观察态接入续流（
        // reconnectIfNeeded 内部先查快照，竞态下幂等）
        if (useSessionsStore.getState().running[p.sessionId] !== true) {
          void deps.autoDeliverReconnect?.(p.sessionId)?.catch((err) => {
            console.warn("[followUp] 自动投递观察态接入失败", err);
          });
        }
      }),
      // ---- 子代理生命周期（跨 Tab Inspector 订阅）----
      deps.listen(EVENT_KEYS.subSpawn, (p: SubSpawnEvent) => {
        subSpawn(p);
      }),
      deps.listen(EVENT_KEYS.subStep, (p: SubStepEvent) => {
        subStep(p);
      }),
      deps.listen(EVENT_KEYS.subReport, (p: SubReportEvent) => {
        subReport(p);
      }),
      deps.listen(EVENT_KEYS.subUsage, (p: SubUsageEvent) => {
        // 子代理自身消耗：仅子面板展示，不再累进会话总量
        //（会话累计由 tokens:update 单一出口承担）
        subUsage(p);
      }),
      deps.listen(EVENT_KEYS.subDone, (p: SubDoneEvent) => {
        subDone(p);
      }),
      deps.listen(EVENT_KEYS.subError, (p: SubErrorEvent) => {
        subError(p);
      }),
      // ---- 自动起名 ----
      deps.listen(EVENT_KEYS.sessionTitle, (p: SessionTitleEvent) => {
        // 标题就地更新：tabs 快照 + 左栏缓存列表同改（会话不在 tabs 的
        // 后台项目会话只更新缓存列表，不强行开 Tab——store 内分支）
        useSessionsStore.getState().applyTitle(p.sessionId, p.title);
      }),
    ];
    return () => {
      for (const unlisten of unlistens) {
        void unlisten.then((fn) => fn()).catch(() => {});
      }
    };
  }, [deps]);
}
