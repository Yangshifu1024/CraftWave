// @vitest-environment happy-dom
/**
 * 设置页「检查更新」区组件测试（33 号票）：三态渲染（有新版 Dialog / 已最新
 * Toast / 失败 Toast）、禁用兜底门（enabled=false / pubkey 未配）、安装门前置
 * （updatePrecheck 阻止原因展示与放行路径）、重启按钮链。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdateSectionDeps } from "./UpdateSection";
import { UpdateSection } from "./UpdateSection";

const getConfig = vi.fn();
const updateStatus = vi.fn();
const updatePrecheck = vi.fn();
const checkForUpdate = vi.fn();
const downloadAndInstallUpdate = vi.fn();
const relaunchForUpdate = vi.fn();
const pushToast = vi.fn();

function deps(): UpdateSectionDeps {
  return {
    getConfig,
    updateStatus,
    updatePrecheck,
    checkForUpdate,
    downloadAndInstallUpdate,
    relaunchForUpdate,
    pushToast,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  getConfig.mockResolvedValue({ update: { enabled: true, intervalHours: 24 } });
  updateStatus.mockResolvedValue({ pubkeyConfigured: true, endpoints: ["https://e/latest.json"] });
});

afterEach(cleanup);

/** 渲染辅助：默认门全开。 */
function renderSection() {
  return render(<UpdateSection deps={deps()} />);
}

describe("禁用兜底门（票内 Answer：secrets 未配 / 配置关闭时入口禁用）", () => {
  it("update.enabled=false → 按钮禁用 + 显示配置关闭原因", async () => {
    getConfig.mockResolvedValue({ update: { enabled: false, intervalHours: 24 } });
    renderSection();
    await waitFor(() => expect(screen.getByTestId("update-section").textContent).toContain("关闭"));
    expect((screen.getByTestId("update-check-button") as HTMLButtonElement).disabled).toBe(true);
    expect(checkForUpdate).not.toHaveBeenCalled();
  });

  it("pubkey 未实配（占位空串）→ 按钮禁用 + 显示密钥未配原因", async () => {
    updateStatus.mockResolvedValue({ pubkeyConfigured: false, endpoints: [] });
    renderSection();
    await waitFor(() => expect(screen.getByTestId("update-section").textContent).toContain("公钥"));
    expect((screen.getByTestId("update-check-button") as HTMLButtonElement).disabled).toBe(true);
  });

  it("门全开 → 按钮可用", async () => {
    renderSection();
    await waitFor(() =>
      expect((screen.getByTestId("update-check-button") as HTMLButtonElement).disabled).toBe(false),
    );
  });
});

describe("检查三态", () => {
  it("已最新 → Toast 轻提示，不弹 Dialog", async () => {
    checkForUpdate.mockResolvedValue({ kind: "upToDate" });
    renderSection();
    await waitFor(() =>
      expect((screen.getByTestId("update-check-button") as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(screen.getByTestId("update-check-button"));
    await waitFor(() => expect(pushToast).toHaveBeenCalledWith("info", "当前已是最新版本。"));
    expect(screen.queryByTestId("update-dialog-body")).toBeNull();
  });

  it("失败 → Toast 错误提示带原始消息", async () => {
    checkForUpdate.mockResolvedValue({ kind: "failed", message: "网络断开" });
    renderSection();
    fireEvent.click(await screen.findByTestId("update-check-button"));
    await waitFor(() =>
      expect(pushToast).toHaveBeenCalledWith("error", "检查更新失败", "网络断开"),
    );
  });

  it("有新版 → Dialog 展示新版本号与 release notes", async () => {
    checkForUpdate.mockResolvedValue({
      kind: "available",
      version: "0.2.0",
      currentVersion: "0.1.0",
      notes: "修复了若干问题",
    });
    renderSection();
    fireEvent.click(await screen.findByTestId("update-check-button"));
    const body = await screen.findByTestId("update-dialog-body");
    expect(body.textContent).toContain("0.2.0");
    expect(body.textContent).toContain("修复了若干问题");
  });
});

describe("安装门（2026-10-29 用户决策：活跃 run / 待审批 / 未暂停队列阻止安装）", () => {
  /** 打开「有新版」Dialog 的公共前置。 */
  async function openAvailableDialog() {
    checkForUpdate.mockResolvedValue({
      kind: "available",
      version: "0.2.0",
      currentVersion: "0.1.0",
      notes: undefined,
    });
    renderSection();
    fireEvent.click(await screen.findByTestId("update-check-button"));
    await screen.findByTestId("update-dialog-body");
  }

  it("updatePrecheck 阻止 → 阻止原因逐行展示，不启动下载", async () => {
    await openAvailableDialog();
    updatePrecheck.mockResolvedValue({
      blocked: true,
      reasons: ["有 1 个会话的任务正在运行（或等待审批），安装重启会中断运行"],
      runningCount: 1,
      pendingAskCount: 0,
      runningQueueCount: 0,
      pausedQueueCount: 0,
    });
    fireEvent.click(screen.getByText("下载并安装"));
    await waitFor(() =>
      expect(screen.getByTestId("update-block-reasons").textContent).toContain("正在运行"),
    );
    expect(downloadAndInstallUpdate).not.toHaveBeenCalled();
  });

  it("放行 → 下载安装完成 → 「重启以完成更新」→ relaunch", async () => {
    await openAvailableDialog();
    updatePrecheck.mockResolvedValue({
      blocked: false,
      reasons: [],
      runningCount: 0,
      pendingAskCount: 0,
      runningQueueCount: 0,
      pausedQueueCount: 1,
    });
    downloadAndInstallUpdate.mockResolvedValue(undefined);
    fireEvent.click(screen.getByText("下载并安装"));
    const ready = await screen.findByTestId("update-ready");
    expect(ready.textContent).toContain("重启");
    fireEvent.click(screen.getByText("重启应用"));
    await waitFor(() => expect(relaunchForUpdate).toHaveBeenCalledTimes(1));
  });

  it("安装中禁止再次点击（进度态按钮禁用）", async () => {
    await openAvailableDialog();
    updatePrecheck.mockResolvedValue({
      blocked: false,
      reasons: [],
      runningCount: 0,
      pendingAskCount: 0,
      runningQueueCount: 0,
      pausedQueueCount: 0,
    });
    // 挂起中的安装：Promise 永不 resolve（模拟下载进行中）
    downloadAndInstallUpdate.mockReturnValue(new Promise(() => {}));
    fireEvent.click(screen.getByText("下载并安装"));
    await waitFor(() =>
      expect(screen.getByTestId("update-progress").textContent).toContain("下载中"),
    );
    const installButton = screen.getByTestId("update-install-button") as HTMLButtonElement;
    expect(installButton.disabled).toBe(true);
  });
});
