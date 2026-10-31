// @vitest-environment happy-dom
/**
 * ask/askHint store 与事件桥测试（store 分域 + 按会话分桶）：
 * store 纯逻辑（开合/分桶/ghost 匹配）与 useBackendEvents 事件分发（事件监听
 * 用注入的 fake 假实现替代真实 IPC，不依赖 Rust 后端）。
 */

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useBackendEvents } from "@/chat/event-bridge";
import { useRunMetricsStore } from "@/chat/run-metrics";
import type { AskOpenedEvent, AskPayload, SessionTitleEvent, TokensUpdateEvent } from "@/ipc/types";
import { matchGhost, useAskStore } from "@/stores/ask";
import { useSessionsStore } from "@/stores/sessions";

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
  useSessionsStore.setState({ tabs: [], sessions: [] });
});

describe("ask store 与事件桥", () => {
  it("store 剧本（bySession+hints 双桶语义）：open/close 按 askId 匹配清除、他会话不误伤、hint 隔离、forgetSession 清两桶、matchGhost 规则", () => {
    // open/close 按 askId 匹配清除（他 id 不误伤、他会话不误伤）
    useAskStore.getState().open("s1", "a1", approval);
    useAskStore.getState().open("s2", "a2", approval);
    expect(useAskStore.getState().bySession.s1?.askId).toBe("a1");
    useAskStore.getState().close("s1", "other");
    expect(useAskStore.getState().bySession.s1).not.toBeNull();
    useAskStore.getState().close("s1", "a1");
    expect(useAskStore.getState().bySession.s1).toBeUndefined();
    // 他会话不受影响
    expect(useAskStore.getState().bySession.s2?.askId).toBe("a2");

    // hint 置位与清除（按会话隔离）
    useAskStore.getState().setHint("s1", "试试让我总结");
    useAskStore.getState().setHint("s2", "另一个会话的建议");
    expect(useAskStore.getState().hints.s1).toBe("试试让我总结");
    useAskStore.getState().clearHint("s1");
    expect(useAskStore.getState().hints.s1).toBeUndefined();
    expect(useAskStore.getState().hints.s2).toBe("另一个会话的建议");

    // forgetSession 清两桶
    useAskStore.getState().open("s3", "a3", approval);
    useAskStore.getState().setHint("s3", "建议");
    useAskStore.getState().forgetSession("s3");
    expect(useAskStore.getState().bySession.s3).toBeUndefined();
    expect(useAskStore.getState().hints.s3).toBeUndefined();

    // matchGhost：空输入整句 / 前缀补尾 / 不匹配与已完整为 null
    expect(matchGhost("试试总结配置", "")).toBe("试试总结配置");
    expect(matchGhost("试试总结配置", "试试")).toBe("总结配置");
    expect(matchGhost("试试总结配置", "试试总结配置")).toBeNull();
    expect(matchGhost("试试总结配置", "无关输入")).toBeNull();
    expect(matchGhost(null, "")).toBeNull();
  });

  it("useBackendEvents 剧本：ask:opened/closed、run:suggestions、tokens:update、session:title 各达其 store；卸载取消全部订阅", async () => {
    // listen fake：登记回调与 unlisten 桩，测试随后手动触发事件验证分发
    const handlers = new Map<string, (payload: unknown) => void>();
    const unlistens: (() => void)[] = [];
    const deps = {
      listen: vi.fn(async (key: string, handler: (payload: never) => void) => {
        handlers.set(key, handler as unknown as (payload: unknown) => void);
        const fn = vi.fn();
        unlistens.push(fn);
        return fn;
      }),
    };
    const { unmount } = renderHook(() => useBackendEvents(deps));
    await vi.waitFor(() => expect(handlers.size).toBe(18));

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

    // session:title → sessions store 标题就地更新（tabs + 列表）
    const metaOf = (id: string, title: string) => ({
      id,
      title,
      createdAt: 0,
      updatedAt: 0,
      revision: 0,
      messageCount: 1,
      totalTokens: 0,
      authorizedExternalDirs: [],
    });
    useSessionsStore.setState({
      tabs: [metaOf("s", "新会话"), metaOf("s2", "别的会话")],
      sessions: [metaOf("s", "新会话"), metaOf("s2", "别的会话")],
    });
    const title: SessionTitleEvent = { sessionId: "s", title: "自动起的标题" };
    act(() => handlers.get("session:title")?.(title));
    expect(useSessionsStore.getState().tabs.find((t) => t.id === "s")?.title).toBe("自动起的标题");
    expect(useSessionsStore.getState().sessions.find((m) => m.id === "s")?.title).toBe(
      "自动起的标题",
    );

    // ask:closed → 按 askId 全桶定位清除
    act(() => handlers.get("ask:closed")?.({ askId: "a9", answered: true }));
    expect(useAskStore.getState().bySession.s).toBeUndefined();

    // 卸载：cleanup 里 unlisten.then(fn => fn()) 是微任务，等待其全部落地
    unmount();
    await vi.waitFor(() => {
      for (const fn of unlistens) expect(fn).toHaveBeenCalled();
    });
  });
});
