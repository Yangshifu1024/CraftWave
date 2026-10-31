// @vitest-environment happy-dom
/**
 * MCP 管理页测试：服务器行（配置 × 快照合并视图）、
 * 启停按钮、mcp:status 事件驱动的状态徽标、双作用域配置编辑保存（未知
 * 字段 round-trip）。IPC 全部经 vi.mock 替换。
 * 精简版：每功能面留代表 1 例，相关段合并。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { McpPage } from "./McpPage";

// ---- IPC 测试桩（vi.hoisted：vi.mock 工厂随 import 提前执行，桩须同升）----

const mocks = vi.hoisted(() => {
  const listConfigMock = vi.fn<() => Promise<McpListConfigResultStub>>();
  const snapshotMock = vi.fn<() => Promise<McpSnapshotResultStub>>();
  const connectMock = vi.fn<(scope: string, name: string) => Promise<void>>();
  const disconnectMock = vi.fn<(scope: string, name: string) => Promise<void>>();
  const saveConfigMock = vi.fn<(args: { scope: string; config: unknown }) => Promise<void>>();
  const testMock = vi.fn<(args: { draft: { command?: string } }) => Promise<McpTestResultStub>>();
  /** mcp:status 订阅桩：保存 handler 供用例直接派发事件。 */
  const statusHandlers = new Set<(payload: McpStatusEventStub) => void>();
  const listenMock = vi.fn<(key: string, handler: (payload: never) => void) => Promise<() => void>>(
    (key, handler) => {
      if (key === "mcp:status") {
        statusHandlers.add(handler as (payload: McpStatusEventStub) => void);
      }
      return Promise.resolve(() =>
        statusHandlers.delete(handler as (payload: McpStatusEventStub) => void),
      );
    },
  );
  return {
    listConfigMock,
    snapshotMock,
    connectMock,
    disconnectMock,
    saveConfigMock,
    testMock,
    statusHandlers,
    listenMock,
  };
});

/** 桩的类型占位（hoisted 块内不能引用外部类型，用结构等价的本地别名）。 */
type McpListConfigResultStub = {
  global: { mcpServers: Record<string, unknown> };
  project: { mcpServers: Record<string, unknown> };
  projectAvailable: boolean;
};
type McpSnapshotResultStub = {
  servers: {
    name: string;
    scope: string;
    state: string;
    toolCount: number;
    detail?: string;
    fingerprint: string;
    trustStatus: string;
  }[];
};
type McpTestResultStub = {
  ok: boolean;
  tools: { name: string; description: string }[];
  error?: string;
};
type McpStatusEventStub = {
  scope: string;
  name: string;
  state: "starting" | "ready" | "stopped" | "error";
  detail?: string;
};

const {
  listConfigMock,
  snapshotMock,
  connectMock,
  disconnectMock,
  saveConfigMock,
  statusHandlers,
} = mocks;

vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    mcpListConfig: () => mocks.listConfigMock(),
    mcpSnapshot: () => mocks.snapshotMock(),
    mcpConnect: (scope: string, name: string) => mocks.connectMock(scope, name),
    mcpDisconnect: (scope: string, name: string) => mocks.disconnectMock(scope, name),
    mcpSaveConfig: (args: { scope: string; config: unknown }) => mocks.saveConfigMock(args),
    mcpTest: (args: { draft: { command?: string } }) => mocks.testMock(args),
    listenEvent: mocks.listenMock as unknown as typeof actual.listenEvent,
  };
});

beforeEach(() => {
  listConfigMock.mockReset();
  snapshotMock.mockReset();
  connectMock.mockReset().mockResolvedValue(undefined);
  disconnectMock.mockReset().mockResolvedValue(undefined);
  saveConfigMock.mockReset().mockResolvedValue(undefined);
  mocks.testMock.mockReset();
  statusHandlers.clear();
  // 空配置 + 空快照（各用例按需覆盖）
  listConfigMock.mockResolvedValue({
    global: { mcpServers: {} },
    project: { mcpServers: {} },
    projectAvailable: false,
  });
  snapshotMock.mockResolvedValue({ servers: [] });
});
afterEach(cleanup);

/** 带一个全局服务器（已配置、未连接、待决策）的基线数据。 */
function seedOneServer(overrides?: Partial<McpSnapshotResultStub["servers"][number]>) {
  listConfigMock.mockResolvedValue({
    global: {
      mcpServers: { filesystem: { command: "npx", args: ["-y", "mcp-fs"] } },
    },
    project: { mcpServers: {} },
    projectAvailable: true,
  });
  snapshotMock.mockResolvedValue({
    servers: [
      {
        name: "filesystem",
        scope: "global",
        state: "stopped",
        toolCount: 0,
        fingerprint: "a1b2c3d4e5f60718",
        trustStatus: "pending",
        ...overrides,
      },
    ],
  });
}

describe("McpPage", () => {
  it("服务器行合并视图（名称/作用域/信任列/状态）；无配置时空态提示", async () => {
    seedOneServer();
    const view = render(<McpPage />);
    const row = await waitFor(() => screen.getByTestId("mcp-server-row"));
    expect(row.textContent).toContain("filesystem");
    expect(row.textContent).toContain("全局");
    expect(screen.getByTestId("mcp-state-filesystem").textContent).toContain("已停止");
    expect(screen.getByTestId("mcp-trust-filesystem").textContent).toBe("待决策");
    view.unmount();

    // 无配置空态
    const empty = render(<McpPage />);
    await waitFor(() =>
      expect(screen.getByTestId("mcp-servers-section").textContent).toContain("还没有配置"),
    );
    empty.unmount();
  });

  it("启停按钮（停止态连接 / ready 态停止）；mcp:status 事件驱动徽标迁移与快照刷新", async () => {
    // 停止态点「连接」调 mcp_connect；ready 态点「停止」调 mcp_disconnect
    seedOneServer();
    const view = render(<McpPage />);
    const toggle = await waitFor(() => screen.getByTestId("mcp-toggle-filesystem"));
    fireEvent.click(toggle);
    await waitFor(() => expect(connectMock).toHaveBeenCalledWith("global", "filesystem"));
    view.unmount();

    cleanup();
    seedOneServer({ state: "ready", toolCount: 3 });
    const ready = render(<McpPage />);
    const stop = await waitFor(() => screen.getByTestId("mcp-toggle-filesystem"));
    expect(stop.textContent).toBe("停止");
    fireEvent.click(stop);
    await waitFor(() => expect(disconnectMock).toHaveBeenCalledWith("global", "filesystem"));
    ready.unmount();

    // 事件迁移：starting → ready（ready 后按快照刷新信任列——事件不带信任面）
    cleanup();
    seedOneServer();
    snapshotMock.mockResolvedValue({
      servers: [
        {
          name: "filesystem",
          scope: "global",
          state: "ready",
          toolCount: 2,
          fingerprint: "a1b2c3d4e5f60718",
          trustStatus: "trusted",
        },
      ],
    });
    const live = render(<McpPage />);
    await waitFor(() => expect(statusHandlers.size).toBeGreaterThan(0));
    for (const handler of statusHandlers) {
      handler({ scope: "global", name: "filesystem", state: "starting" });
    }
    await waitFor(() =>
      expect(screen.getByTestId("mcp-state-filesystem").textContent).toContain("启动中"),
    );
    for (const handler of statusHandlers) {
      handler({ scope: "global", name: "filesystem", state: "ready", detail: "2 个工具已注册" });
    }
    await waitFor(() =>
      expect(screen.getByTestId("mcp-state-filesystem").textContent).toContain("2 工具"),
    );
    await waitFor(() =>
      expect(screen.getByTestId("mcp-trust-filesystem").textContent).toBe("已记住"),
    );
    live.unmount();
  });

  it("配置编辑保存：载荷携带解析后的整份配置（未知字段保真）；JSON 语法错误不发起保存", async () => {
    listConfigMock.mockResolvedValue({
      global: {
        mcpServers: {
          filesystem: { command: "npx", futureField: "未知字段保存不丢" },
        },
      },
      project: { mcpServers: {} },
      projectAvailable: true,
    });
    const view = render(<McpPage />);
    const editor = (await waitFor(() =>
      screen.getByTestId("mcp-config-editor"),
    )) as HTMLTextAreaElement;
    // 全局作用域原文进编辑器（未知字段在场）
    const text = JSON.parse(editor.value) as { mcpServers: Record<string, unknown> };
    expect(text.mcpServers.filesystem).toMatchObject({
      command: "npx",
      futureField: "未知字段保存不丢",
    });

    // 用户编辑后保存：载荷 = 解析后的整份 JSON（未知字段原样跟随）
    fireEvent.change(editor, {
      target: {
        value: JSON.stringify({
          mcpServers: { filesystem: { command: "node", futureField: "仍在" } },
        }),
      },
    });
    fireEvent.click(screen.getByTestId("mcp-save-config"));
    await waitFor(() =>
      expect(saveConfigMock).toHaveBeenCalledWith({
        scope: "global",
        config: { mcpServers: { filesystem: { command: "node", futureField: "仍在" } } },
      }),
    );
    await waitFor(() => expect(screen.getByTestId("mcp-save-notice")).toBeTruthy());
    view.unmount();

    // JSON 语法错误行内提示，不发起保存
    saveConfigMock.mockClear();
    const bad = render(<McpPage />);
    const editor2 = await waitFor(
      () => screen.getByTestId("mcp-config-editor") as HTMLTextAreaElement,
    );
    fireEvent.change(editor2, { target: { value: "{ 不是 JSON" } });
    fireEvent.click(screen.getByTestId("mcp-save-config"));
    await waitFor(() => expect(screen.getByTestId("mcp-save-error")).toBeTruthy());
    expect(saveConfigMock).not.toHaveBeenCalled();
    bad.unmount();
  });
});
