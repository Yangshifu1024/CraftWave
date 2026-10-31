/**
 * ipc/updater.ts 单测：静默检查间隔判定纯函数表驱动 +
 * checkForUpdate 三态收敛（插件 mock）+ 暂存资源释放纪律。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// 插件 mock（vi.mock 提升：check 返回值由用例控制）
const checkMock = vi.fn();
const closeMock = vi.fn();
const downloadAndInstallMock = vi.fn();
const relaunchMock = vi.fn();

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: (...args: unknown[]) => checkMock(...args),
}));

vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: (...args: unknown[]) => relaunchMock(...args),
}));

import {
  checkForUpdate,
  discardPendingUpdate,
  downloadAndInstallUpdate,
  relaunchForUpdate,
  shouldSilentCheck,
} from "./updater";

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  // 用例间不留悬挂更新资源（被测纪律本身）
  discardPendingUpdate();
});

describe("ipc/updater", () => {
  it("shouldSilentCheck（静默检查间隔判定，表驱动）：enabled 门、从未检查、interval 边界与可调档、非正 interval 兜底", () => {
    const HOUR = 3_600_000;
    const cases: [
      now: number,
      last: number,
      intervalHours: number,
      enabled: boolean,
      expected: boolean,
    ][] = [
      // enabled=false 恒不检查（不发起更新外联）
      [1_000, 0, 24, false, false],
      [1_000, 0, 0, false, false],
      // 从未检查过（记账缺省 0/负数）→ 检查
      [5_000, 0, 24, true, true],
      [5_000, -1, 24, true, true],
      // 距上次检查不足 intervalHours → 跳过；达到 → 检查
      [1_000_000 + 24 * HOUR - 1, 1_000_000, 24, true, false],
      [1_000_000 + 24 * HOUR, 1_000_000, 24, true, true],
      // 间隔可调：1 小时档
      [1_000_000 + HOUR, 1_000_000, 1, true, true],
      [1_000_000 + HOUR - 1, 1_000_000, 1, true, false],
      // intervalHours 非正（配置损坏兜底）→ 不检查
      [10_000, 0, 0, true, false],
      [10_000, 0, -5, true, false],
    ];
    for (const [now, last, intervalHours, enabled, expected] of cases) {
      expect(shouldSilentCheck(now, last, intervalHours, enabled)).toBe(expected);
    }
  });

  it("checkForUpdate 三态收敛 + 暂存资源释放：null→upToDate、available 三字段、抛错→failed（Error 与非 Error 都 String 化不抛出）", async () => {
    // 插件返回 null → upToDate
    checkMock.mockResolvedValue(null);
    await expect(checkForUpdate()).resolves.toEqual({ kind: "upToDate" });

    // 插件返回更新 → available 三字段 + 资源暂存（discard 释放，幂等）
    checkMock.mockResolvedValue({
      version: "0.2.0",
      currentVersion: "0.1.0",
      body: "修复说明",
      close: closeMock,
      downloadAndInstall: downloadAndInstallMock,
    });
    await expect(checkForUpdate()).resolves.toEqual({
      kind: "available",
      version: "0.2.0",
      currentVersion: "0.1.0",
      notes: "修复说明",
    });
    expect(discardPendingUpdate()).toBeUndefined();
    expect(closeMock).toHaveBeenCalledTimes(1);
    discardPendingUpdate();
    expect(closeMock).toHaveBeenCalledTimes(1);

    // 插件抛错 → failed 带原始消息（不抛出）；非 Error 抛出物 String 化
    checkMock.mockRejectedValue(new Error("updater 配置无效"));
    await expect(checkForUpdate()).resolves.toEqual({
      kind: "failed",
      message: "updater 配置无效",
    });
    checkMock.mockRejectedValue("网络断开");
    await expect(checkForUpdate()).resolves.toEqual({ kind: "failed", message: "网络断开" });
  });

  it("downloadAndInstallUpdate：无暂存更新时拒绝（调用序错误）；进度事件映射 Started/Progress/Finished → downloading/installing/done", async () => {
    await expect(downloadAndInstallUpdate()).rejects.toThrow("no update checked");

    checkMock.mockResolvedValue({
      version: "0.2.0",
      currentVersion: "0.1.0",
      close: closeMock,
      downloadAndInstall: downloadAndInstallMock.mockImplementation(async (onEvent) => {
        onEvent({ event: "Started", data: { contentLength: 1024 } });
        onEvent({ event: "Progress", data: { chunkLength: 256 } });
        onEvent({ event: "Finished" });
      }),
    });
    await checkForUpdate();
    const progress: string[] = [];
    await downloadAndInstallUpdate((p) => progress.push(p.kind));
    expect(progress).toEqual([
      "downloading", // 调用起始
      "downloading", // Started
      "downloading", // Progress
      "installing", // Finished
      "done", // 完成收口
    ]);
  });

  it("relaunchForUpdate 转发 process 插件 relaunch", async () => {
    relaunchMock.mockResolvedValue(undefined);
    await relaunchForUpdate();
    expect(relaunchMock).toHaveBeenCalledTimes(1);
  });
});
