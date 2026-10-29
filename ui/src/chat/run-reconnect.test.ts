/**
 * run 重连观察态纯逻辑测试（§5.6 runEpoch 自愈 / §5.8 attach 重放，05 号票）：
 * IPC 全部走注入的假实现，不依赖 Rust 后端。覆盖：
 * - 快照 running → 进入观察态并 attach(sinceSeq=0)；非 running 不进；
 * - 帧 runEpoch 过滤：重复/倒退 seq 丢弃；seq 缺口记 console 警告；
 * - runEnded 帧 → 退出观察态 + onEnded 收口（树重载信号）；
 * - 无帧兜底：5 秒无新帧且快照 idle → 收口（runEnded 帧丢失场景）；
 * - attach 空重放（runId 省略）→ 立即收口退出；
 * - 待审批恢复：查询命中登记 AskPanel、未命中不登记；
 * - 观察态旁路运行态节点记账（帧路径不触碰 noteRuntimeNode，§6.3 双写）。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { SequencedFrame } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { useSessionPersistStore } from "@/stores/session";
import {
  FRAME_IDLE_TIMEOUT_MS,
  type ReconnectDeps,
  reconnectIfNeeded,
  restorePendingAsk,
  useRunReconnectStore,
} from "./run-reconnect";

type FrameEmitter = (frame: SequencedFrame) => void;

/** 帧构造简写（ts 字段固定，seq/type/其余字段按用例给）。 */
function frame(seq: number, extra: Record<string, unknown>): SequencedFrame {
  return { seq, ts: 1, ...extra } as unknown as SequencedFrame;
}

/** attach 假实现：捕获 onFrame 回调（emitters 供测试体直接投递帧），返回值
 *  由用例经 impl 决定（默认挂起不返回——模拟 run 持续运行）。 */
function fakeAttach(impl?: (emit: FrameEmitter) => Promise<{ runId?: string; replayed: number }>) {
  const calls: { sessionId: string; sinceSeq: number }[] = [];
  const emitters: FrameEmitter[] = [];
  const attachRun = vi.fn((sessionId: string, onFrame: FrameEmitter, sinceSeq = 0) => {
    calls.push({ sessionId, sinceSeq });
    emitters.push(onFrame);
    if (impl) return impl(onFrame);
    return new Promise<{ runId?: string; replayed: number }>(() => {});
  });
  return { calls, emitters, attachRun };
}

/** 取最近一次 attach 的帧投递回调。 */
function captureEmitter(attach: ReturnType<typeof fakeAttach>): FrameEmitter {
  const emit = attach.emitters.at(-1);
  if (!emit) throw new Error("attach 尚未发生");
  return emit;
}

/** 构造依赖面：快照查询与收口 spy 可控。 */
function makeDeps(
  overrides: Partial<ReconnectDeps> = {},
): ReconnectDeps & { onEndedSpy: ReturnType<typeof vi.fn> } {
  const onEndedSpy = vi.fn(async () => {});
  const deps: ReconnectDeps = {
    sessionRunSnapshot: vi.fn(async () => null),
    attachRun: vi.fn(async () => ({ replayed: 0 })),
    cancelRun: vi.fn(async () => true),
    sessionPendingAsk: vi.fn(async () => null),
    onEnded: onEndedSpy,
    idleCheckIntervalMs: 10,
    ...overrides,
  };
  return { ...deps, onEndedSpy } as ReconnectDeps & { onEndedSpy: ReturnType<typeof vi.fn> };
}

/** 观察态 store 复位（用例互不污染）。 */
afterEach(() => {
  useRunReconnectStore.setState({
    scopes: {},
    reloadTick: {},
  });
  useAskStore.setState({ bySession: {}, hints: {} });
  useSessionPersistStore.setState({ scopes: {} });
  vi.restoreAllMocks();
});

describe("reconnectIfNeeded：观察态进入与退出", () => {
  it("快照 running → attach(sinceSeq=0) 重放 + 续流，runEnded 收口退出", async () => {
    const attach = fakeAttach(async (emit) => {
      // 重放帧（重载模拟：全量 0..=2）+ runEnded
      emit(frame(0, { type: "runStarted", runId: "r1", model: "m" }));
      emit(frame(1, { type: "textDelta", turn: 1, delta: "你好" }));
      emit(frame(2, { type: "textDelta", turn: 1, delta: "呀" }));
      emit(
        frame(3, {
          type: "runEnded",
          reason: "done",
          durationMs: 5,
          toolCount: 0,
          turn: 1,
        }),
      );
      return { runId: "r1", replayed: 3 };
    });
    const deps = makeDeps({
      sessionRunSnapshot: vi.fn(async () => ({
        status: "running" as const,
        runId: "r1",
        startedAt: 1,
        turn: 1,
        toolCount: 0,
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        lastSeq: 3,
      })),
      attachRun: attach.attachRun as unknown as ReconnectDeps["attachRun"],
    });
    const entered = await reconnectIfNeeded("s1", deps);
    expect(entered, "快照 running → 进入观察态").toBe(true);
    expect(attach.calls).toEqual([{ sessionId: "s1", sinceSeq: 0 }]);
    const scope = useRunReconnectStore.getState().scopes.s1;
    expect(scope?.phase, "runEnded 后已退出观察态").toBe("idle");
    expect(scope?.maxSeq, "会话级 runEpoch 记账到最后帧").toBe(3);
    expect(deps.onEndedSpy, "runEnded → 收口回调（树重载）").toHaveBeenCalledTimes(1);
    expect(
      useRunReconnectStore.getState().reloadTick.s1,
      "收口信号自增（SessionTab 依赖它重载树）",
    ).toBe(1);
  });

  it("快照非 running（null / idle）→ 不进观察态不 attach", async () => {
    const attach = fakeAttach(async () => ({ replayed: 0 }));
    const deps = makeDeps({
      sessionRunSnapshot: vi.fn(async () => null),
      attachRun: attach.attachRun as unknown as ReconnectDeps["attachRun"],
    });
    expect(await reconnectIfNeeded("s2", deps)).toBe(false);
    expect(attach.calls).toEqual([]);
    expect(useRunReconnectStore.getState().scopes.s2?.phase ?? "idle").toBe("idle");
  });

  it("attach 返回空重放（runId 省略，attach 与快照之间的竞态）→ 立即收口退出", async () => {
    const attach = fakeAttach(async () => ({ replayed: 0 }));
    const deps = makeDeps({
      sessionRunSnapshot: vi.fn(async () => ({
        status: "running" as const,
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        lastSeq: 0,
      })),
      attachRun: attach.attachRun as unknown as ReconnectDeps["attachRun"],
    });
    const entered = await reconnectIfNeeded("s3", deps);
    expect(entered, "attach 时 run 已结束 → 不算进入观察态").toBe(false);
    expect(deps.onEndedSpy, "仍应收口重载终态").toHaveBeenCalledTimes(1);
    expect(useRunReconnectStore.getState().scopes.s3?.phase).toBe("idle");
  });

  it("帧 runEpoch：重复/倒退 seq 丢弃；seq 缺口记 console 警告", async () => {
    const attach = fakeAttach(async () => {
      await new Promise<void>(() => {}); // 挂起 attach：帧由测试体直接投递
      return { runId: "r1", replayed: 1 };
    });
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const deps = makeDeps({
      sessionRunSnapshot: vi.fn(async () => ({
        status: "running" as const,
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        lastSeq: 0,
      })),
      attachRun: attach.attachRun as unknown as ReconnectDeps["attachRun"],
    });
    // 不 await：attach 挂起期间由本用例直接驱动帧投递
    void reconnectIfNeeded("s4", deps);
    await vi.waitFor(() => {
      expect(attach.calls.length).toBe(1);
      expect(useRunReconnectStore.getState().scopes.s4?.phase).toBe("observing");
    });
    const emitter = captureEmitter(attach);
    emitter(frame(5, { type: "textDelta", turn: 1, delta: "a" }));
    emitter(frame(5, { type: "textDelta", turn: 1, delta: "dup" })); // 重复 → 丢
    emitter(frame(3, { type: "textDelta", turn: 1, delta: "old" })); // 倒退 → 丢
    emitter(frame(9, { type: "textDelta", turn: 1, delta: "gap" })); // 缺口（5→9）→ 收但警告
    const scope = useRunReconnectStore.getState().scopes.s4;
    expect(scope?.maxSeq).toBe(9);
    expect(scope?.snapshot?.segments).toEqual([{ kind: "text", text: "agap" }]);
    expect(
      warnSpy.mock.calls.some((args) => String(args[0]).includes("帧序缺口")),
      "seq 跳跃触发对账告警",
    ).toBe(true);
  });

  it("无帧兜底：超过 5 秒无新帧且快照 idle → 收口（runEnded 帧丢失场景）", async () => {
    const attach = fakeAttach(); // 默认挂起：run 持续运行，无帧到达
    // 快照查询：首轮 running（进入观察态），兜底轮询起返回 null（run 已终态）
    let snapshotCalls = 0;
    const deps = makeDeps({
      sessionRunSnapshot: vi.fn(async () => {
        snapshotCalls += 1;
        if (snapshotCalls <= 1) {
          return {
            status: "running" as const,
            usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
            lastSeq: 0,
          };
        }
        return null;
      }),
      attachRun: attach.attachRun as unknown as ReconnectDeps["attachRun"],
    });
    void reconnectIfNeeded("s5", deps);
    await vi.waitFor(() => {
      expect(useRunReconnectStore.getState().scopes.s5?.phase).toBe("observing");
    });
    // 一帧都不发：把 lastFrameAt 拨回 5 秒前，等待兜底轮询（10ms 间隔）触发
    useRunReconnectStore.setState((s) => ({
      scopes: {
        ...s.scopes,
        s5: { ...s.scopes.s5!, lastFrameAt: Date.now() - FRAME_IDLE_TIMEOUT_MS - 1 },
      },
    }));
    await vi.waitFor(() => {
      expect(deps.onEndedSpy, "快照 idle → 兜底收口").toHaveBeenCalledTimes(1);
    });
    expect(useRunReconnectStore.getState().scopes.s5?.phase).toBe("idle");
  });

  it("观察态旁路运行态节点记账：帧路径不触碰 noteRuntimeNode（§6.3 双写）", async () => {
    const noteRuntimeNode = vi.fn();
    useSessionPersistStore.setState((s) => ({ ...s, noteRuntimeNode }));
    const attach = fakeAttach(async (emit) => {
      emit(frame(0, { type: "runStarted", runId: "r-observe", model: "m" }));
      emit(frame(1, { type: "textDelta", turn: 1, delta: "x" }));
      return { runId: "r-observe", replayed: 2 };
    });
    const deps = makeDeps({
      sessionRunSnapshot: vi.fn(async () => ({
        status: "running" as const,
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        lastSeq: 1,
      })),
      attachRun: attach.attachRun as unknown as ReconnectDeps["attachRun"],
    });
    await reconnectIfNeeded("s6", deps);
    // 观察态帧全部消化（runStarted + textDelta 经 store 记账），但不触碰 persist
    expect(useRunReconnectStore.getState().scopes.s6?.snapshot?.started).toBe(true);
    expect(
      noteRuntimeNode,
      "重放帧中的 runStarted 不触发运行态节点记账（防污染 canonical 双写）",
    ).not.toHaveBeenCalled();
    expect(Object.keys(useSessionPersistStore.getState().scopes)).toEqual([]);
  });
});

describe("restorePendingAsk：待审批恢复查询（§5.6 #59c）", () => {
  const approvalPayload = {
    kind: "approval" as const,
    title: "写入新文件 a.rs",
    options: [{ id: "allow_once", label: "允许" }],
  };

  it("查询命中 → 按 askId 恢复 AskPanel（原始载荷随行）", async () => {
    const deps = makeDeps({
      sessionPendingAsk: vi.fn(async () => ({
        askId: "ask-1",
        runId: "r1",
        kind: "approval",
        payload: approvalPayload,
      })),
    });
    const restored = await restorePendingAsk("s7", deps);
    expect(restored).toBe(true);
    const current = useAskStore.getState().bySession.s7;
    expect(current?.askId).toBe("ask-1");
    expect(current?.payload).toEqual(approvalPayload);
  });

  it("查询未命中（已失效/无请求）→ 不登记", async () => {
    const deps = makeDeps({
      sessionPendingAsk: vi.fn(async () => null),
    });
    expect(await restorePendingAsk("s8", deps)).toBe(false);
    expect(useAskStore.getState().bySession.s8).toBeUndefined();
  });

  it("reconnectIfNeeded 无论是否进观察态都恢复待审批", async () => {
    const pendingAsk = vi.fn(async () => ({
      askId: "ask-2",
      runId: "r1",
      kind: "question" as const,
      payload: { kind: "question" as const, title: "t", questions: [] },
    }));
    const deps = makeDeps({
      sessionRunSnapshot: vi.fn(async () => null), // 无活跃 run
      sessionPendingAsk: pendingAsk as unknown as ReconnectDeps["sessionPendingAsk"],
    });
    await reconnectIfNeeded("s9", deps);
    expect(useAskStore.getState().bySession.s9?.askId).toBe("ask-2");
  });
});
