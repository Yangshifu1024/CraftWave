// @vitest-environment happy-dom
/**
 * Inspector「授权」页测试（24 号票，§9.9 撤销 UI 的会话面）：当前会话
 * 已授权外部目录列表渲染 + 移除授权（revokeExternalDir 载荷 + 刷新）。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SessionScopeContext } from "@/chat/session-scope";
import type { SessionMeta } from "@/ipc/types";
import { useSessionsStore } from "@/stores/sessions";
import { ExternalGrantsPanel } from "./ExternalGrants";

/** revokeExternalDir / refresh 的 mock（真实 IPC 被 vi.mock 替换）。 */
const revokeMock =
  vi.fn<(args: { sessionId: string; dir: string }) => Promise<{ roots: string[] }>>();
const refreshMock = vi.fn<() => Promise<void>>();
vi.mock("@/ipc/client", () => ({
  revokeExternalDir: (args: { sessionId: string; dir: string }) => revokeMock(args),
}));

/** 会话 meta fixture（授权目录可注入）。 */
function meta(id: string, dirs: string[]): SessionMeta {
  return {
    id,
    projectId: undefined,
    title: `会话 ${id}`,
    createdAt: 1,
    updatedAt: 1,
    revision: 1,
    messageCount: 0,
    totalTokens: 0,
    authorizedExternalDirs: dirs,
  };
}

beforeEach(() => {
  revokeMock.mockReset();
  revokeMock.mockResolvedValue({ roots: [] });
  refreshMock.mockReset();
  refreshMock.mockResolvedValue(undefined);
  useSessionsStore.setState({
    sessions: [meta("s1", ["/Users/demo/外部资料", "/data/共享"])],
  });
});
afterEach(cleanup);

function renderPanel() {
  return render(
    <SessionScopeContext.Provider value="s1">
      <ExternalGrantsPanel />
    </SessionScopeContext.Provider>,
  );
}

describe("ExternalGrantsPanel（24 号票，§9.9）", () => {
  it("渲染当前会话已授权目录列表（每行一个目录 + 移除按钮）", () => {
    const { container } = renderPanel();
    const rows = container.querySelectorAll('[data-testid="external-grant-row"]');
    expect(rows).toHaveLength(2);
    expect(container.textContent).toContain("/Users/demo/外部资料");
    expect(container.textContent).toContain("/data/共享");
    expect(container.textContent).toContain("该目录及子目录的读/写/列");
  });

  it("点移除 → revokeExternalDir 载荷（sessionId, dir）+ 刷新回填权威列表", async () => {
    useSessionsStore.setState({ refresh: refreshMock });
    renderPanel();
    fireEvent.click(screen.getByTestId("external-grant-revoke-/Users/demo/外部资料"));
    await waitFor(() =>
      expect(revokeMock).toHaveBeenCalledWith({
        sessionId: "s1",
        dir: "/Users/demo/外部资料",
      }),
    );
    await waitFor(() => expect(refreshMock).toHaveBeenCalled());
  });

  it("无授权时渲染空态说明（临时会话或未授权过）", () => {
    useSessionsStore.setState({ sessions: [meta("s1", [])] });
    const { container } = renderPanel();
    expect(screen.getByTestId("external-grants-empty")).toBeDefined();
    expect(container.querySelectorAll('[data-testid="external-grant-row"]')).toHaveLength(0);
  });
});
