// @vitest-environment happy-dom
/**
 * 压缩 store 与事件桥三键测试（FR-03-05，04 号票）：
 * 一部分测 compaction store 纯逻辑（分桶/投影/phase），一部分测
 * useBackendEvents 对 run:compacting / run:compacted / run:compact_failed
 * 三键的分发（参考 stores/ask.test.ts 的 fake listen 形态）。
 */

import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useCompactionStore } from "@/chat/compaction-store";
import { useBackendEvents } from "@/chat/event-bridge";
import type { StoredCompactionEntry } from "@/ipc/types";

afterEach(() => {
  useCompactionStore.setState({ bySession: {} });
});

/** 造一条压缩边界投影（生成物 StoredCompactionEntry 为裸载荷形态）。 */
const compaction = (cut: string): StoredCompactionEntry => ({
  id: `manual:compact:${cut}`,
  cutEntryId: cut,
  summary: "摘要",
  tokensBefore: 1000,
  createdAt: 1,
});

describe("useCompactionStore（压缩状态分桶）", () => {
  it("setCompactions / setPhase 各自写同会话桶互不覆盖", () => {
    act(() => {
      useCompactionStore.getState().setCompactions("s1", [compaction("a1")]);
      useCompactionStore.getState().setPhase("s1", "compacting");
      useCompactionStore.getState().setPhase("s2", "failed");
    });
    expect(useCompactionStore.getState().bySession.s1).toEqual({
      compactions: [compaction("a1")],
      phase: "compacting",
    });
    expect(useCompactionStore.getState().bySession.s2?.phase).toBe("failed");
    expect(useCompactionStore.getState().bySession.s2?.compactions).toEqual([]);
  });

  it("forSession 删除后清桶；未删除会话不受影响", () => {
    act(() => {
      useCompactionStore.getState().setCompactions("s1", [compaction("a1")]);
      useCompactionStore.getState().setCompactions("s2", []);
    });
    act(() => useCompactionStore.getState().forgetSession("s1"));
    expect(useCompactionStore.getState().bySession.s1).toBeUndefined();
    expect(useCompactionStore.getState().bySession.s2).toBeDefined();
  });
});

describe("useBackendEvents 压缩三键（04 号票）", () => {
  it("run:compacting/compacted/compact_failed 各达 compaction store", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    const reload = vi.fn(async () => ({
      sessionId: "s",
      messages: [],
      compactions: [compaction("a2")],
      revision: 9,
    }));
    const deps = {
      listen: vi.fn(async (key: string, handler: (payload: never) => void) => {
        handlers.set(key, handler as unknown as (payload: unknown) => void);
        return () => {
          handlers.delete(key);
        };
      }),
      reloadCompactions: reload,
    };
    renderHook(() => useBackendEvents(deps));
    await vi.waitFor(() => expect(handlers.size).toBe(8));

    // compacting → 指示条 phase
    act(() => handlers.get("run:compacting")?.({ sessionId: "s" }));
    expect(useCompactionStore.getState().bySession.s?.phase).toBe("compacting");

    // compact_failed → 行内轻提示 phase（不打断输入）
    act(() => handlers.get("run:compact_failed")?.({ sessionId: "s" }));
    expect(useCompactionStore.getState().bySession.s?.phase).toBe("failed");

    // compacted → 收口 + 边界投影刷新（reloadCompactions 拉最新史册）
    act(() =>
      handlers.get("run:compacted")?.({ sessionId: "s", tokensBefore: 900, tokensAfter: 300 }),
    );
    expect(useCompactionStore.getState().bySession.s?.phase).toBeNull();
    await vi.waitFor(() => expect(reload).toHaveBeenCalledWith("s"));
    expect(useCompactionStore.getState().bySession.s?.compactions).toEqual([compaction("a2")]);
  });

  it("compacted 收口时边界刷新失败：保留旧投影不炸", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    const deps = {
      listen: vi.fn(async (key: string, handler: (payload: never) => void) => {
        handlers.set(key, handler as unknown as (payload: unknown) => void);
        return () => {
          handlers.delete(key);
        };
      }),
      reloadCompactions: vi.fn(async () => {
        throw new Error("ipc down");
      }),
    };
    act(() => {
      useCompactionStore.getState().setCompactions("s", [compaction("a1")]);
      useCompactionStore.getState().setPhase("s", "compacting");
    });
    renderHook(() => useBackendEvents(deps));
    await vi.waitFor(() => expect(handlers.size).toBe(8));
    act(() =>
      handlers.get("run:compacted")?.({ sessionId: "s", tokensBefore: 900, tokensAfter: 300 }),
    );
    // phase 收口；投影保留旧值（下次会话进入时收养自愈）
    expect(useCompactionStore.getState().bySession.s?.phase).toBeNull();
    expect(useCompactionStore.getState().bySession.s?.compactions).toEqual([compaction("a1")]);
    // 未处理的 rejection 不外溢（事件桥内已 catch）
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
});
