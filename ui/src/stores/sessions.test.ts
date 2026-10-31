// @vitest-environment happy-dom
/**
 * sessions store 测试（16 号票，FR-01-01 多 Tab 状态面）：
 * bootstrap（ui-state 恢复/兜底）、openTab/closeTab/setActive、ui-state 落盘。
 * ipc/client 经 vi.mock 注入 fake（替代真实后端调用，测试不依赖 Rust 侧）。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProjectMeta, SessionMeta } from "@/ipc/types";
import { findReusableEmptyTab, useSessionsStore } from "./sessions";

/** 造一条 SessionMeta 测试样例（默认非空会话——messageCount>0 才进左栏列表；
 *  空会话样例用 emptyMeta 构造）。 */
const meta = (id: string, projectId?: string, title = `会话${id}`): SessionMeta => ({
  id,
  projectId,
  title,
  createdAt: 0,
  updatedAt: 0,
  revision: 0,
  messageCount: 1,
  totalTokens: 0,
  authorizedExternalDirs: [],
});

/** 空会话样例（新建未发送形态：messageCount=0，左栏列表不显示）。 */
const emptyMeta = (id: string, projectId?: string): SessionMeta => ({
  ...meta(id, projectId),
  messageCount: 0,
});

// 各 IPC 命令的测试桩（vi.fn 可按用例指定返回值或抛错）
const listProjects = vi.fn<() => Promise<ProjectMeta[]>>();
const listSessions = vi.fn<() => Promise<SessionMeta[]>>();
const createSession = vi.fn<(projectId?: string) => Promise<SessionMeta>>();
const setUiState = vi.fn<(state: unknown) => Promise<void>>();
const getUiState = vi.fn<() => Promise<unknown>>();

// 只替换本测试用到的命令，其余保留 client.ts 真实现
vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    listProjects: () => listProjects(),
    listSessions: () => listSessions(),
    createSession: (projectId?: string) => createSession(projectId),
    getUiState: () => getUiState(),
    setUiState: (state: unknown) => setUiState(state),
  };
});

// 每个用例前：重置 mock、给安全的默认返回值、恢复 store 初始状态
beforeEach(() => {
  vi.resetAllMocks();
  listProjects.mockResolvedValue([]);
  listSessions.mockResolvedValue([]);
  createSession.mockImplementation(async (projectId) =>
    meta(`new-${projectId ?? "temp"}`, projectId),
  );
  getUiState.mockResolvedValue(null);
  setUiState.mockResolvedValue(undefined);
  useSessionsStore.setState(useSessionsStore.getInitialState());
});

describe("sessions store", () => {
  it("bootstrap 剧本：ui-state 恢复剔 ghost、空会话双过滤、无存档取最新、无可恢复自动建", async () => {
    // 1) ui-state 的 Tab 布局按现存会话重建，已删除条目剔除；activeId 失效回退首个 Tab
    listSessions.mockResolvedValue([meta("s1"), meta("s2")]);
    getUiState.mockResolvedValue({ tabIds: ["s2", "ghost", "s1"], activeId: "ghost" });
    await useSessionsStore.getState().bootstrap();
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["s2", "s1"]);
    expect(useSessionsStore.getState().activeId).toBe("s2");

    // 2) 空会话（未发送）不进左栏列表，tabIds 引用它的 Tab 也不恢复
    //（无内容可保，恢复只会产生左栏不可达的孤儿 Tab）
    useSessionsStore.setState(useSessionsStore.getInitialState());
    listSessions.mockResolvedValue([meta("sent"), emptyMeta("draft"), emptyMeta("draft2")]);
    getUiState.mockResolvedValue({ tabIds: ["draft", "sent"], activeId: "draft" });
    await useSessionsStore.getState().bootstrap();
    const state = useSessionsStore.getState();
    expect(state.sessions.map((m) => m.id)).toEqual(["sent"]);
    expect(state.tabs.map((t) => t.id)).toEqual(["sent"]);
    expect(state.activeId).toBe("sent");

    // 3) 无存档但已有会话 → 恢复最新一条（列表首位）
    useSessionsStore.setState(useSessionsStore.getInitialState());
    listSessions.mockResolvedValue([meta("latest"), meta("old")]);
    await useSessionsStore.getState().bootstrap();
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["latest"]);
    expect(useSessionsStore.getState().activeId).toBe("latest");

    // 4) 无可恢复（无存档 + 无非空会话，空会话不算数）→ 兜底新建而非恢复空会话
    useSessionsStore.setState(useSessionsStore.getInitialState());
    listSessions.mockResolvedValue([emptyMeta("draft")]);
    createSession.mockResolvedValue(emptyMeta("first"));
    await useSessionsStore.getState().bootstrap();
    expect(createSession).toHaveBeenLastCalledWith(undefined);
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["first"]);

    // 5) 全新安装（列表为空）→ 自动建一个临时会话并刷新列表
    useSessionsStore.setState(useSessionsStore.getInitialState());
    listSessions.mockResolvedValue([]);
    createSession.mockResolvedValue(meta("first"));
    await useSessionsStore.getState().bootstrap();
    expect(listSessions).toHaveBeenCalled();
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["first"]);
  });

  it("空会话显示纪律与空 Tab 复用：refresh 过滤列表但保 Tab；复用仅限同域且从未落盘", async () => {
    // refresh 同样过滤空会话；已知会话快照照常同步；空 Tab 保留在 Tab 面（用户正在用）
    const s = useSessionsStore.getState();
    s.openTab(meta("s1"));
    s.openTab(emptyMeta("draft"));
    listSessions.mockResolvedValue([meta("s1"), emptyMeta("draft")]);
    await useSessionsStore.getState().refresh();
    let state = useSessionsStore.getState();
    expect(state.sessions.map((m) => m.id)).toEqual(["s1"]);
    expect(state.tabs.map((t) => t.id)).toEqual(["s1", "draft"]);

    // findReusableEmptyTab：同域 + 从未落盘才复用；运行中（已 save）与异域不复用
    const tabs = [emptyMeta("draft", undefined), emptyMeta("proj", "p1")];
    expect(findReusableEmptyTab(tabs, {}, undefined)?.id).toBe("draft");
    // 运行中（已 save，revision>0）：不复用（首条消息已落盘、索引未刷回的窗口）
    expect(findReusableEmptyTab(tabs, { draft: { revision: 4 } }, undefined)).toBeUndefined();
    // 项目域不匹配：临时空 Tab 不给「项目下新建」复用
    expect(findReusableEmptyTab(tabs, {}, "p1")?.id).toBe("proj");
    // 发送过的会话不在候选（messageCount>0）
    expect(findReusableEmptyTab([meta("sent")], {}, undefined)).toBeUndefined();
    state = useSessionsStore.getState();
    expect(state.tabs).toHaveLength(2);
  });

  it("Tab 操作剧本：打开/激活/重复打开仅激活、关闭右移左补与回退、refresh 同步快照、applyTitle 就地改标题", async () => {
    const s = useSessionsStore.getState();
    // openTab 打开并激活；重复打开仅激活
    s.openTab(meta("a"));
    s.openTab(meta("b"));
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["a", "b"]);
    expect(useSessionsStore.getState().activeId).toBe("b");
    s.openTab(meta("a"));
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["a", "b"]);
    expect(useSessionsStore.getState().activeId).toBe("a");

    // closeTab 关闭活跃 Tab 时右邻补位；末尾关闭回退；关空后归 null
    s.openTab(meta("c"));
    s.setActive("b");
    s.closeTab("b");
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["a", "c"]);
    expect(useSessionsStore.getState().activeId).toBe("c");
    s.closeTab("c");
    expect(useSessionsStore.getState().activeId).toBe("a");
    s.closeTab("a");
    expect(useSessionsStore.getState().activeId).toBeNull();
    expect(useSessionsStore.getState().tabs).toEqual([]);

    // refresh 同步 Tab 快照（改名后标题一致）
    s.openTab(meta("s1", undefined, "旧名"));
    listSessions.mockResolvedValue([meta("s1", undefined, "新名")]);
    await useSessionsStore.getState().refresh();
    expect(useSessionsStore.getState().tabs[0]?.title).toBe("新名");

    // applyTitle（27 号票 session:title 事件面）：在 Tab 中的会话 tabs 与左栏
    // 缓存列表同改（标题就地更新，其余字段与排序不动）
    const opened = meta("s1", undefined, "新会话");
    useSessionsStore.setState({
      tabs: [opened, meta("s2", undefined, "另一会话")],
      sessions: [opened, meta("s2", undefined, "另一会话")],
    });
    useSessionsStore.getState().applyTitle("s1", "自动起的标题");
    let state = useSessionsStore.getState();
    expect(state.tabs.map((t) => t.title)).toEqual(["自动起的标题", "另一会话"]);
    expect(state.tabs[0]?.messageCount).toBe(opened.messageCount);
    expect(state.sessions.map((m) => m.title)).toEqual(["自动起的标题", "另一会话"]);

    // applyTitle：会话不在 tabs（后台项目会话）→ 只更新缓存列表，不强行开 Tab
    useSessionsStore.setState({
      tabs: [meta("s2", "p1", "项目会话")],
      sessions: [meta("s1", "p1", "新会话"), meta("s2", "p1", "项目会话")],
      activeId: "s2",
    });
    useSessionsStore.getState().applyTitle("s1", "后台起名");
    state = useSessionsStore.getState();
    expect(state.tabs.map((t) => t.id)).toEqual(["s2"]);
    expect(state.activeId).toBe("s2");
    expect(state.sessions.find((m) => m.id === "s1")?.title).toBe("后台起名");
  });

  it("scrollAnchors（29 号票滚动锚点桶）：bootstrap 恢复带形状清洗；set/forget 随防抖落盘", async () => {
    // bootstrap 从 ui-state 恢复锚点；坏数据条目剔除（形状清洗）
    listSessions.mockResolvedValue([meta("s1")]);
    getUiState.mockResolvedValue({
      tabIds: ["s1"],
      activeId: "s1",
      scrollAnchors: {
        s1: { leafId: "m9", offset: 120 },
        broken: { leafId: 42, offset: "x" }, // 类型不符 → 剔除
        worse: "not-an-object", // 非对象 → 剔除
      },
    });
    await useSessionsStore.getState().bootstrap();
    expect(useSessionsStore.getState().scrollAnchors.s1).toEqual({ leafId: "m9", offset: 120 });
    expect(useSessionsStore.getState().scrollAnchors.broken).toBeUndefined();
    expect(useSessionsStore.getState().scrollAnchors.worse).toBeUndefined();

    // setScrollAnchor 写桶并随防抖落盘带出 scrollAnchors 字段；forget 清除并落盘空桶
    vi.useFakeTimers();
    try {
      useSessionsStore.setState(useSessionsStore.getInitialState());
      useSessionsStore.getState().setScrollAnchor("s1", { leafId: "m2", offset: 64 });
      expect(useSessionsStore.getState().scrollAnchors.s1).toEqual({ leafId: "m2", offset: 64 });
      await vi.advanceTimersByTimeAsync(350);
      let payload = setUiState.mock.calls.at(-1)?.[0] as { scrollAnchors?: unknown };
      expect(payload?.scrollAnchors).toEqual({ s1: { leafId: "m2", offset: 64 } });
      useSessionsStore.getState().forgetScrollAnchor("s1");
      expect(useSessionsStore.getState().scrollAnchors.s1).toBeUndefined();
      await vi.advanceTimersByTimeAsync(350);
      payload = setUiState.mock.calls.at(-1)?.[0] as { scrollAnchors?: unknown };
      expect(payload?.scrollAnchors).toEqual({});
    } finally {
      vi.useRealTimers();
    }
  });
});
