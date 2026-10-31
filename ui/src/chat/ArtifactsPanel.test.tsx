// @vitest-environment happy-dom
/**
 * 产物面板测试（28 号票，FR-10-06 / §10.7）：
 * - 清单渲染（文件名 + 相对路径 + ×runCount 徽标）、exists=false 灰显置底；
 * - 预览 Dialog（truncated 提示、读取失败行内提示）；
 * - 变更面板联动接缝（openChangesPanel，26 号票约定接口）。
 *
 * IPC 经 vi.mock 替换（ChangesPanel.test 同款桩面）。
 * 精简版：清单与置底合并；预览成败合并。
 */

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ArtifactRow } from "@/ipc/types";
import { ArtifactsPanel } from "./ArtifactsPanel";
import { openChangesPanel, useChangesStore } from "./changes-store";

// ---- IPC 测试桩（vi.hoisted：vi.mock 工厂随 import 提前执行，桩须同升）----

const mocks = vi.hoisted(() => ({
  listArtifactsMock: vi.fn<(sessionId: string) => Promise<unknown>>(),
  openInEditorMock: vi.fn<(path: string, line?: number) => Promise<void>>(),
  readWorkspaceFileMock:
    vi.fn<(sessionId: string, path: string, range?: unknown) => Promise<unknown>>(),
  listenEventMock: vi.fn<(key: string, handler: (p: never) => void) => Promise<() => void>>(),
}));

vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    listArtifacts: (sessionId: string) => mocks.listArtifactsMock(sessionId),
    openInEditor: (path: string, line?: number) => mocks.openInEditorMock(path, line),
    readWorkspaceFile: (sessionId: string, path: string, range?: unknown) =>
      mocks.readWorkspaceFileMock(sessionId, path, range),
    // 事件订阅桩：不触真实 Tauri listen（happy-dom 无 __TAURI_INTERNALS__）
    listenEvent: (key: string, handler: (p: never) => void) => mocks.listenEventMock(key, handler),
  };
});

beforeEach(() => {
  mocks.listenEventMock.mockReset().mockResolvedValue(() => {});
  mocks.listArtifactsMock.mockReset().mockResolvedValue({ artifacts: [] });
  mocks.openInEditorMock.mockReset().mockResolvedValue(undefined);
  mocks.readWorkspaceFileMock.mockReset();
  useChangesStore.setState({ open: false, sessionId: "", focusPath: null });
});
afterEach(() => {
  cleanup();
});

/** 造产物行。 */
function row(path: string, runCount = 1, exists = true): ArtifactRow {
  return { path, runCount, lastSeenAt: 0, exists };
}

/** 渲染并冲刷取数 Promise。 */
async function renderPanel(sessionId = "s1") {
  render(<ArtifactsPanel sessionId={sessionId} />);
  await act(async () => {});
}

describe("ArtifactsPanel（产物页）", () => {
  it("清单渲染：文件名/相对路径/计数徽标；exists=false 灰显置底且无行操作", async () => {
    // 后端排序契约：存在在前、已删除置底——前端按返回序渲染
    mocks.listArtifactsMock.mockResolvedValue({
      artifacts: [row("proj/live.txt", 2, true), row("proj/gone.txt", 1, false)],
    });
    await renderPanel();
    expect(screen.getByTestId("artifacts-list").textContent).toContain("live.txt");
    // 两行各带计数徽标；第一行 = live.txt 的 ×2
    expect(screen.getAllByTestId("artifact-run-count")[0].textContent).toBe("×2");
    expect(screen.getByTitle("proj/live.txt")).toBeTruthy();
    // 已删除行：删除标记、不提供行操作；置底在 DOM 序最后
    const items = screen.getAllByTestId("artifact-missing");
    expect(items.length).toBe(1);
    expect(items[0].textContent).toContain("已删除");
    expect(screen.queryByTestId("artifact-preview-live.txt")).toBeTruthy();
    expect(screen.queryByTestId("artifact-preview-gone.txt")).toBeNull();
    const rowsInOrder = screen
      .getAllByTestId(/^artifact-row-/)
      .map((el) => el.getAttribute("data-testid"));
    expect(rowsInOrder).toEqual(["artifact-row-proj/live.txt", "artifact-row-proj/gone.txt"]);
  });

  it("预览：truncated 有提示行；读取失败行内展示不弹窗", async () => {
    mocks.listArtifactsMock.mockResolvedValue({ artifacts: [row("proj/big.log")] });
    mocks.readWorkspaceFileMock.mockResolvedValue({
      content: "第一行\n第二行",
      truncated: true,
    });
    await renderPanel();
    await act(async () => {
      fireEvent.click(screen.getByTestId("artifact-preview-big.log"));
    });
    expect(screen.getByTestId("artifact-preview-truncated").textContent).toContain("512KB");
    expect(screen.getByTestId("artifact-preview-content").textContent).toContain("第二行");
    cleanup();

    // 预览失败（二进制/越界）：错误行内展示
    mocks.listArtifactsMock.mockResolvedValue({ artifacts: [row("proj/bin.png")] });
    mocks.readWorkspaceFileMock.mockRejectedValue({
      code: "E_INVALID_ARG",
      message: "二进制文件不支持文本预览",
    });
    await renderPanel();
    await act(async () => {
      fireEvent.click(screen.getByTestId("artifact-preview-bin.png"));
    });
    expect(screen.queryByTestId("artifact-preview-content")).toBeNull();
    expect(screen.getByRole("alert").textContent).toContain("二进制文件不支持文本预览");
  });

  it("变更联动接缝：点击「变更」经 openChangesPanel 打开面板并定位路径", async () => {
    mocks.listArtifactsMock.mockResolvedValue({ artifacts: [row("proj/changed.ts")] });
    await renderPanel();
    await act(async () => {
      fireEvent.click(screen.getByTestId("artifact-locate-changed.ts"));
    });
    const state = useChangesStore.getState();
    expect(state.open).toBe(true);
    expect(state.sessionId).toBe("s1");
    expect(state.focusPath).toBe("proj/changed.ts");
    // 收口（不污染其他用例）
    openChangesPanel("s1");
    useChangesStore.getState().closePanel();
  });
});
