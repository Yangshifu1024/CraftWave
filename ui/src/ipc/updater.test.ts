/**
 * ipc/updater.ts 单测（33 号票）：静默检查间隔判定纯函数表驱动 +
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

describe("shouldSilentCheck（静默检查间隔判定，表驱动）", () => {
  const HOUR = 3_600_000;

  it("enabled=false 恒不检查（NFR-11：不发起更新外联）", () => {
    expect(shouldSilentCheck(1_000, 0, 24, false)).toBe(false);
    expect(shouldSilentCheck(1_000, 0, 0, false)).toBe(false);
  });

  it("从未检查过（记账缺省 0）→ 检查", () => {
    expect(shouldSilentCheck(5_000, 0, 24, true)).toBe(true);
    expect(shouldSilentCheck(5_000, -1, 24, true)).toBe(true);
  });

  it("距上次检查不足 intervalHours → 跳过；达到 → 检查", () => {
    const last = 1_000_000;
    expect(shouldSilentCheck(last + 24 * HOUR - 1, last, 24, true)).toBe(false);
    expect(shouldSilentCheck(last + 24 * HOUR, last, 24, true)).toBe(true);
    // 间隔可调：1 小时档
    expect(shouldSilentCheck(last + HOUR, last, 1, true)).toBe(true);
    expect(shouldSilentCheck(last + HOUR - 1, last, 1, true)).toBe(false);
  });

  it("intervalHours 非正（配置损坏兜底）→ 不检查", () => {
    expect(shouldSilentCheck(10_000, 0, 0, true)).toBe(false);
    expect(shouldSilentCheck(10_000, 0, -5, true)).toBe(false);
  });
});

describe("checkForUpdate（三态收敛）", () => {
  it("插件返回 null → upToDate", async () => {
    checkMock.mockResolvedValue(null);
    await expect(checkForUpdate()).resolves.toEqual({ kind: "upToDate" });
  });

  it("插件返回更新 → available 三字段 + 资源暂存（discard 释放）", async () => {
    const update = {
      version: "0.2.0",
      currentVersion: "0.1.0",
      body: "修复说明",
      close: closeMock,
      downloadAndInstall: downloadAndInstallMock,
    };
    checkMock.mockResolvedValue(update);
    await expect(checkForUpdate()).resolves.toEqual({
      kind: "available",
      version: "0.2.0",
      currentVersion: "0.1.0",
      notes: "修复说明",
    });
    expect(discardPendingUpdate()).toBeUndefined();
    expect(closeMock).toHaveBeenCalledTimes(1);
    // 幂等：再次 discard 不再 close
    discardPendingUpdate();
    expect(closeMock).toHaveBeenCalledTimes(1);
  });

  it("插件抛错 → failed 带原始消息（不抛出）", async () => {
    checkMock.mockRejectedValue(new Error("updater 配置无效"));
    await expect(checkForUpdate()).resolves.toEqual({
      kind: "failed",
      message: "updater 配置无效",
    });
  });

  it("非 Error 抛出物 → failed 带 String 化消息", async () => {
    checkMock.mockRejectedValue("网络断开");
    await expect(checkForUpdate()).resolves.toEqual({
      kind: "failed",
      message: "网络断开",
    });
  });
});

describe("downloadAndInstallUpdate（进度回调链）", () => {
  it("无暂存更新时拒绝（调用序错误）", async () => {
    await expect(downloadAndInstallUpdate()).rejects.toThrow("no update checked");
  });

  it("进度事件映射：Started/Progress/Finished → downloading/installing/done", async () => {
    const update = {
      version: "0.2.0",
      currentVersion: "0.1.0",
      close: closeMock,
      downloadAndInstall: downloadAndInstallMock.mockImplementation(async (onEvent) => {
        onEvent({ event: "Started", data: { contentLength: 1024 } });
        onEvent({ event: "Progress", data: { chunkLength: 256 } });
        onEvent({ event: "Finished" });
      }),
    };
    checkMock.mockResolvedValue(update);
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
});

describe("relaunchForUpdate", () => {
  it("转发 process 插件 relaunch", async () => {
    relaunchMock.mockResolvedValue(undefined);
    await relaunchForUpdate();
    expect(relaunchMock).toHaveBeenCalledTimes(1);
  });
});
