// @vitest-environment happy-dom
/**
 * 配额 store 单测（21 号票，FR-02-15）：
 * - providerOfModel：会话覆盖模型 > 全局 activeModelId 的归属解析（票面
 *   「首屏 = 当前会话所用 provider」的纯函数锚点）；
 * - worstRemainingPercent：多窗口取最紧张（CodeWave 择劣口径）；
 * - store.load：成功写入 / 失败静默保留旧值（离线兜底）/ 并发去重。
 *
 * IPC 经 vi.mock 替换（SkillsPage.test 同款桩面）。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig, QuotaEntry, QuotaSnapshot } from "@/ipc/types";
import { providerOfModel, useQuotaStore, worstRemainingPercent } from "./quota-store";

// ---- IPC 测试桩（vi.hoisted：vi.mock 工厂随 import 提前执行，桩须同升）----

const mocks = vi.hoisted(() => ({
  quotaSnapshotsMock: vi.fn<() => Promise<unknown>>(),
}));

vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    quotaSnapshots: () => mocks.quotaSnapshotsMock(),
  };
});

beforeEach(() => {
  mocks.quotaSnapshotsMock.mockReset().mockResolvedValue([]);
  useQuotaStore.setState({ rows: null, loading: false });
});

/** 最小配置：两家供应商各一模型。 */
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

describe("providerOfModel：当前会话所用 provider 解析", () => {
  const config = baseConfig();

  it("会话覆盖模型优先于全局 activeModelId", () => {
    expect(providerOfModel(config, "m-zai")).toBe("p-zai");
  });

  it("无覆盖时回落全局 activeModelId 的归属", () => {
    expect(providerOfModel(config, undefined)).toBe("p-deepseek");
  });

  it("模型悬空 / 未配置激活模型 → undefined", () => {
    expect(providerOfModel(config, "m-ghost")).toBeUndefined();
    expect(providerOfModel({ ...config, activeModelId: undefined }, undefined)).toBeUndefined();
  });
});

describe("worstRemainingPercent：多窗口择劣", () => {
  const entry = (remainingPercent?: number): QuotaEntry => ({
    key: "five_hour",
    remainingPercent,
  });

  it("取全部百分比窗口的最小剩余", () => {
    expect(worstRemainingPercent([entry(80), entry(12.5), entry(45)])).toBe(12.5);
  });

  it("无数值行（余额类）与空明细 → null", () => {
    expect(worstRemainingPercent([{ key: "balance_cny", valueText: "CNY 1.00" }])).toBeNull();
    expect(worstRemainingPercent([])).toBeNull();
  });
});

describe("useQuotaStore.load：快照拉取", () => {
  const row = (providerId: string, state: QuotaSnapshot["state"]): QuotaSnapshot => ({
    providerId,
    state,
    fetchedAt: "2026-10-30T08:00:00Z",
  });

  it("成功写入行面", async () => {
    mocks.quotaSnapshotsMock.mockResolvedValue([row("p-deepseek", "ok")]);
    await useQuotaStore.getState().load();
    expect(useQuotaStore.getState().rows).toHaveLength(1);
    expect(useQuotaStore.getState().loading).toBe(false);
  });

  it("失败静默：保留旧 rows 不抛错（离线兜底）", async () => {
    mocks.quotaSnapshotsMock.mockResolvedValueOnce([row("p-deepseek", "ok")]);
    await useQuotaStore.getState().load();
    mocks.quotaSnapshotsMock.mockRejectedValueOnce(new Error("断网"));
    await useQuotaStore.getState().load();
    expect(useQuotaStore.getState().rows).toHaveLength(1);
    expect(useQuotaStore.getState().loading).toBe(false);
  });

  it("并发去重：进行中不重复发起", async () => {
    let resolve!: (value: QuotaSnapshot[]) => void;
    mocks.quotaSnapshotsMock.mockReturnValue(
      new Promise<QuotaSnapshot[]>((r) => {
        resolve = r;
      }),
    );
    const first = useQuotaStore.getState().load();
    const second = useQuotaStore.getState().load();
    resolve([]);
    await Promise.all([first, second]);
    expect(mocks.quotaSnapshotsMock).toHaveBeenCalledTimes(1);
  });
});
