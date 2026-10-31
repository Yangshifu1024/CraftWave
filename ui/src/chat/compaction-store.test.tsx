// @vitest-environment happy-dom
/**
 * 压缩 store 与事件桥测试：compaction store 纯逻辑
 * （分桶/投影/phase）+ useBackendEvents 对 run:compacting / run:compacted /
 * run:compact_failed 三键的分发（参考 stores/ask.test.ts 的 fake listen 形态）。
 */

import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useCompactionStore } from "@/chat/compaction-store";
import { useBackendEvents } from "@/chat/event-bridge";
import type { StoredCompactionEntry } from "@/ipc/types";
import { useToastStore } from "@/stores/toast";

afterEach(() => {
  useCompactionStore.setState({ bySession: {} });
  useToastStore.setState({ toasts: [] });
});

/** 造一条压缩边界投影（生成物 StoredCompactionEntry 为裸载荷形态）。 */
const compaction = (cut: string): StoredCompactionEntry => ({
  id: `manual:compact:${cut}`,
  cutEntryId: cut,
  summary: "摘要",
  tokensBefore: 1000,
  createdAt: 1,
});

/** fake listen：捕获各事件键的 handler（事件桥注册 18 个键）。 */
function fakeListen() {
  const handlers = new Map<string, (payload: unknown) => void>();
  return {
    handlers,
    listen: vi.fn(async (key: string, handler: (payload: never) => void) => {
      handlers.set(key, handler as unknown as (payload: unknown) => void);
      return () => {
        handlers.delete(key);
      };
    }),
  };
}

describe("compaction", () => {
  it("store 分桶（setCompactions/setPhase/forgetSession）+ 事件桥三键分发（指示条/Toast/收口刷新）+ 刷新失败保留旧投影", async () => {
    // ---- useCompactionStore：setCompactions / setPhase 各自写同会话桶互不覆盖 ----
    act(() => {
      useCompactionStore.getState().setCompactions("s1", [compaction("a1")]);
      useCompactionStore.getState().setPhase("s1", "compacting");
      useCompactionStore.getState().setPhase("s2", null);
    });
    expect(useCompactionStore.getState().bySession.s1).toEqual({
      compactions: [compaction("a1")],
      phase: "compacting",
    });
    expect(useCompactionStore.getState().bySession.s2?.phase).toBeNull();
    expect(useCompactionStore.getState().bySession.s2?.compactions).toEqual([]);

    // ---- useBackendEvents 压缩三键 ----
    const first = fakeListen();
    const reload = vi.fn(async () => ({
      sessionId: "s",
      messages: [],
      compactions: [compaction("a2")],
      revision: 9,
    }));
    renderHook(() => useBackendEvents({ listen: first.listen, reloadCompactions: reload }));
    await vi.waitFor(() => expect(first.handlers.size).toBe(18));

    // compacting → 指示条 phase
    act(() => first.handlers.get("run:compacting")?.({ sessionId: "s" }));
    expect(useCompactionStore.getState().bySession.s?.phase).toBe("compacting");

    // compact_failed → Toast 反馈 + phase 复位（指示条撤除）
    act(() => first.handlers.get("run:compact_failed")?.({ sessionId: "s" }));
    expect(useCompactionStore.getState().bySession.s?.phase).toBeNull();
    const errorToast = useToastStore
      .getState()
      .toasts.find((t) => t.kind === "error" && t.title.includes("压缩失败"));
    expect(errorToast).toBeDefined();

    // compacted → 收口 + 边界投影刷新（reloadCompactions 拉最新史册）
    act(() =>
      first.handlers.get("run:compacted")?.({
        sessionId: "s",
        tokensBefore: 900,
        tokensAfter: 300,
      }),
    );
    expect(useCompactionStore.getState().bySession.s?.phase).toBeNull();
    await vi.waitFor(() => expect(reload).toHaveBeenCalledWith("s"));
    expect(useCompactionStore.getState().bySession.s?.compactions).toEqual([compaction("a2")]);

    // ---- compacted 收口时边界刷新失败：保留旧投影不炸 ----
    const second = fakeListen();
    renderHook(() =>
      useBackendEvents({
        listen: second.listen,
        reloadCompactions: vi.fn(async () => {
          throw new Error("ipc down");
        }),
      }),
    );
    await vi.waitFor(() => expect(second.handlers.size).toBe(18));
    act(() => {
      useCompactionStore.getState().setCompactions("s9", [compaction("a1")]);
      useCompactionStore.getState().setPhase("s9", "compacting");
    });
    act(() =>
      second.handlers.get("run:compacted")?.({
        sessionId: "s9",
        tokensBefore: 900,
        tokensAfter: 300,
      }),
    );
    // phase 收口；投影保留旧值（下次会话进入时收养自愈）
    expect(useCompactionStore.getState().bySession.s9?.phase).toBeNull();
    expect(useCompactionStore.getState().bySession.s9?.compactions).toEqual([compaction("a1")]);
    // 未处理的 rejection 不外溢（事件桥内已 catch）
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
});
