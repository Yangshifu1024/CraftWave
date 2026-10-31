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
import { useSessionsStore } from "@/stores/sessions";

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
  listSessions: vi.fn(async () => []),
  // syncTreeBeforeSend 首次可见刷新经 sessions store 的 refresh 拉取
  listProjects: vi.fn(async () => []),
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

describe("session 持久化（15 号票，§6.3 双写协议）", () => {
  it("toSavePayload 剧本：线性链 parentId、运行态节点剔除与 {runId}:marker 改写、新 user 挂 marker 等价", () => {
    // 无运行态节点：线性链 parentId + activeLeaf=末条
    const nodes = new Map<string, string>();
    let saved = toSavePayload([msg("u1", "user", "你好"), msg("a1", "assistant", "答复")], nodes);
    expect(saved.messages.map((m) => m.id)).toEqual(["u1", "a1"]);
    expect(saved.messages[0].parentId).toBeUndefined();
    expect(saved.messages[1].parentId).toBe("u1");
    expect(saved.activeLeafId).toBe("a1");

    // 运行态节点剔除：其子引用保持 canonical 父（u2 排在 a1 后 parentId=a1 不变）；
    // 末条恰是被剔除的运行态节点时 activeLeaf 改写为 {runId}:marker
    saved = toSavePayload(
      [
        msg("u1", "user", "你好"),
        msg("a1", "assistant", "历史答复（canonical 收养节点）"),
        msg("u2", "user", "继续"),
        msg("a-fe", "assistant", "本 app 运行态节点"),
      ],
      new Map([["a-fe", "run-7"]]),
    );
    expect(saved.messages.map((m) => m.id)).toEqual(["u1", "a1", "u2"]);
    expect(saved.messages[2].parentId).toBe("a1");
    expect(saved.activeLeafId).toBe("run-7:marker");

    // 运行态节点后有新 user：user 挂在 marker 等价上
    saved = toSavePayload(
      [msg("u1", "user", "你好"), msg("a-fe", "assistant", "x"), msg("u2", "user", "追问")],
      new Map([["a-fe", "run-7"]]),
    );
    expect(saved.messages.map((m) => m.id)).toEqual(["u1", "u2"]);
    expect(saved.messages[1].parentId).toBe("run-7:marker");
    expect(saved.activeLeafId).toBe("u2");
  });

  it("syncTreeBeforeSend 剧本（E_SESSION_REV 自愈）：成功推进 revision、过期 load 校正重试一次、非 REV 上抛、首次可见补 refresh", async () => {
    // 成功路径推进 revision（baseRevision 首次为 0）
    vi.mocked(client.saveSessionTree).mockResolvedValue({ revision: 3 });
    await syncTreeBeforeSend("s1", [msg("u1", "user", "你好")]);
    expect(client.saveSessionTree).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: "s1", baseRevision: 0 }),
    );
    expect(useSessionPersistStore.getState().scopes.s1?.revision).toBe(3);

    // revision 过期 → load 校正后重试一次（baseRevision 用校正值）
    const savesBefore = vi.mocked(client.saveSessionTree).mock.calls.length;
    vi.mocked(client.saveSessionTree)
      .mockRejectedValueOnce({ code: "E_SESSION_REV", message: "基线过期" })
      .mockResolvedValue({ revision: 9 });
    vi.mocked(client.loadSessionTree).mockResolvedValue({
      sessionId: "s1",
      messages: [],
      compactions: [],
      revision: 8,
    });
    await syncTreeBeforeSend("s1", [msg("u1", "user", "你好")]);
    expect(client.loadSessionTree).toHaveBeenCalledWith("s1");
    expect(client.saveSessionTree).toHaveBeenCalledTimes(savesBefore + 2);
    expect(client.saveSessionTree).toHaveBeenLastCalledWith(
      expect.objectContaining({ baseRevision: 8 }),
    );
    expect(useSessionPersistStore.getState().scopes.s1?.revision).toBe(9);

    // 非 REV 错误上抛（发送流程 catch 后继续）
    vi.mocked(client.saveSessionTree).mockRejectedValue({ code: "E_STORAGE_IO", message: "盘满" });
    await expect(syncTreeBeforeSend("s1", [msg("u1", "user", "你好")])).rejects.toMatchObject({
      code: "E_STORAGE_IO",
    });

    // 首次可见刷新：会话已在左栏列表（非空）→ save 成功后不触发 refresh
    vi.mocked(client.listSessions).mockClear();
    useSessionsStore.setState({
      sessions: [
        {
          id: "s1",
          projectId: undefined,
          title: "t",
          createdAt: 0,
          updatedAt: 0,
          revision: 1,
          messageCount: 2,
          totalTokens: 0,
          authorizedExternalDirs: [],
        },
      ],
    });
    vi.mocked(client.saveSessionTree).mockResolvedValue({ revision: 3 });
    await syncTreeBeforeSend("s1", [msg("u1", "user", "你好")]);
    expect(client.listSessions).not.toHaveBeenCalled();

    // 空会话（列表无它）→ save 成功后 refresh 让左栏即时出现
    useSessionsStore.setState(useSessionsStore.getInitialState());
    await syncTreeBeforeSend("s1", [msg("u1", "user", "你好")]);
    expect(client.listSessions).toHaveBeenCalledTimes(1);
  });

  it("loadHistory：新会话（E_NOT_FOUND）返回 null；成功返回 canonical 树并推进 revision", async () => {
    vi.mocked(client.loadSessionTree).mockRejectedValue({
      code: "E_NOT_FOUND",
      message: "会话不存在",
    });
    expect(await loadHistory("s1")).toBeNull();

    const tree: SessionTreeData = {
      sessionId: "s1",
      messages: [{ id: "u1", role: "user", parts: [], createdAt: 1 }] satisfies MessageEntry[],
      compactions: [],
      activeLeafId: "u1",
      revision: 5,
    };
    vi.mocked(client.loadSessionTree).mockResolvedValue(tree);
    expect(await loadHistory("s1")).toBe(tree);
    expect(useSessionPersistStore.getState().scopes.s1?.revision).toBe(5);
  });
});
