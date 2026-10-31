// @vitest-environment happy-dom
/**
 * 设置页「检查更新」区组件测试（33 号票）：三态渲染（有新版 Dialog / 已最新
 * Toast / 失败 Toast）、禁用兜底门（enabled=false / pubkey 未配）、安装门前置
 * （updatePrecheck 阻止原因展示与放行路径）、重启按钮链。
 * 精简版：门与三态循环断言；安装门阻止/放行合并。
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

describe("UpdateSection（33 号票）", () => {
  it("禁用兜底门：配置关闭 / pubkey 未实配时按钮禁用并示原因；门全开可用", async () => {
    const cases: { label: string; seed?: () => void; enabled: boolean; hint: string }[] = [
      {
        label: "update.enabled=false → 显示配置关闭原因",
        seed: () => getConfig.mockResolvedValue({ update: { enabled: false, intervalHours: 24 } }),
        enabled: false,
        hint: "关闭",
      },
      {
        label: "pubkey 未实配（占位空串）→ 显示密钥未配原因",
        seed: () => {
          // 复位第一行覆盖过的配置开关（循环内不经过 beforeEach）
          getConfig.mockResolvedValue({ update: { enabled: true, intervalHours: 24 } });
          updateStatus.mockResolvedValue({ pubkeyConfigured: false, endpoints: [] });
        },
        enabled: false,
        hint: "公钥",
      },
      {
        label: "门全开 → 按钮可用",
        enabled: true,
        hint: "",
      },
    ];
    for (const { seed, enabled, hint } of cases) {
      seed?.();
      const view = renderSection();
      await waitFor(() =>
        expect((screen.getByTestId("update-check-button") as HTMLButtonElement).disabled).toBe(
          !enabled,
        ),
      );
      if (hint) {
        expect(screen.getByTestId("update-section").textContent).toContain(hint);
      } else {
        expect(checkForUpdate).not.toHaveBeenCalled();
      }
      view.unmount();
    }
  });

  it("检查三态：已最新 Toast / 失败 Toast 带原始消息 / 有新版 Dialog 展示版本号与 notes", async () => {
    const cases: {
      label: string;
      seed: () => void;
      check: () => void | Promise<void>;
    }[] = [
      {
        label: "已最新 → Toast 轻提示，不弹 Dialog",
        seed: () => checkForUpdate.mockResolvedValue({ kind: "upToDate" }),
        check: async () => {
          await waitFor(() => expect(pushToast).toHaveBeenCalledWith("info", "当前已是最新版本。"));
          expect(screen.queryByTestId("update-dialog-body")).toBeNull();
        },
      },
      {
        label: "失败 → Toast 错误提示带原始消息",
        seed: () => checkForUpdate.mockResolvedValue({ kind: "failed", message: "网络断开" }),
        check: async () => {
          await waitFor(() =>
            expect(pushToast).toHaveBeenCalledWith("error", "检查更新失败", "网络断开"),
          );
        },
      },
      {
        label: "有新版 → Dialog 展示新版本号与 release notes",
        seed: () =>
          checkForUpdate.mockResolvedValue({
            kind: "available",
            version: "0.2.0",
            currentVersion: "0.1.0",
            notes: "修复了若干问题",
          }),
        check: async () => {
          const body = await screen.findByTestId("update-dialog-body");
          expect(body.textContent).toContain("0.2.0");
          expect(body.textContent).toContain("修复了若干问题");
        },
      },
    ];
    for (const { seed, check } of cases) {
      seed();
      const view = renderSection();
      const button = await screen.findByTestId("update-check-button");
      await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
      fireEvent.click(button);
      await check();
      view.unmount();
    }
  });

  it("安装门（2026-10-29 用户决策）：precheck 阻止原因逐行展示不下载；放行链到重启", async () => {
    /** 打开「有新版」Dialog 的公共前置。 */
    async function openAvailableDialog() {
      checkForUpdate.mockResolvedValue({
        kind: "available",
        version: "0.2.0",
        currentVersion: "0.1.0",
        notes: undefined,
      });
      const view = renderSection();
      fireEvent.click(await screen.findByTestId("update-check-button"));
      await screen.findByTestId("update-dialog-body");
      return view;
    }

    // 阻止：原因逐行展示，不启动下载
    const blocked = await openAvailableDialog();
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
    blocked.unmount();

    // 放行：下载安装完成 → 「重启以完成更新」→ relaunch
    const allowed = await openAvailableDialog();
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
    allowed.unmount();
  });
});
