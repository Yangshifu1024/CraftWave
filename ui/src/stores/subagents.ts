/**
 * 子代理 store（zustand）：Inspector 子代理页与 subagent
 * 工具卡的共享数据面。
 *
 * 数据源双通道（幂等合并，subId 去重）：
 * - **事件**（sub:spawn/step/report/usage/done/error，跨 Tab 广播）——
 *   event-bridge 订阅后调用对应显式方法（`subXxx` 系列）；
 * - **帧**（subStarted/subFrame/subReport/subDone，父 run 帧面）——
 *   实时（runtime onFrame）与重连重放（run-reconnect）两条路径都经过，
 *   经 [`applySubagentFrame`] 分发；重载后子代理进行中状态由重放恢复。
 *
 * 状态收口语义：done/error 之前每条记录 state=running；step 推进 turn
 * 计数；usage 为子代理自身累计（仅子面板展示，不再累进会话总量——
 * 会话累计由 tokens:update 单一出口承担）。
 */

import { create } from "zustand";

import { t } from "@/i18n/core";
import type { ChatFrame } from "@/ipc/generated/ChatFrame";
import type { SubOutcome } from "@/ipc/generated/SubOutcome";
import type { UsageTotal } from "@/ipc/generated/UsageTotal";

/** 单个子代理的展示记录（Inspector 列表行与工具卡共用）。 */
export interface SubagentItem {
  subId: string;
  /** 委派发起的父工具调用 id（subagent 工具卡按它关联）。 */
  callId: string;
  role: string;
  sessionId: string;
  runId: string;
  /** 生命周期：running → 终态四象限（report/partial/no_report/cancelled）。 */
  state: "running" | SubOutcome;
  /** 已推进的模型轮次（sub:step / subFrame(stepStarted) 驱动）。 */
  turn: number;
  /** 子代理自身用量累计（子面板展示面）。 */
  usage: UsageTotal;
  /** `<report>` 结构化汇报全文（subReport 帧后可展开）。 */
  report: string;
  /** 失败说明（sub:error 事件 / no_report）。 */
  error?: string;
  startedAt: number;
}

/** 抽屉回看的历史状态：加载中 / 已加载（消息数组）/ 失败。 */
export interface SubagentHistoryState {
  loading: boolean;
  error: string | null;
  messages: unknown[];
}

/** 按会话分桶的子代理列表 + 抽屉开合态。 */
interface SubagentsState {
  bySession: Record<string, SubagentItem[]>;
  /** 抽屉回看展开中的子代理 id（同一时间只开一个）。 */
  openSubId: string | null;
  /** 抽屉回看的消息缓存（loadSubagentHistory 拉取结果）。 */
  historyBySub: Record<string, SubagentHistoryState>;
}

/** 空用量（usage 未上报前的稳定引用）。 */
const EMPTY_USAGE: UsageTotal = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };

/** 子代理状态仓库实例。 */
export const useSubagentsStore = create<SubagentsState>(() => ({
  bySession: {},
  openSubId: null,
  historyBySub: {},
}));

/** 定位（或新建）一个子代理记录：找不到时以已知字段兜底建行（重连重放
 *  场景 subFrame 可能先于 sub:spawn 事件到达）。 */
function upsertItem(
  sessionId: string,
  subId: string,
  seed: { callId?: string; role?: string; runId?: string },
  patch: Partial<Omit<SubagentItem, "subId" | "sessionId" | "startedAt">>,
): void {
  const state = useSubagentsStore.getState();
  const list = state.bySession[sessionId] ?? [];
  const index = list.findIndex((item) => item.subId === subId);
  if (index === -1) {
    const item: SubagentItem = {
      subId,
      callId: seed.callId ?? "",
      role: seed.role ?? "",
      sessionId,
      runId: seed.runId ?? "",
      state: patch.state ?? "running",
      turn: patch.turn ?? 0,
      usage: patch.usage ?? EMPTY_USAGE,
      report: patch.report ?? "",
      ...(patch.error !== undefined ? { error: patch.error } : {}),
      startedAt: Date.now(),
    };
    useSubagentsStore.setState({
      bySession: { ...state.bySession, [sessionId]: [...list, item] },
    });
    return;
  }
  const next = [...list];
  const current = next[index];
  if (current === undefined) {
    return;
  }
  next[index] = {
    ...current,
    ...(patch.callId !== undefined && current.callId === "" ? { callId: patch.callId } : {}),
    ...(patch.role !== undefined ? { role: patch.role } : {}),
    ...(patch.runId !== undefined && current.runId === "" ? { runId: patch.runId } : {}),
    ...(patch.turn !== undefined ? { turn: Math.max(current.turn, patch.turn) } : {}),
    ...(patch.usage !== undefined ? { usage: patch.usage } : {}),
    ...(patch.report !== undefined ? { report: patch.report } : {}),
    ...(patch.error !== undefined ? { error: patch.error } : {}),
    ...(patch.state !== undefined ? { state: patch.state } : {}),
  };
  useSubagentsStore.setState({
    bySession: { ...state.bySession, [sessionId]: next },
  });
}

/** sub:spawn（委派启动：角色与父 run 归属）。 */
export function subSpawn(event: {
  sessionId: string;
  runId: string;
  subId: string;
  role: string;
}): void {
  upsertItem(event.sessionId, event.subId, { role: event.role, runId: event.runId }, {});
}

/** sub:step（轮次推进）。 */
export function subStep(event: { sessionId: string; subId: string; turn: number }): void {
  upsertItem(event.sessionId, event.subId, {}, { turn: event.turn });
}

/** sub:usage（子代理自身消耗，子面板展示面）。 */
export function subUsage(event: { sessionId: string; subId: string; usage: UsageTotal }): void {
  upsertItem(event.sessionId, event.subId, {}, { usage: event.usage });
}

/** sub:report（`<report>` 汇报全文）。 */
export function subReport(event: { sessionId: string; subId: string; report: string }): void {
  upsertItem(event.sessionId, event.subId, {}, { report: event.report });
}

/** sub:done（四象限收口）。 */
export function subDone(event: { sessionId: string; subId: string; outcome: string }): void {
  upsertItem(event.sessionId, event.subId, {}, { state: normalizeOutcome(event.outcome) });
}

/** sub:error（异常终态说明）。 */
export function subError(event: {
  sessionId: string;
  subId: string;
  code?: string;
  message?: string;
}): void {
  upsertItem(
    event.sessionId,
    event.subId,
    {},
    {
      state: "no_report",
      error: `${event.code ?? "E_INTERNAL"}: ${event.message ?? ""}`,
    },
  );
}

/** outcome 字符串 → 终态（未知值按 no_report 收口）。 */
function normalizeOutcome(outcome: string | undefined): SubagentItem["state"] {
  if (outcome === "report" || outcome === "partial" || outcome === "cancelled") return outcome;
  return "no_report";
}

/** 帧分发（runtime onFrame 与重连重放共用；父 run 帧面的 sub 帧驱动，
 *  重载后子代理进行中状态由 attach 重放恢复）。 */
export function applySubagentFrame(sessionId: string, frame: ChatFrame): void {
  switch (frame.type) {
    case "subStarted":
      upsertItem(
        sessionId,
        frame.subId,
        { callId: frame.callId, role: frame.role },
        {
          state: "running",
        },
      );
      break;
    case "subFrame":
      if (frame.frame.type === "stepStarted") {
        upsertItem(sessionId, frame.subId, {}, { turn: frame.frame.turn });
      }
      break;
    case "subReport":
      upsertItem(sessionId, frame.subId, {}, { report: frame.report });
      break;
    case "subDone":
      upsertItem(sessionId, frame.subId, {}, { state: normalizeOutcome(frame.outcome) });
      break;
    default:
      break;
  }
}

/** 打开/关闭抽屉回看。 */
export function setOpenSub(subId: string | null): void {
  useSubagentsStore.setState({ openSubId: subId });
}

/** 标记抽屉历史加载中（拉取前调用）。 */
export function markHistoryLoading(subId: string): void {
  const state = useSubagentsStore.getState();
  useSubagentsStore.setState({
    historyBySub: {
      ...state.historyBySub,
      [subId]: { loading: true, error: null, messages: [] },
    },
  });
}

/** 抽屉历史加载完成（成功或失败）。 */
export function setHistoryLoaded(subId: string, messages: unknown[], error: string | null): void {
  const state = useSubagentsStore.getState();
  useSubagentsStore.setState({
    historyBySub: {
      ...state.historyBySub,
      [subId]: { loading: false, error, messages },
    },
  });
}

/** 终态徽标的展示文案（四象限着色语义，Inspector/工具卡共用）：经字典
 *  查词（i18n，模块级 t()）。 */
export function outcomeLabel(state: SubagentItem["state"]): string {
  switch (state) {
    case "report":
      return t("inspector.outcomeCompleted");
    case "partial":
      return t("inspector.outcomeBudgetExhausted");
    case "cancelled":
      return t("inspector.outcomeCancelled");
    case "no_report":
      return t("inspector.outcomeNoReport");
    default:
      return t("inspector.outcomeRunning");
  }
}
