/**
 * run 重连观察态（帧序自愈 + attach_run 热重连）。
 *
 * 触发场景：WebView 重载/崩溃后 Rust run 仍存活——前端对运行中的 run 一无
 * 所见（帧通道已随旧 webview 消亡）。本模块在会话进入/恢复路径先调
 * `session_run_snapshot`：status=running → 进入**重连观察态**——
 * `attach_run(sessionId, channel, sinceSeq=0)` 从 0 重放最多 512 帧（重放
 * 窗口耗尽时后端补 runSnapshot 摘要帧），此后续流帧经同一通道热切换到达，
 * 帧喂独立 FrameBuffer 渲染只读实时视图（不注入 assistant-ui LocalRuntime
 * ——运行态 adapter 实例已随重载消亡，树协议不动）。
 *
 * 帧序自愈（前端半）：
 * - 帧 seq 必须 > 已见最大 seq 否则丢弃（"已见最大 seq"为会话级状态，
 *   不再是 runtime.ts 里单 adapter 的局部变量）；
 * - seq 缺口（跳跃）记 console 并由无帧兜底对账收口；
 * - runEnded 帧丢失兜底：5 秒无新帧且快照 status=idle → 收口重载终态。
 *
 * 待审批恢复（重连时查询后端仍有效的待审批/
 * 询问请求（`session_pending_ask`），命中即恢复 AskPanel（原始操作及参数
 * 照常展示）；已失效的请求由后端 E_NOT_FOUND 拒绝（AskPanel 收到即关闭）
 * ——重载本身不代表批准或拒绝，attach 不改变 run 生命周期或审批决定。
 *
 * 渲染面：ReconnectOverlay（chat/ReconnectOverlay.tsx）消费本 store 的
 * 观察态快照；收口后经 `reloadTick` 信号触发 canonical 树重载。
 */

import { create } from "zustand";
import * as client from "@/ipc/client";
import type { SequencedFrame } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { applyAuditFrame } from "@/stores/audits";
import { applySubagentFrame } from "@/stores/subagents";
import { FrameBuffer, type FrameSnapshot } from "./frames";

/** 无帧兜底窗口（runEnded 帧丢失兜底）：5 秒无新帧且快照 status=idle
 *  → 重载 canonical 终态收口。 */
export const FRAME_IDLE_TIMEOUT_MS = 5000;

/** 兜底定时器的默认轮询间隔（每秒检查一次最后帧时刻）。 */
export const IDLE_CHECK_INTERVAL_MS = 1000;

/** 单会话的观察态作用域。 */
export interface ReconnectScope {
  /** 观察态相位（idle = 不在观察态）。 */
  phase: "idle" | "observing";
  /** 观察态渲染数据（独立 FrameBuffer 的全量累计快照）。 */
  snapshot: FrameSnapshot | null;
  /** 已见最大帧序号（帧序自愈记账；-1 = 尚未见过任何帧）。 */
  maxSeq: number;
  /** 最后一帧到达时刻（Date.now() 毫秒；无帧兜底判定的基准）。 */
  lastFrameAt: number;
}

/** 重连观察态仓库的形状（方法说明见各字段行内注释）。 */
export interface RunReconnectState {
  /** sessionId → 观察态作用域（缺省 = idle / 无快照 / maxSeq -1）。 */
  scopes: Record<string, ReconnectScope>;
  /** 收口信号：runEnded（或兜底）退出观察态后自增，SessionTab 依赖它重载
   *  canonical 树（"runEnded 后树完整收口"）。 */
  reloadTick: Record<string, number>;
  /** 进入观察态（phase=observing，快照清空）。 */
  enter: (sessionId: string) => void;
  /** 退出观察态并发出树重载信号。 */
  exit: (sessionId: string) => void;
  /** 记账一帧：更新会话级 maxSeq 与渲染快照（帧已过 runEpoch 过滤）。 */
  noteFrame: (sessionId: string, seq: number, snapshot: FrameSnapshot) => void;
  /** 会话关闭/删除时丢弃其观察态作用域。 */
  forgetSession: (sessionId: string) => void;
}

/** 空作用域：不在观察态、未见任何帧。 */
const emptyScope = (): ReconnectScope => ({
  phase: "idle",
  snapshot: null,
  maxSeq: -1,
  lastFrameAt: 0,
});

/** 重连观察态仓库实例（组件用 selector 订阅，非 React 场景用 getState()）。 */
export const useRunReconnectStore = create<RunReconnectState>((set) => ({
  scopes: {},
  reloadTick: {},

  enter: (sessionId) =>
    set((s) => ({
      scopes: {
        ...s.scopes,
        // lastFrameAt 起算点 = 进入时刻：进入后 5 秒静默期（长工具执行不误触对账轮询）
        [sessionId]: { ...emptyScope(), phase: "observing" as const, lastFrameAt: Date.now() },
      },
    })),

  exit: (sessionId) =>
    set((s) => {
      const scopes = { ...s.scopes };
      const prior = scopes[sessionId] ?? emptyScope();
      // 保留 runEpoch 记账（同会话后续重连判定仍有意义），相位回 idle
      scopes[sessionId] = { ...prior, phase: "idle" as const, snapshot: null };
      return {
        scopes,
        reloadTick: { ...s.reloadTick, [sessionId]: (s.reloadTick[sessionId] ?? 0) + 1 },
      };
    }),

  noteFrame: (sessionId, seq, snapshot) =>
    set((s) => {
      const scope = s.scopes[sessionId] ?? emptyScope();
      return {
        scopes: {
          ...s.scopes,
          [sessionId]: {
            ...scope,
            maxSeq: Math.max(scope.maxSeq, seq),
            snapshot,
            lastFrameAt: Date.now(),
          },
        },
      };
    }),

  forgetSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.scopes) && !(sessionId in s.reloadTick)) return s;
      const scopes = { ...s.scopes };
      const reloadTick = { ...s.reloadTick };
      delete scopes[sessionId];
      delete reloadTick[sessionId];
      return { scopes, reloadTick };
    }),
}));

/** 重连流程的依赖面（全部可注入——测试换假实现）。 */
export interface ReconnectDeps {
  sessionRunSnapshot: typeof client.sessionRunSnapshot;
  attachRun: typeof client.attachRun;
  cancelRun: typeof client.cancelRun;
  sessionPendingAsk: typeof client.sessionPendingAsk;
  /** runEnded/兜底收口：重载 canonical 树并恢复渲染（生产 = SessionTab 的
   *  load_session_tree 重 import；测试注入 spy 断言）。 */
  onEnded: () => Promise<void>;
  /** 兜底定时器轮询间隔（测试可缩短）。 */
  idleCheckIntervalMs?: number;
}

/** 生产依赖：ipc/client 真实出口；onEnded 由调用方注入（树重载是视图层职责）。 */
export function defaultReconnectDeps(): ReconnectDeps {
  return {
    sessionRunSnapshot: client.sessionRunSnapshot,
    attachRun: client.attachRun,
    cancelRun: client.cancelRun,
    sessionPendingAsk: client.sessionPendingAsk,
    onEnded: async () => {},
  };
}

/**
 * 会话进入/恢复路径的统一入口（SessionTab 挂载时调用）：
 * ① `session_run_snapshot` → status=running → 进入重连观察态（attach 重放续流）；
 * ② 无论是否进入观察态，都查一次待审批请求并恢复 AskPanel（重连
 *    时刻审批可能仍挂起，与观察态无耦合）。
 * 返回是否进入了观察态。
 */
export async function reconnectIfNeeded(
  sessionId: string,
  overrides: Partial<ReconnectDeps> = {},
): Promise<boolean> {
  const deps = { ...defaultReconnectDeps(), ...overrides };
  let snap: Awaited<ReturnType<typeof deps.sessionRunSnapshot>> = null;
  try {
    snap = await deps.sessionRunSnapshot(sessionId);
  } catch (err) {
    // IPC 失败按无 run 处理（正常会话进入路径不受影响）
    console.warn("[reconnect] 运行快照查询失败（按无运行中任务处理）", err);
  }
  await restorePendingAsk(sessionId, overrides);
  if (snap?.status !== "running") {
    return false;
  }
  return observeRun(sessionId, deps);
}

/**
 * 待审批/询问恢复：查询后端仍有效的未决请求，命中即
 * 按 askId 登记 AskPanel（原始载荷随行）。查询与实时 ask:opened/closed
 * 事件按 askId 对账——后开覆盖先开是 ask store 的既有语义，已解决的请求
 * 经 ask:closed 事件或 resolve 的 E_NOT_FOUND 关闭（AskPanel 侧）。
 * 返回是否恢复了未决请求。
 */
export async function restorePendingAsk(
  sessionId: string,
  overrides: Partial<ReconnectDeps> = {},
): Promise<boolean> {
  const deps = { ...defaultReconnectDeps(), ...overrides };
  let pending: Awaited<ReturnType<typeof deps.sessionPendingAsk>> = null;
  try {
    pending = await deps.sessionPendingAsk(sessionId);
  } catch (err) {
    console.warn("[reconnect] 待审批查询失败（跳过恢复）", err);
    return false;
  }
  if (!pending) return false;
  useAskStore.getState().open(sessionId, pending.askId, pending.payload);
  return true;
}

/**
 * 进入观察态：attach_run(sinceSeq=0) 重放 + 续流；帧喂独立
 * FrameBuffer，帧序过滤与会话级 maxSeq 记账在此处完成（重放帧中的
 * runStarted 不触发运行态节点记账——本路径不触碰 noteRuntimeNode，观察态
 * 旁路双写协议）；runEnded → 收口（exit + onEnded 树重载）；5 秒无
 * 帧且快照 status=idle → 兜底收口（runEnded 丢失）。
 */
async function observeRun(sessionId: string, deps: ReconnectDeps): Promise<boolean> {
  // 防重入：run:start 事件驱动的自动接入与本路径并发时，已在
  // 观察态（同一 run 正在续流）则不再进第二次 attach
  if (useRunReconnectStore.getState().scopes[sessionId]?.phase === "observing") {
    return false;
  }
  useRunReconnectStore.getState().enter(sessionId);
  const buffer = new FrameBuffer();
  let finished = false;
  let timer: ReturnType<typeof setInterval> | undefined;
  const finish = async () => {
    if (finished) return;
    finished = true;
    if (timer !== undefined) clearInterval(timer);
    useRunReconnectStore.getState().exit(sessionId);
    try {
      await deps.onEnded();
    } catch (err) {
      console.warn("[reconnect] 终态收口（树重载）失败", err);
    }
  };
  // 无帧兜底：5 秒无新帧且快照 status=idle → 收口重载终态
  timer = setInterval(() => {
    const scope = useRunReconnectStore.getState().scopes[sessionId];
    if (scope?.phase !== "observing") return;
    if (Date.now() - scope.lastFrameAt < FRAME_IDLE_TIMEOUT_MS) return;
    void (async () => {
      try {
        const snap = await deps.sessionRunSnapshot(sessionId);
        if (snap?.status !== "running") await finish();
      } catch {
        // 查询失败下一轮再试（run 侧网络抖动不立即收口）
      }
    })();
  }, deps.idleCheckIntervalMs ?? IDLE_CHECK_INTERVAL_MS);
  try {
    const result = await deps.attachRun(
      sessionId,
      (frame: SequencedFrame) => {
        const store = useRunReconnectStore.getState();
        const maxSeen = store.scopes[sessionId]?.maxSeq ?? -1;
        // 帧序自愈：帧 seq 必须 > 已见最大，否则丢弃
        if (frame.seq <= maxSeen) {
          return;
        }
        // seq 缺口（跳跃）记 console——收口判定交给无帧兜底对账
        if (frame.seq > maxSeen + 1) {
          console.warn(`[reconnect] 帧序缺口 ${maxSeen} → ${frame.seq}（会话 ${sessionId}）`);
        }
        buffer.push(frame);
        store.noteFrame(sessionId, frame.seq, buffer.snapshot());
        // 子代理 4 帧：重放路径同步 subagents store（重载后
        // 运行中子代理列表由重放恢复，与事件通道幂等）
        applySubagentFrame(sessionId, frame);
        // 审计状态帧：重放路径同步 audits store（重载恢复审计行）
        applyAuditFrame(sessionId, frame);
        if (frame.type === "runEnded") void finish();
      },
      0,
    );
    if (result.runId === undefined) {
      // 快照与 attach 之间 run 已收口（竞态）：直接退出观察态并重载终态
      await finish();
      return false;
    }
  } catch (err) {
    console.warn("[reconnect] attach_run 失败（退出观察态）", err);
    await finish();
    return false;
  }
  return true;
}

/** 观察态取消（ReconnectOverlay 的取消按钮）：cancel_run 置位，终态以
 *  runEnded(cancelled) 帧为权威（收口由帧驱动路径完成）。 */
export function cancelObservedRun(
  sessionId: string,
  overrides: Partial<ReconnectDeps> = {},
): Promise<boolean> {
  const deps = { ...defaultReconnectDeps(), ...overrides };
  return deps.cancelRun(sessionId);
}
