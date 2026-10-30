// @vitest-environment happy-dom
/**
 * ask/askHint store 与事件桥测试（§10.1/§3.5，14 号票；16 号票改按会话分桶）：
 * 一部分测 store 纯逻辑（开合/分桶/ghost 匹配），一部分测 useBackendEvents
 * 事件分发（事件监听用注入的 fake 假实现替代真实 IPC，不依赖 Rust 后端）。
 */

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useBackendEvents } from "@/chat/event-bridge";
import { useRunMetricsStore } from "@/chat/run-metrics";
import type { AskOpenedEvent, AskPayload, TokensUpdateEvent } from "@/ipc/types";
import { matchGhost, useAskStore } from "@/stores/ask";

/** 最小可用的审批载荷样例（字段值本身不参与断言）。 */
const approval: AskPayload = {
  kind: "approval",
  title: "t",
  options: [],
};

// 每个用例后清理：卸载组件并重置两个全局 store，保证用例互不污染
afterEach(() => {
  cleanup();
  useAskStore.setState({ bySession: {}, hints: {} });
  useRunMetricsStore.setState(useRunMetricsStore.getInitialState());
});

describe("ask store（按会话分桶）", () => {
  it("open/close 按 askId 匹配清除（他 id 不误伤、他会话不误伤）", () => {
    useAskStore.getState().open("s1", "a1", approval);
    useAskStore.getState().open("s2", "a2", approval);
    expect(useAskStore.getState().bySession.s1?.askId).toBe("a1");
    useAskStore.getState().close("s1", "other");
    expect(useAskStore.getState().bySession.s1).not.toBeNull();
    useAskStore.getState().close("s1", "a1");
    expect(useAskStore.getState().bySession.s1).toBeUndefined();
    // 他会话不受影响
    expect(useAskStore.getState().bySession.s2?.askId).toBe("a2");
  });

  it("hint 置位与清除（按会话隔离）", () => {
    useAskStore.getState().setHint("s1", "试试让我总结");
    useAskStore.getState().setHint("s2", "另一个会话的建议");
    expect(useAskStore.getState().hints.s1).toBe("试试让我总结");
    useAskStore.getState().clearHint("s1");
    expect(useAskStore.getState().hints.s1).toBeUndefined();
    expect(useAskStore.getState().hints.s2).toBe("另一个会话的建议");
  });

  it("forgetSession 清两桶", () => {
    useAskStore.getState().open("s1", "a1", approval);
    useAskStore.getState().setHint("s1", "建议");
    useAskStore.getState().forgetSession("s1");
    expect(useAskStore.getState().bySession.s1).toBeUndefined();
    expect(useAskStore.getState().hints.s1).toBeUndefined();
  });
});

describe("matchGhost（§10.4 ghost 匹配）", () => {
  it("空输入整句 / 前缀补尾 / 不匹配与已完整为 null", () => {
    expect(matchGhost("试试总结配置", "")).toBe("试试总结配置");
    expect(matchGhost("试试总结配置", "试试")).toBe("总结配置");
    expect(matchGhost("试试总结配置", "试试总结配置")).toBeNull();
    expect(matchGhost("试试总结配置", "无关输入")).toBeNull();
    expect(matchGhost(null, "")).toBeNull();
  });
});

describe("useBackendEvents（事件分发，按 sessionId 入桶）", () => {
  it("ask:opened/closed、run:suggestions、tokens:update 各达其 store", async () => {
    // listen fake：把每个事件键的回调登记进 Map，测试随后手动触发它们，
    // 即可验证事件桥是否把载荷分发到了正确的 store 桶
    const handlers = new Map<string, (payload: unknown) => void>();
    const deps = {
      listen: vi.fn(async (key: string, handler: (payload: never) => void) => {
        handlers.set(key, handler as unknown as (payload: unknown) => void);
        return () => {
          handlers.delete(key);
        };
      }),
    };
    renderHook(() => useBackendEvents(deps));
    await vi.waitFor(() => expect(handlers.size).toBe(17));

    // ask:opened → ask store（按 sessionId 分桶）
    const opened: AskOpenedEvent = {
      sessionId: "s",
      askId: "a9",
      payload: { kind: "question", title: "t", questions: [] },
    };
    act(() => handlers.get("ask:opened")?.(opened));
    expect(useAskStore.getState().bySession.s?.askId).toBe("a9");

    // run:suggestions → hint（同会话桶）
    act(() => handlers.get("run:suggestions")?.({ sessionId: "s", runId: "r", text: "建议词" }));
    expect(useAskStore.getState().hints.s).toBe("建议词");

    // tokens:update → sessionUsage 权威 replace（帧累计被校正）
    useRunMetricsStore.setState({
      bySession: {
        s: useRunMetricsStore.getInitialState().bySession.s ?? {
          rate: null,
          turnUsage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          sessionUsage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0 },
          lastRun: null,
        },
      },
    });
    const tokens: TokensUpdateEvent = {
      sessionId: "s",
      usage: { input: 360, output: 144, cacheRead: 0, cacheWrite: 0 },
    };
    act(() => handlers.get("tokens:update")?.(tokens));
    expect(useRunMetricsStore.getState().bySession.s?.sessionUsage).toEqual(tokens.usage);

    // ask:closed → 按 askId 全桶定位清除
    act(() => handlers.get("ask:closed")?.({ askId: "a9", answered: true }));
    expect(useAskStore.getState().bySession.s).toBeUndefined();
  });

  it("卸载时取消全部订阅", async () => {
    const unlistens = Array.from({ length: 17 }, () => vi.fn());
    let i = 0;
    const deps = {
      listen: vi.fn(async () => {
        const fn = unlistens[i] ?? vi.fn();
        i += 1;
        return fn;
      }),
    };
    const { unmount } = renderHook(() => useBackendEvents(deps));
    await vi.waitFor(() => expect(deps.listen).toHaveBeenCalledTimes(17));
    unmount();
    // cleanup 里 unlisten.then(fn => fn()) 是微任务，等待其落地
    await vi.waitFor(() => {
      for (const fn of unlistens) expect(fn).toHaveBeenCalled();
    });
  });
});
