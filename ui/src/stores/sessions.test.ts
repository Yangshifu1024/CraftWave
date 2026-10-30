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
 *  空会话样例用 withCount(0) 构造）。 */
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

describe("bootstrap（启动恢复）", () => {
  it("ui-state 的 Tab 布局按现存会话重建，已删除条目剔除", async () => {
    listSessions.mockResolvedValue([meta("s1"), meta("s2")]);
    getUiState.mockResolvedValue({ tabIds: ["s2", "ghost", "s1"], activeId: "ghost" });
    await useSessionsStore.getState().bootstrap();
    const state = useSessionsStore.getState();
    expect(state.tabs.map((t) => t.id)).toEqual(["s2", "s1"]);
    // activeId 失效时回退首个 Tab
    expect(state.activeId).toBe("s2");
  });

  it("无存档但已有会话 → 恢复最新一条（列表首位）", async () => {
    listSessions.mockResolvedValue([meta("latest"), meta("old")]);
    await useSessionsStore.getState().bootstrap();
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["latest"]);
    expect(useSessionsStore.getState().activeId).toBe("latest");
  });

  it("全新安装 → 自动建一个临时会话并刷新列表", async () => {
    createSession.mockResolvedValue(meta("first"));
    await useSessionsStore.getState().bootstrap();
    expect(createSession).toHaveBeenCalledWith(undefined);
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["first"]);
    expect(listSessions).toHaveBeenCalled();
  });

  it("空会话（未发送）不进左栏列表，tabIds 引用它的 Tab 也不恢复", async () => {
    listSessions.mockResolvedValue([meta("sent"), emptyMeta("draft"), emptyMeta("draft2")]);
    getUiState.mockResolvedValue({
      tabIds: ["draft", "sent"],
      activeId: "draft",
    });
    await useSessionsStore.getState().bootstrap();
    const state = useSessionsStore.getState();
    // 列表只有发送过的会话
    expect(state.sessions.map((m) => m.id)).toEqual(["sent"]);
    // 空会话 Tab 不恢复（无内容可保，恢复只会产生左栏不可达的孤儿 Tab）
    expect(state.tabs.map((t) => t.id)).toEqual(["sent"]);
    expect(state.activeId).toBe("sent");
  });

  it("列表全为空会话时兜底新建，而非恢复空会话", async () => {
    listSessions.mockResolvedValue([emptyMeta("draft")]);
    createSession.mockResolvedValue(emptyMeta("first"));
    await useSessionsStore.getState().bootstrap();
    expect(createSession).toHaveBeenCalledWith(undefined);
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["first"]);
  });
});

describe("空会话显示纪律与空 Tab 复用", () => {
  it("refresh 同样过滤空会话；已知会话快照照常同步", async () => {
    const s = useSessionsStore.getState();
    s.openTab(meta("s1"));
    s.openTab(emptyMeta("draft"));
    listSessions.mockResolvedValue([meta("s1"), emptyMeta("draft")]);
    await useSessionsStore.getState().refresh();
    const state = useSessionsStore.getState();
    expect(state.sessions.map((m) => m.id)).toEqual(["s1"]);
    // 空 Tab 保留在 Tab 面（用户正在用），只是列表不显示
    expect(state.tabs.map((t) => t.id)).toEqual(["s1", "draft"]);
  });

  it("findReusableEmptyTab：同域 + 从未落盘才复用；运行中（已 save）与异域不复用", () => {
    const tabs = [emptyMeta("draft", undefined), emptyMeta("proj", "p1")];
    // 空白临时 Tab：复用
    expect(findReusableEmptyTab(tabs, {}, undefined)?.id).toBe("draft");
    // 运行中（已 save，revision>0）：不复用（首条消息已落盘、索引未刷回的窗口）
    expect(findReusableEmptyTab(tabs, { draft: { revision: 4 } }, undefined)).toBeUndefined();
    // 项目域不匹配：临时空 Tab 不给「项目下新建」复用
    expect(findReusableEmptyTab(tabs, {}, "p1")?.id).toBe("proj");
    // 发送过的会话不在候选（messageCount>0）
    expect(findReusableEmptyTab([meta("sent")], {}, undefined)).toBeUndefined();
  });
});

describe("Tab 操作", () => {
  it("openTab 打开并激活；重复打开仅激活", async () => {
    const s = useSessionsStore.getState();
    s.openTab(meta("a"));
    s.openTab(meta("b"));
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["a", "b"]);
    expect(useSessionsStore.getState().activeId).toBe("b");
    s.openTab(meta("a"));
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["a", "b"]);
    expect(useSessionsStore.getState().activeId).toBe("a");
  });

  it("closeTab 关闭活跃 Tab 时右移左补；关闭他 Tab 不动活跃", () => {
    const s = useSessionsStore.getState();
    s.openTab(meta("a"));
    s.openTab(meta("b"));
    s.openTab(meta("c"));
    s.setActive("b");
    s.closeTab("b");
    expect(useSessionsStore.getState().tabs.map((t) => t.id)).toEqual(["a", "c"]);
    // 右邻补位
    expect(useSessionsStore.getState().activeId).toBe("c");
    s.closeTab("c");
    // 末尾关闭回退
    expect(useSessionsStore.getState().activeId).toBe("a");
    s.closeTab("a");
    expect(useSessionsStore.getState().activeId).toBeNull();
    expect(useSessionsStore.getState().tabs).toEqual([]);
  });

  it("refresh 同步 Tab 快照（改名后标题一致）", async () => {
    const s = useSessionsStore.getState();
    s.openTab(meta("s1", undefined, "旧名"));
    listSessions.mockResolvedValue([meta("s1", undefined, "新名")]);
    await useSessionsStore.getState().refresh();
    expect(useSessionsStore.getState().tabs[0]?.title).toBe("新名");
  });
});
