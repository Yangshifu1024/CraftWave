// @vitest-environment happy-dom
/**
 * Inspector「授权」页测试（撤销 UI 的会话面）：当前会话
 * 已授权外部目录列表渲染 + 移除授权（revokeExternalDir 载荷 + 刷新）。
 * 精简版：列表/撤销/空态合并为代表例。
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

describe("ExternalGrantsPanel", () => {
  it("授权列表渲染、移除授权载荷（sessionId, dir）+ 刷新、无授权空态", async () => {
    // 每行一个目录 + 移除按钮 + 粒度说明
    useSessionsStore.setState({ refresh: refreshMock });
    const view = renderPanel();
    const rows = view.container.querySelectorAll('[data-testid="external-grant-row"]');
    expect(rows).toHaveLength(2);
    expect(view.container.textContent).toContain("/Users/demo/外部资料");
    expect(view.container.textContent).toContain("/data/共享");
    expect(view.container.textContent).toContain("该目录及子目录的读/写/列");

    // 移除授权 → revokeExternalDir 载荷 + 刷新回填权威列表
    fireEvent.click(screen.getByTestId("external-grant-revoke-/Users/demo/外部资料"));
    await waitFor(() =>
      expect(revokeMock).toHaveBeenCalledWith({
        sessionId: "s1",
        dir: "/Users/demo/外部资料",
      }),
    );
    await waitFor(() => expect(refreshMock).toHaveBeenCalled());
    view.unmount();

    // 无授权时渲染空态说明（临时会话或未授权过）
    useSessionsStore.setState({ sessions: [meta("s1", [])] });
    const empty = renderPanel();
    expect(screen.getByTestId("external-grants-empty")).toBeDefined();
    expect(empty.container.querySelectorAll('[data-testid="external-grant-row"]')).toHaveLength(0);
    empty.unmount();
  });
});
