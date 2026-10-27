// @vitest-environment happy-dom
/**
 * sessions store 测试（16 号票，FR-01-01 多 Tab 状态面）：
 * bootstrap（ui-state 恢复/兜底）、openTab/closeTab/setActive、ui-state 落盘。
 * ipc/client 注入 fake。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProjectMeta, SessionMeta } from "@/ipc/types";
import { useSessionsStore } from "./sessions";

const meta = (id: string, projectId?: string, title = `会话${id}`): SessionMeta => ({
  id,
  projectId,
  title,
  createdAt: 0,
  updatedAt: 0,
  revision: 0,
  messageCount: 0,
  totalTokens: 0,
});

const listProjects = vi.fn<() => Promise<ProjectMeta[]>>();
const listSessions = vi.fn<() => Promise<SessionMeta[]>>();
const createSession = vi.fn<(projectId?: string) => Promise<SessionMeta>>();
const setUiState = vi.fn<(state: unknown) => Promise<void>>();
const getUiState = vi.fn<() => Promise<unknown>>();

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
