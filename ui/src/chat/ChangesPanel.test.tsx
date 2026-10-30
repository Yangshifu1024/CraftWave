// @vitest-environment happy-dom
/**
 * 变更面板测试（26 号票，FR-11-02 / §10.7）：
 * - 文件列表渲染（状态徽标 + 增删统计）、点选切换 diff、定位 focusPath；
 * - 非 git 仓库空态（isRepo=false →「当前项目不是 git 仓库」，不报错）；
 * - 干净工作区空态与加载失败态；
 * - recent log 折叠条与手动刷新按钮；
 * - openChangesPanel 联动接口（28 号票产物页约定）驱动的开合与定位。
 *
 * IPC 经 vi.mock 替换（QuotaPanel.test 同款桩面）。
 */

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DiffPatch, GitStatusFile } from "@/ipc/types";
import { ChangesPanel, mergeChanges } from "./ChangesPanel";
import { openChangesPanel, useChangesStore } from "./changes-store";

// ---- IPC 测试桩（vi.hoisted：vi.mock 工厂随 import 提前执行，桩须同升）----

const mocks = vi.hoisted(() => ({
  gitStatusMock: vi.fn<(args: unknown) => Promise<unknown>>(),
  gitDiffMock: vi.fn<(args: unknown) => Promise<unknown>>(),
  gitRecentLogMock: vi.fn<(args: unknown) => Promise<unknown>>(),
}));

vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    gitStatus: (args: unknown) => mocks.gitStatusMock(args),
    gitDiff: (args: unknown) => mocks.gitDiffMock(args),
    gitRecentLog: (args: unknown) => mocks.gitRecentLogMock(args),
  };
});

beforeEach(() => {
  mocks.gitStatusMock.mockReset().mockResolvedValue({ isRepo: true, files: [] });
  mocks.gitDiffMock.mockReset().mockResolvedValue({ isRepo: true, patches: [] });
  mocks.gitRecentLogMock.mockReset().mockResolvedValue({ isRepo: true, commits: [] });
  useChangesStore.setState({ open: false, sessionId: "", focusPath: null });
});
afterEach(() => {
  cleanup();
});

/** 造状态条目。 */
function file(path: string, status: string): GitStatusFile {
  return { path, status };
}

/** 造 diff 补丁。 */
function patch(path: string, addLines: number, delLines: number): DiffPatch {
  const hunks = [];
  if (addLines > 0 || delLines > 0) {
    hunks.push({
      header: "@@ -1,1 +1,1 @@",
      lines: [
        ...Array.from({ length: delLines }, () => ({ kind: "del" as const, text: "old" })),
        ...Array.from({ length: addLines }, () => ({ kind: "add" as const, text: "new" })),
      ],
    });
  }
  return { path, addLines, delLines, binary: false, truncated: false, hunks };
}

/** 以面板打开态渲染（session s1）；act 冲刷拉取 Promise 后再断言。 */
async function renderOpen(sessionId = "s1") {
  await act(async () => {
    useChangesStore.getState().openPanel(sessionId);
  });
  render(<ChangesPanel />);
  await act(async () => {});
}

describe("mergeChanges 纯函数", () => {
  it("按路径合并状态与增删统计；缺补丁时统计为 null", () => {
    const rows = mergeChanges(
      [file("a.ts", "M"), file("b.ts", "untracked")],
      [patch("a.ts", 3, 1)],
    );
    expect(rows).toEqual([
      { path: "a.ts", status: "M", addLines: 3, delLines: 1 },
      { path: "b.ts", status: "untracked", addLines: null, delLines: null },
    ]);
  });
});

describe("变更面板渲染", () => {
  it("文件列表渲染状态徽标与增删统计，点选切换右列 diff", async () => {
    mocks.gitStatusMock.mockResolvedValue({
      isRepo: true,
      files: [file("a.ts", "M"), file("new.ts", "untracked")],
      user: { name: "tester", email: "t@t" },
    });
    mocks.gitDiffMock.mockResolvedValue({
      isRepo: true,
      patches: [patch("a.ts", 2, 1), patch("new.ts", 1, 0)],
    });
    mocks.gitRecentLogMock.mockResolvedValue({
      isRepo: true,
      commits: [{ sha: "a".repeat(40), summary: "init", time: 0 }],
    });
    await renderOpen();
    // 三命令都以当前会话调用
    expect(mocks.gitStatusMock).toHaveBeenCalledWith({ sessionId: "s1" });
    expect(mocks.gitDiffMock).toHaveBeenCalledWith({ sessionId: "s1" });
    // 文件列表两行（状态 + ±N 统计）
    expect(screen.getByTestId("changes-file-a.ts")).toBeTruthy();
    expect(screen.getByTestId("changes-file-new.ts")).toBeTruthy();
    expect(screen.getByText("+2")).toBeTruthy();
    expect(screen.getByText("−1")).toBeTruthy();
    // 缺省选中第一个文件并渲染其 diff
    expect(screen.getByTestId("changes-diff")).toBeTruthy();
    // recent log 折叠条展开后有提交行
    fireEvent.click(screen.getByText(/最近提交/));
    expect(screen.getByText("init")).toBeTruthy();
    // user info 弱化行
    expect(screen.getByTestId("changes-user").textContent).toContain("tester");
  });

  it("focusPath 定位该文件的 diff（openChangesPanel 联动接口）", async () => {
    mocks.gitStatusMock.mockResolvedValue({
      isRepo: true,
      files: [file("a.ts", "M"), file("b.ts", "M")],
    });
    mocks.gitDiffMock.mockResolvedValue({
      isRepo: true,
      patches: [patch("a.ts", 1, 1), patch("b.ts", 5, 0)],
    });
    await act(async () => {
      openChangesPanel("s1", "b.ts");
    });
    render(<ChangesPanel />);
    await act(async () => {});
    // 选中项 = b.ts（联动定位）：其 diff 补丁渲染在右列
    const diff = screen.getByTestId("changes-diff");
    expect(diff.textContent).toContain("b.ts");
    expect(diff.textContent).toContain("+5");
  });

  it("非 git 仓库显示空态不报错（验收第三条）", async () => {
    mocks.gitStatusMock.mockResolvedValue({ isRepo: false, files: [] });
    mocks.gitDiffMock.mockResolvedValue({ isRepo: false, patches: [] });
    mocks.gitRecentLogMock.mockResolvedValue({ isRepo: false, commits: [] });
    await renderOpen();
    expect(screen.getByTestId("changes-empty").textContent).toContain("当前项目不是 git 仓库");
    expect(screen.queryByTestId("changes-file-list")).toBeNull();
  });

  it("仓库干净时显示无变更空态", async () => {
    mocks.gitStatusMock.mockResolvedValue({ isRepo: true, files: [] });
    await renderOpen();
    expect(screen.getByTestId("changes-clean").textContent).toContain("工作区干净");
  });

  it("拉取失败显示错误行（降级不白屏）", async () => {
    mocks.gitStatusMock.mockRejectedValue(new Error("ipc down"));
    await renderOpen();
    expect(screen.getByTestId("changes-error").textContent).toContain("ipc down");
  });

  it("手动刷新按钮重拉三命令", async () => {
    mocks.gitStatusMock.mockResolvedValue({ isRepo: true, files: [] });
    await renderOpen();
    expect(mocks.gitStatusMock).toHaveBeenCalledTimes(1);
    await act(async () => {
      fireEvent.click(screen.getByTestId("changes-refresh"));
    });
    expect(mocks.gitStatusMock).toHaveBeenCalledTimes(2);
    expect(mocks.gitDiffMock).toHaveBeenCalledTimes(2);
  });

  it("关闭按钮回写 store（open=false）", async () => {
    mocks.gitStatusMock.mockResolvedValue({ isRepo: true, files: [] });
    await renderOpen();
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(useChangesStore.getState().open).toBe(false);
  });
});
