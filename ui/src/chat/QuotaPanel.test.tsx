// @vitest-environment happy-dom
/**
 * QuotaPanel 交互测试（21 号票，FR-02-15 Inspector 配额页）：
 * - 首屏大卡 = 当前会话所用 provider（ok 态：名称/剩余%/进度条/明细缺省展开）；
 * - 六态行渲染：ok / error（原因 + 重试 + lastOkAt + 离线兜底明细标注）/
 *   rejected / no_key（「去设置」跳转）/ unsupported（灰行占位不报错）；
 * - unsupported >3 家折叠为一行汇总；
 * - 刷新策略：手动刷新按钮、切 provider 即时刷新、5min 周期且卸载即停
 *   （visible 门控 = 条件渲染卸载）。
 *
 * IPC 经 vi.mock 替换（SkillsPage.test 同款桩面）；会话模型覆盖直接
 * setState 真实 store（session-model 为纯内存 zustand）。
 */

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QUOTA_REFRESH_INTERVAL_MS, useQuotaStore } from "@/chat/quota-store";
import { useSessionModelStore } from "@/chat/session-model";
import type { AppConfig, QuotaSnapshot } from "@/ipc/types";
import { QuotaPanel } from "./QuotaPanel";

// ---- IPC 测试桩（vi.hoisted：vi.mock 工厂随 import 提前执行，桩须同升）----

const mocks = vi.hoisted(() => ({
  quotaSnapshotsMock: vi.fn<() => Promise<unknown>>(),
  getConfigMock: vi.fn<() => Promise<unknown>>(),
}));

vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    quotaSnapshots: () => mocks.quotaSnapshotsMock(),
    getConfig: () => mocks.getConfigMock(),
  };
});

beforeEach(() => {
  vi.useFakeTimers();
  mocks.quotaSnapshotsMock.mockReset().mockResolvedValue([]);
  mocks.getConfigMock.mockReset().mockResolvedValue(baseConfig());
  useQuotaStore.setState({ rows: null, loading: false });
  useSessionModelStore.setState({ bySession: {}, openRequest: null });
});
afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

/** 最小配置：DeepSeek（激活模型归属）与 Z.ai 两家。 */
function baseConfig(): AppConfig {
  return {
    schemaVersion: 2,
    providers: [
      {
        id: "p-deepseek",
        name: "DeepSeek",
        apiFormat: "openai_chat",
        models: [{ id: "m-ds", model: "deepseek-chat", maxTokens: 0, contextWindow: 0 }],
      },
      {
        id: "p-zai",
        name: "Z.ai",
        apiFormat: "openai_chat",
        models: [{ id: "m-zai", model: "glm-4.7", maxTokens: 0, contextWindow: 0 }],
      },
    ],
    activeModelId: "m-ds",
    approval: {
      enabled: true,
      autoConfirm: false,
      sessionMemoryEnabled: true,
      allowlistEnabled: true,
      commandAllowlist: [],
      requirePlan: false,
      gitAllowStageCommit: false,
    },
    ui: {
      language: "zh-CN",
      theme: "system",
      closeToTray: true,
      hotkey: { enabled: true, accelerator: "alt+space" },
    },
    log: { level: "info" },
    notifications: { enabled: true, interventionOnly: false },
    compaction: { threshold: 0.6, timeoutSecs: 180 },
    tools: { maxParallel: 4, batchReadLimit: 20 },
    watchdog: { stallTimeoutSecs: 300 },
    network: { allowPrivateNetwork: false },
    search: { searxng: { enabled: false, baseUrl: "http://127.0.0.1:8888", maxResults: 10 } },
  } as AppConfig;
}

/** 快照行工厂（默认 ok + 一个百分比窗口）。 */
function row(overrides: Partial<QuotaSnapshot> & { providerId: string }): QuotaSnapshot {
  return {
    state: "ok",
    fetchedAt: "2026-10-30T08:00:00Z",
    ...overrides,
  } as QuotaSnapshot;
}

const okData = (
  entries: QuotaSnapshot["data"] extends undefined
    ? never
    : NonNullable<QuotaSnapshot["data"]>["entries"],
) => ({
  displayName: "DeepSeek",
  entries,
});

const percentEntry = (key: string, remaining: number) => ({
  key,
  usedPercent: Math.round((100 - remaining) * 10) / 10,
  remainingPercent: remaining,
});

async function renderPanel(props?: { sessionId?: string; onOpenSettings?: () => void }) {
  const onOpenSettings = props?.onOpenSettings ?? vi.fn();
  const view = render(
    <QuotaPanel
      sessionId={props?.sessionId ?? "s1"}
      onOpenSettings={props?.onOpenSettings ?? onOpenSettings}
    />,
  );
  // 首拉完成（getConfig + quotaSnapshots）
  await act(async () => {
    await vi.runOnlyPendingTimersAsync();
    await Promise.resolve();
    await Promise.resolve();
  });
  return { onOpenSettings, ...view };
}

describe("首屏与六态行渲染", () => {
  it("ok 态首屏大卡：名称 + 剩余% + 进度条 + 缺省展开窗口明细", async () => {
    mocks.quotaSnapshotsMock.mockResolvedValue([
      row({
        providerId: "p-deepseek",
        lastOkAt: "2026-10-30T08:00:00Z",
        data: {
          displayName: "DeepSeek",
          entries: [percentEntry("five_hour", 62.4), percentEntry("weekly", 88)],
        },
      }),
    ]);
    await renderPanel();
    // 首屏大卡（当前会话 provider = p-deepseek，activeModelId 归属）
    expect(screen.getByTestId("quota-current-card").textContent).toContain("DeepSeek");
    expect(screen.getByTestId("quota-current-card").textContent).toContain("38%");
    expect(screen.getByTestId("quota-row-p-deepseek").getAttribute("data-quota-state")).toBe("ok");
    // 明细缺省展开：两个窗口各自「剩余 N%」
    expect(screen.getByTestId("quota-detail-p-deepseek").textContent).toContain("剩余 62%");
    expect(screen.getByTestId("quota-detail-p-deepseek").textContent).toContain("剩余 88%");
  });

  it("error 态：警示副文案 + 展开原因/重试/lastOkAt 标注离线兜底明细", async () => {
    mocks.quotaSnapshotsMock.mockResolvedValue([
      row({
        providerId: "p-deepseek",
        state: "error",
        lastOkAt: "2026-10-29T20:00:00Z",
        data: {
          displayName: "DeepSeek",
          entries: [percentEntry("five_hour", 40)],
          error: "DeepSeek API 401：认证失效",
          keySource: "config",
        },
      }),
    ]);
    await renderPanel();
    const line = screen.getByTestId("quota-row-p-deepseek");
    expect(line.getAttribute("data-quota-state")).toBe("error");
    expect(line.textContent).toContain("查询出错");
    // 展开区：原因 + 重试按钮 + 时点标注（离线兜底明细以最近成功时点说明）
    const detail = screen.getByTestId("quota-detail-p-deepseek");
    expect(detail.textContent).toContain("401");
    expect(screen.getByTestId("quota-retry-p-deepseek")).toBeTruthy();
    expect(detail.textContent).toContain("获取的最近数据");
    // 行面带「上次成功」标注
    expect(line.textContent).toContain("上次成功");
    // 离线兜底明细：失败行带最近一次成功数据
    expect(detail.textContent).toContain("剩余 40%");
  });

  it("rejected 态：灰行示警且可展开原因（密钥被拒）", async () => {
    mocks.quotaSnapshotsMock.mockResolvedValue([
      row({
        providerId: "p-zai",
        state: "rejected",
        data: {
          displayName: "Z.ai",
          entries: [],
          error: "Z.ai API 401：无效令牌",
        },
      }),
    ]);
    await renderPanel();
    const line = screen.getByTestId("quota-row-p-zai");
    expect(line.getAttribute("data-quota-state")).toBe("rejected");
    expect(line.textContent).toContain("查询被拒");
    fireEvent.click(line);
    expect(screen.getByTestId("quota-detail-p-zai").textContent).toContain("无效令牌");
  });

  it("no_key 态：标签行 + 「去设置」跳转设置页回调", async () => {
    mocks.quotaSnapshotsMock.mockResolvedValue([
      row({
        providerId: "p-zai",
        state: "no_key",
        data: { displayName: "Z.ai", entries: [] },
      }),
    ]);
    const onOpenSettings = vi.fn();
    await renderPanel({ onOpenSettings });
    const line = screen.getByTestId("quota-row-p-zai");
    expect(line.getAttribute("data-quota-state")).toBe("no_key");
    expect(line.textContent).toContain("未配置 Key");
    fireEvent.click(screen.getByTestId("quota-goto-settings-p-zai"));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("unsupported 态：灰行「暂不支持查询」占位不报错", async () => {
    mocks.quotaSnapshotsMock.mockResolvedValue([
      row({
        providerId: "p-local",
        state: "unsupported",
        data: {
          displayName: "本地模型",
          entries: [],
          reason: "no_adapter",
        },
      }),
    ]);
    await renderPanel();
    const line = screen.getByTestId("quota-row-p-local");
    expect(line.getAttribute("data-quota-state")).toBe("unsupported");
    expect(line.textContent).toContain("暂不支持查询");
  });

  it("unsupported 超过 3 家折叠为一行汇总（点击展开）", async () => {
    const unsupported = [1, 2, 3, 4].map((i) =>
      row({
        providerId: `p-gw-${i}`,
        state: "unsupported",
        data: { displayName: `网关 ${i}`, entries: [], reason: "no_adapter" },
      }),
    );
    mocks.quotaSnapshotsMock.mockResolvedValue(unsupported);
    await renderPanel();
    // 4 家 > 3 → 折叠为一行汇总，灰行隐藏
    expect(screen.getByTestId("quota-unsupported-summary").textContent).toContain("4 家");
    expect(screen.queryByTestId("quota-row-p-gw-1")).toBeNull();
    fireEvent.click(screen.getByTestId("quota-unsupported-summary"));
    expect(screen.getByTestId("quota-row-p-gw-1")).toBeTruthy();
  });

  it("当前会话无可识别供应商时给出占位（不报错）", async () => {
    mocks.getConfigMock.mockResolvedValue({
      ...baseConfig(),
      activeModelId: undefined,
    });
    await renderPanel();
    expect(screen.getByTestId("quota-no-active").textContent).toContain("没有使用可识别的供应商");
  });
});

describe("刷新策略（FR-02-15）", () => {
  it("手动刷新按钮重拉快照", async () => {
    await renderPanel();
    const calls = mocks.quotaSnapshotsMock.mock.calls.length;
    fireEvent.click(screen.getByTestId("quota-refresh"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(mocks.quotaSnapshotsMock.mock.calls.length).toBeGreaterThan(calls);
  });

  it("切换 provider（会话覆盖模型）即时刷新且首屏卡随之切换", async () => {
    mocks.quotaSnapshotsMock.mockResolvedValue([
      row({
        providerId: "p-deepseek",
        data: okData([percentEntry("five_hour", 60)]),
      }),
      row({
        providerId: "p-zai",
        data: { displayName: "Z.ai", entries: [percentEntry("five_hour", 15)] },
      }),
    ]);
    await renderPanel();
    expect(screen.getByTestId("quota-current-card").textContent).toContain("DeepSeek");
    const calls = mocks.quotaSnapshotsMock.mock.calls.length;
    // Ctrl+L 切换会话模型 → 即时刷新
    act(() => {
      useSessionModelStore.setState({ bySession: { s1: "m-zai" } });
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(mocks.quotaSnapshotsMock.mock.calls.length).toBeGreaterThan(calls);
    expect(screen.getByTestId("quota-current-card").textContent).toContain("Z.ai");
  });

  it("配额页可见时每 5 分钟周期刷新；卸载即停（visible 门控）", async () => {
    await renderPanel();
    const calls = mocks.quotaSnapshotsMock.mock.calls.length;
    // 5 分钟周期
    await act(async () => {
      await vi.advanceTimersByTimeAsync(QUOTA_REFRESH_INTERVAL_MS);
    });
    expect(mocks.quotaSnapshotsMock.mock.calls.length).toBe(calls + 1);
    // 卸载（配额页不可见）→ 定时器清理，时间推进不再拉取
    cleanup();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(QUOTA_REFRESH_INTERVAL_MS * 3);
    });
    expect(mocks.quotaSnapshotsMock.mock.calls.length).toBe(calls + 1);
  });
});
