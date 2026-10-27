// @vitest-environment happy-dom
/**
 * SettingsPage 布局与离开拦截测试（照抄 CodeWave——交互对标项目——的设置页交互）：
 * 三组 10 页导航、切页标题联动、脏改动三选弹框、搜索态替掉导航列表。
 * IPC 经 vi.mock 替换为 fake（不依赖真实后端）。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig } from "@/ipc/types";
import { useSettingsStore } from "@/stores/settings";
import { SettingsPage } from "./SettingsPage";

// IPC 测试桩（vi.fn 可按用例指定返回值）
const getConfigMock = vi.fn<() => Promise<AppConfig>>();
const saveConfigMock = vi.fn<(config: AppConfig) => Promise<AppConfig>>();
// 只替换读/存配置两个命令，其余保留 client.ts 真实现
vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    getConfig: () => getConfigMock(),
    saveConfig: (config: AppConfig) => saveConfigMock(config),
  };
});

/** 基准配置样例：草稿与基线都从这里深拷贝，保证两份独立。 */
const BASE_CONFIG: AppConfig = {
  schemaVersion: 2,
  providers: [],
  approval: { enabled: true, autoConfirm: false, commandAllowlist: [] },
  ui: { language: "zh-CN", theme: "system" },
  log: { level: "info" },
};

// 每个用例前：重置 mock、把 store 置为"草稿=基线"的干净状态（不脏）
beforeEach(() => {
  getConfigMock.mockReset();
  saveConfigMock.mockReset();
  getConfigMock.mockResolvedValue(structuredClone(BASE_CONFIG));
  useSettingsStore.setState({
    config: structuredClone(BASE_CONFIG),
    baseline: structuredClone(BASE_CONFIG),
    loading: false,
    saving: false,
    error: null,
  });
});
// 每个用例后卸载渲染出的组件，避免 DOM 残留影响下一个用例
afterEach(cleanup);

/** 把 store 草稿改脏（非供应商字段）并等脏点渲染：click 闭包里的 anyDirty 才是新值 */
async function makeDirty() {
  useSettingsStore.setState((s) => ({
    config: s.config ? { ...s.config, log: { level: "debug" } } : s.config,
  }));
  await waitFor(() => expect(screen.getAllByTitle("有未保存的改动").length).toBeGreaterThan(0));
}

describe("SettingsPage 布局", () => {
  it("渲染三组 10 页导航与默认页标题", async () => {
    render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(screen.getByRole("tab", { name: "界面" })).toBeTruthy());
    for (const label of [
      "界面",
      "模型与供应商",
      "网络与连接",
      "安全与审批",
      "MCP",
      "技能",
      "关于",
    ]) {
      expect(screen.getByRole("tab", { name: new RegExp(label) })).toBeTruthy();
    }
    for (const group of ["外观与模型", "安全与能力", "诊断与其他"]) {
      expect(screen.getByText(group)).toBeTruthy();
    }
    expect(screen.getByText("界面", { selector: ".settings-actions-title" })).toBeTruthy();
  });

  it("切页联动操作条标题", async () => {
    render(<SettingsPage onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "日志" }));
    expect(screen.getByText("日志", { selector: ".settings-actions-title" })).toBeTruthy();
  });
});

describe("离开拦截三选", () => {
  it("无脏改动时切页直接生效，不弹三选", async () => {
    render(<SettingsPage onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "关于" }));
    expect(screen.queryByText("有未保存的设置改动")).toBeNull();
    expect(screen.getByText("关于", { selector: ".settings-actions-title" })).toBeTruthy();
  });

  it("脏改动切页弹三选；放弃改动后跳目标页且脏点消失", async () => {
    getConfigMock.mockResolvedValue(structuredClone(BASE_CONFIG));
    render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(screen.getByRole("tab", { name: "日志" })).toBeTruthy());
    await makeDirty();
    fireEvent.click(screen.getByRole("tab", { name: "日志" }));
    expect(screen.getByText("有未保存的设置改动")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "放弃改动" }));
    await waitFor(() =>
      expect(screen.getByText("日志", { selector: ".settings-actions-title" })).toBeTruthy(),
    );
    expect(screen.queryByText("有未保存的设置改动")).toBeNull();
    expect(screen.queryByText("有未保存的改动")).toBeNull();
  });

  it("脏改动返回工作区弹三选；留在原地不关闭", async () => {
    const onClose = vi.fn();
    render(<SettingsPage onClose={onClose} />);
    await waitFor(() => expect(screen.getByRole("button", { name: "返回工作区" })).toBeTruthy());
    await makeDirty();
    fireEvent.click(screen.getByRole("button", { name: "返回工作区" }));
    expect(screen.getByText("有未保存的设置改动")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "留在原地" }));
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.queryByText("有未保存的设置改动")).toBeNull();
  });
});

describe("搜索", () => {
  it("搜索态替掉导航列表，命中页可点击跳转", async () => {
    render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(screen.getByRole("tab", { name: "日志" })).toBeTruthy());
    fireEvent.change(screen.getByLabelText("搜索设置"), { target: { value: "日志" } });
    expect(screen.queryByRole("tab", { name: "界面" })).toBeNull();
    fireEvent.click(screen.getByRole("option", { name: /日志/ }));
    expect(screen.getByText("日志", { selector: ".settings-actions-title" })).toBeTruthy();
  });
});
