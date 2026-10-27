// @vitest-environment happy-dom
/**
 * ask/askHint store 与事件桥测试（§10.1/§3.5，14 号票）：
 * store 纯逻辑 + useBackendEvents 事件分发（listen 注入 fake）。
 */

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useBackendEvents } from "@/chat/event-bridge";
import { useRunMetricsStore } from "@/chat/run-metrics";
import type { AskOpenedEvent, TokensUpdateEvent } from "@/ipc/types";
import { matchGhost, useAskStore } from "@/stores/ask";

afterEach(() => {
  cleanup();
  useAskStore.setState({ current: null, hint: null });
  useRunMetricsStore.setState(useRunMetricsStore.getInitialState());
});

describe("ask store", () => {
  it("open/close 按 askId 匹配清除（他 id 不误伤）", () => {
    useAskStore.getState().open("a1", {
      kind: "approval",
      title: "t",
      options: [],
    });
    expect(useAskStore.getState().current?.askId).toBe("a1");
    useAskStore.getState().close("other");
    expect(useAskStore.getState().current).not.toBeNull();
    useAskStore.getState().close("a1");
    expect(useAskStore.getState().current).toBeNull();
  });

  it("hint 置位与清除", () => {
    useAskStore.getState().setHint("试试让我总结");
    expect(useAskStore.getState().hint).toBe("试试让我总结");
    useAskStore.getState().clearHint();
    expect(useAskStore.getState().hint).toBeNull();
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

describe("useBackendEvents（事件分发）", () => {
  it("ask:opened/closed、run:suggestions、tokens:update 各达其 store", async () => {
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
    await vi.waitFor(() => expect(handlers.size).toBe(4));

    // ask:opened → ask store
    const opened: AskOpenedEvent = {
      sessionId: "s",
      askId: "a9",
      payload: { kind: "question", title: "t", questions: [] },
    };
    act(() => handlers.get("ask:opened")?.(opened));
    expect(useAskStore.getState().current?.askId).toBe("a9");

    // run:suggestions → hint
    act(() => handlers.get("run:suggestions")?.({ sessionId: "s", runId: "r", text: "建议词" }));
    expect(useAskStore.getState().hint).toBe("建议词");

    // tokens:update → sessionUsage 权威 replace（帧累计被校正）
    useRunMetricsStore.setState({
      sessionUsage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0 },
    });
    const tokens: TokensUpdateEvent = {
      sessionId: "s",
      usage: { input: 360, output: 144, cacheRead: 0, cacheWrite: 0 },
    };
    act(() => handlers.get("tokens:update")?.(tokens));
    expect(useRunMetricsStore.getState().sessionUsage).toEqual(tokens.usage);

    // ask:closed → 清除
    act(() => handlers.get("ask:closed")?.({ askId: "a9", answered: true }));
    expect(useAskStore.getState().current).toBeNull();
  });

  it("卸载时取消全部订阅", async () => {
    const unlistens = [vi.fn(), vi.fn(), vi.fn(), vi.fn()];
    let i = 0;
    const deps = {
      listen: vi.fn(async () => {
        const fn = unlistens[i] ?? vi.fn();
        i += 1;
        return fn;
      }),
    };
    const { unmount } = renderHook(() => useBackendEvents(deps));
    await vi.waitFor(() => expect(deps.listen).toHaveBeenCalledTimes(4));
    unmount();
    // cleanup 里 unlisten.then(fn => fn()) 是微任务，等待其落地
    await vi.waitFor(() => {
      for (const fn of unlistens) expect(fn).toHaveBeenCalled();
    });
  });
});
