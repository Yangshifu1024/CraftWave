/**
 * 会话持久化前端侧测试（15 号票，§6.3 双写协议）：
 * toSavePayload 剔除/改写纯逻辑 + syncTreeBeforeSend 的 E_SESSION_REV 自愈 +
 * loadHistory 的 NotFound 降级。IPC 经 vi.mock 注入 fake（替代真实后端调用，
 * 测试不依赖 Rust 侧）。
 */

import type { ThreadMessage } from "@assistant-ui/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as client from "@/ipc/client";
import type { MessageEntry, SessionTreeData } from "@/ipc/types";
import {
  loadHistory,
  syncTreeBeforeSend,
  toSavePayload,
  useSessionPersistStore,
} from "@/stores/session";

// 整体替换 IPC 客户端：只提供本测试触达的命令（其余导出是空 mock，
// 防止经 toWire 依赖链引入真实实现）
vi.mock("@/ipc/client", () => ({
  toIpcError: (err: unknown) => err as { code: string; message: string },
  saveSessionTree: vi.fn(),
  loadSessionTree: vi.fn(),
  // runtime.ts 经 toWire 依赖链引入的其余导出（本测试不触达 IPC 真实现）
  startChat: vi.fn(),
  cancelRun: vi.fn(),
  loadToolOutcomes: vi.fn(),
  listSessions: vi.fn(),
}));

/** 造一条最小 ThreadMessage 测试样例（id/角色/文本即可驱动被测逻辑）。 */
function msg(
  id: string,
  role: "user" | "assistant",
  text: string,
  extra: { runId?: string; status?: unknown } = {},
): ThreadMessage {
  return {
    id,
    role,
    content: text.length > 0 ? [{ type: "text", text }] : [],
    createdAt: new Date(1_760_000_000_000),
    ...(extra.runId !== undefined ? { metadata: { runId: extra.runId } } : {}),
    ...(extra.status !== undefined ? { status: extra.status } : {}),
  } as unknown as ThreadMessage;
}

// 每个用例后清理：清空 mock 调用记录并把持久化 store 重置回初始状态
afterEach(() => {
  vi.clearAllMocks();
  useSessionPersistStore.setState(useSessionPersistStore.getInitialState());
});

describe("toSavePayload（运行态节点剔除与引用改写）", () => {
  it("无运行态节点：线性链 parentId + activeLeaf=末条", () => {
    const nodes = new Map<string, string>();
    const { messages, activeLeafId } = toSavePayload(
      [msg("u1", "user", "你好"), msg("a1", "assistant", "答复")],
      nodes,
    );
    expect(messages.map((m) => m.id)).toEqual(["u1", "a1"]);
    expect(messages[0].parentId).toBeUndefined();
    expect(messages[1].parentId).toBe("u1");
    expect(activeLeafId).toBe("a1");
  });

  it("运行态节点剔除：其子引用改写为 {runId}:marker，末条为运行态时 activeLeaf 派生", () => {
    const nodes = new Map([["a-fe", "run-7"]]);
    const { messages, activeLeafId } = toSavePayload(
      [
        msg("u1", "user", "你好"),
        msg("a1", "assistant", "历史答复（canonical 收养节点）"),
        msg("u2", "user", "继续"),
        msg("a-fe", "assistant", "本 app 运行态节点"),
      ],
      nodes,
    );
    // a-fe 是运行态节点，被剔除；u2 排在 a1 之后，parentId 保持 a1 不变；
    // 末条恰是被剔除的运行态节点，activeLeafId 改写为其 canonical 等价 marker
    expect(messages.map((m) => m.id)).toEqual(["u1", "a1", "u2"]);
    expect(messages[2].parentId).toBe("a1");
    expect(activeLeafId).toBe("run-7:marker");
  });

  it("运行态节点后有新 user：user 挂在 marker 等价上", () => {
    const nodes = new Map([["a-fe", "run-7"]]);
    const { messages, activeLeafId } = toSavePayload(
      [msg("u1", "user", "你好"), msg("a-fe", "assistant", "x"), msg("u2", "user", "追问")],
      nodes,
    );
    expect(messages.map((m) => m.id)).toEqual(["u1", "u2"]);
    expect(messages[1].parentId).toBe("run-7:marker");
    expect(activeLeafId).toBe("u2");
  });
});

describe("syncTreeBeforeSend（E_SESSION_REV 自愈）", () => {
  it("成功路径推进 revision", async () => {
    const save = vi.mocked(client.saveSessionTree).mockResolvedValue({ revision: 3 });
    await syncTreeBeforeSend("s1", [msg("u1", "user", "你好")]);
    expect(save).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: "s1", baseRevision: 0 }),
    );
    expect(useSessionPersistStore.getState().scopes.s1?.revision).toBe(3);
  });

  it("revision 过期 → load 校正后重试一次", async () => {
    const save = vi
      .mocked(client.saveSessionTree)
      .mockRejectedValueOnce({ code: "E_SESSION_REV", message: "基线过期" })
      .mockResolvedValue({ revision: 9 });
    const load = vi.mocked(client.loadSessionTree).mockResolvedValue({
      sessionId: "s1",
      messages: [],
      revision: 8,
    });
    await syncTreeBeforeSend("s1", [msg("u1", "user", "你好")]);
    expect(load).toHaveBeenCalledWith("s1");
    expect(save).toHaveBeenCalledTimes(2);
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ baseRevision: 8 }));
    expect(useSessionPersistStore.getState().scopes.s1?.revision).toBe(9);
  });

  it("非 REV 错误上抛（发送流程 catch 后继续）", async () => {
    vi.mocked(client.saveSessionTree).mockRejectedValue({ code: "E_STORAGE_IO", message: "盘满" });
    await expect(syncTreeBeforeSend("s1", [msg("u1", "user", "你好")])).rejects.toMatchObject({
      code: "E_STORAGE_IO",
    });
  });
});

describe("loadHistory", () => {
  it("新会话（E_NOT_FOUND）返回 null", async () => {
    vi.mocked(client.loadSessionTree).mockRejectedValue({
      code: "E_NOT_FOUND",
      message: "会话不存在",
    });
    expect(await loadHistory("s1")).toBeNull();
  });

  it("成功返回 canonical 树并推进 revision", async () => {
    const tree: SessionTreeData = {
      sessionId: "s1",
      messages: [{ id: "u1", role: "user", parts: [], createdAt: 1 }] satisfies MessageEntry[],
      activeLeafId: "u1",
      revision: 5,
    };
    vi.mocked(client.loadSessionTree).mockResolvedValue(tree);
    expect(await loadHistory("s1")).toBe(tree);
    expect(useSessionPersistStore.getState().scopes.s1?.revision).toBe(5);
  });
});
