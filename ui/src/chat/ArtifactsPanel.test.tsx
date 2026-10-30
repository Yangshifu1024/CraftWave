// @vitest-environment happy-dom
/**
 * 产物面板测试（28 号票，FR-10-06 / §10.7）：
 * - 清单渲染（文件名 + 相对路径 + ×runCount 徽标）、exists=false 灰显置底；
 * - 空态（无登记文件）与加载失败兜底；
 * - 预览 Dialog（truncated 提示、读取失败行内提示）；
 * - open_in_editor 失败（E_NOT_FOUND 无编辑器）行内提示；
 * - 变更面板联动接缝（openChangesPanel，26 号票约定接口）。
 *
 * IPC 经 vi.mock 替换（ChangesPanel.test 同款桩面）。
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
  it("空态：无登记文件给引导文案", async () => {
    await renderPanel();
    expect(screen.getByTestId("artifacts-empty").textContent).toContain("还没有产物");
  });

  it("清单渲染：文件名 + 相对路径 + 计数徽标", async () => {
    mocks.listArtifactsMock.mockResolvedValue({
      artifacts: [row("proj/src/main.rs", 3)],
    });
    await renderPanel();
    expect(screen.getByTestId("artifacts-list").textContent).toContain("main.rs");
    expect(screen.getByTestId("artifact-run-count").textContent).toBe("×3");
    expect(screen.getByTitle("proj/src/main.rs")).toBeTruthy();
  });

  it("exists=false 灰显置底：删除标记 + 不渲染行操作", async () => {
    // 后端排序契约：存在在前、已删除置底——前端按返回序渲染
    mocks.listArtifactsMock.mockResolvedValue({
      artifacts: [row("proj/live.txt", 2, true), row("proj/gone.txt", 1, false)],
    });
    await renderPanel();
    const items = screen.getAllByTestId("artifact-missing");
    expect(items.length).toBe(1);
    expect(items[0].textContent).toContain("已删除");
    // 已删除行不提供操作钮；存在行提供
    expect(screen.queryByTestId("artifact-preview-live.txt")).toBeTruthy();
    expect(screen.queryByTestId("artifact-preview-gone.txt")).toBeNull();
    // 置底：已删除行在 DOM 序的最后（测试容器行序）
    const rowsInOrder = screen
      .getAllByTestId(/^artifact-row-/)
      .map((el) => el.getAttribute("data-testid"));
    expect(rowsInOrder).toEqual(["artifact-row-proj/live.txt", "artifact-row-proj/gone.txt"]);
  });

  it("预览：read_workspace_file 取数进 Dialog，truncated 有提示行", async () => {
    mocks.listArtifactsMock.mockResolvedValue({
      artifacts: [row("proj/big.log")],
    });
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
  });

  it("预览失败（二进制/越界）：错误行内展示，不弹窗", async () => {
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

  it("打开失败（探测不到编辑器 E_NOT_FOUND）：行内提示", async () => {
    mocks.listArtifactsMock.mockResolvedValue({ artifacts: [row("proj/a.txt")] });
    mocks.openInEditorMock.mockRejectedValue({
      code: "E_NOT_FOUND",
      message: "未探测到可用编辑器",
    });
    await renderPanel();
    await act(async () => {
      fireEvent.click(screen.getByTestId("artifact-open-a.txt"));
    });
    expect(screen.getByRole("alert").textContent).toContain("未探测到可用编辑器");
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

  it("run 终态事件刷新：run:done 到达后重新拉取（他会话事件不刷新）", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    // listenEvent 桩：捕获注册的 handler（键 → 回调）
    mocks.listenEventMock.mockImplementation((key, handler) => {
      handlers.set(key, handler as unknown as (payload: unknown) => void);
      return Promise.resolve(() => {});
    });
    mocks.listArtifactsMock.mockResolvedValue({ artifacts: [row("proj/a.txt")] });
    await renderPanel();
    expect(mocks.listArtifactsMock).toHaveBeenCalledTimes(1);
    // 他会话的 run 终态：不触发重拉
    await act(async () => {
      handlers.get("run:done")?.({ sessionId: "s-other" });
    });
    expect(mocks.listArtifactsMock).toHaveBeenCalledTimes(1);
    // 本会话 run 正常结束 → 面板重拉
    await act(async () => {
      handlers.get("run:done")?.({ sessionId: "s1" });
    });
    expect(mocks.listArtifactsMock).toHaveBeenCalledTimes(2);
  });
});
