// @vitest-environment happy-dom
/**
 * SettingsPage 布局与离开拦截测试（照抄 CodeWave——交互对标项目——的设置页交互）：
 * 三组 10 页导航、切页标题联动、脏改动三选弹框、搜索态替掉导航列表。
 * IPC 经 vi.mock 替换为 fake（不依赖真实后端）。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig, NotificationPermissionStatus, ProjectMeta } from "@/ipc/types";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { SettingsPage } from "./SettingsPage";

// IPC 测试桩（vi.fn 可按用例指定返回值）
const getConfigMock = vi.fn<() => Promise<AppConfig>>();
const saveConfigMock = vi.fn<(config: AppConfig) => Promise<AppConfig>>();
const permissionStatusMock = vi.fn<() => Promise<NotificationPermissionStatus>>();
const openSettingsMock = vi.fn<() => Promise<void>>();
const openDirMock = vi.fn<(path: string) => Promise<void>>();
// 只替换读/存配置、通知引导与 open_dir，其余保留 client.ts 真实现
vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    getConfig: () => getConfigMock(),
    saveConfig: (config: AppConfig) => saveConfigMock(config),
    notificationPermissionStatus: () => permissionStatusMock(),
    openNotificationSettings: () => openSettingsMock(),
    openDir: (path: string) => openDirMock(path),
  };
});

/** 基准配置样例：草稿与基线都从这里深拷贝，保证两份独立。 */
const BASE_CONFIG: AppConfig = {
  schemaVersion: 2,
  providers: [],
  approval: { enabled: true, autoConfirm: false, commandAllowlist: [] },
  ui: { language: "zh-CN", theme: "system" },
  log: { level: "info" },
  notifications: { enabled: true, interventionOnly: false },
  compaction: { threshold: 0.6, timeoutSecs: 180 },
  tools: { maxParallel: 4 },
  watchdog: { stallTimeoutSecs: 300 },
};

// 每个用例前：重置 mock、把 store 置为"草稿=基线"的干净状态（不脏）
beforeEach(() => {
  getConfigMock.mockReset();
  saveConfigMock.mockReset();
  permissionStatusMock.mockReset();
  openSettingsMock.mockReset();
  openDirMock.mockReset();
  getConfigMock.mockResolvedValue(structuredClone(BASE_CONFIG));
  // 授权态默认已授予：不影响既有用例的断言面（引导条只在未授权时出现）
  permissionStatusMock.mockResolvedValue({ status: "granted", hasSettingsEntry: true });
  // openDir 默认成功（三入口接线用例按需覆盖为 reject）
  openDirMock.mockResolvedValue(undefined);
  useSettingsStore.setState({
    config: structuredClone(BASE_CONFIG),
    baseline: structuredClone(BASE_CONFIG),
    loading: false,
    saving: false,
    error: null,
  });
  // 会话域复位：无活跃 Tab / 无项目（项目入口用例按需造数据）
  useSessionsStore.setState({ tabs: [], activeId: null, projects: [] });
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

describe("通知授权引导（19 号票）", () => {
  it("未授权（prompt）时通知区显示引导与打开系统设置入口，点击走深链", async () => {
    permissionStatusMock.mockResolvedValue({ status: "prompt", hasSettingsEntry: true });
    render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(screen.getByTestId("open-notification-settings")).toBeTruthy());
    expect(screen.getByText(/尚未授予通知权限/)).toBeTruthy();
    fireEvent.click(screen.getByTestId("open-notification-settings"));
    await waitFor(() => expect(openSettingsMock).toHaveBeenCalledTimes(1));
  });

  it("被拒（denied）时显示被拒文案", async () => {
    permissionStatusMock.mockResolvedValue({ status: "denied", hasSettingsEntry: true });
    render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText(/已被拒绝/)).toBeTruthy());
    expect(screen.getByTestId("open-notification-settings")).toBeTruthy();
  });

  it("已授权与查不出（unknown）时不显示引导，不打扰", async () => {
    permissionStatusMock.mockResolvedValue({ status: "granted", hasSettingsEntry: true });
    const { unmount } = render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(permissionStatusMock).toHaveBeenCalled());
    expect(screen.queryByTestId("open-notification-settings")).toBeNull();
    expect(screen.queryByText(/尚未授予通知权限/)).toBeNull();
    unmount();

    permissionStatusMock.mockResolvedValue({ status: "unknown", hasSettingsEntry: true });
    render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(permissionStatusMock).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId("open-notification-settings")).toBeNull();
  });

  it("无系统设置入口（Linux）时引导条不出按钮", async () => {
    permissionStatusMock.mockResolvedValue({ status: "prompt", hasSettingsEntry: false });
    render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText(/尚未授予通知权限/)).toBeTruthy());
    expect(screen.queryByTestId("open-notification-settings")).toBeNull();
  });
});

describe("字号与错误恢复入口", () => {
  it("错误恢复可直接打开供应商页", () => {
    render(<SettingsPage initialPage="providers" onClose={() => {}} />);
    expect(screen.getByRole("tab", { name: "模型与供应商" }).getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("字号即时预览；保存进入配置，取消关闭后回到基线", async () => {
    const close = vi.fn();
    const { unmount } = render(<SettingsPage onClose={close} />);
    const slider = await screen.findByRole("slider", { name: "界面字号" });
    fireEvent.change(slider, { target: { value: "18" } });
    await waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--ui-font-size")).toBe("18px"),
    );
    expect(useSettingsStore.getState().config?.ui.fontSize).toBe(18);
    saveConfigMock.mockImplementation(async (config) => structuredClone(config));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    expect(saveConfigMock.mock.calls[0]?.[0].ui.fontSize).toBe(18);
    await waitFor(() => expect(useSettingsStore.getState().baseline?.ui.fontSize).toBe(18));
    getConfigMock.mockResolvedValue(structuredClone(useSettingsStore.getState().baseline!));
    fireEvent.change(slider, { target: { value: "12" } });
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    unmount();
    expect(document.documentElement.style.getPropertyValue("--ui-font-size")).toBe("18px");
  });
});

/** 造一份项目注册表条目（homeDir 指向虚拟主目录）。 */
function projectFixture(id: string, homeDir: string): ProjectMeta {
  return { id, name: `项目-${id}`, homeDir, createdAt: 0 };
}

describe("数据目录入口（03 号票 open_dir 三按钮）", () => {
  it("关于页「打开数据目录」按 ~/ 语义路径调 open_dir；失败行内提示", async () => {
    render(<SettingsPage initialPage="about" onClose={() => {}} />);
    const button = await screen.findByTestId("open-data-dir");
    fireEvent.click(button);
    await waitFor(() => expect(openDirMock).toHaveBeenCalledWith("~/.craftwave"));
    // 失败 → 行内提示错误码与消息，不打断页面
    openDirMock.mockRejectedValue({ code: "E_PATH_OUTSIDE_ROOTS", message: "路径越界" });
    fireEvent.click(button);
    await waitFor(() => expect(screen.getByText(/E_PATH_OUTSIDE_ROOTS/)).toBeTruthy());
  });

  it("日志页「打开日志目录」按 ~/ 语义路径调 open_dir", async () => {
    render(<SettingsPage initialPage="logs" onClose={() => {}} />);
    const button = await screen.findByTestId("open-logs-dir");
    fireEvent.click(button);
    await waitFor(() => expect(openDirMock).toHaveBeenCalledWith("~/.craftwave/logs"));
  });

  it("工作区与智能体页：有项目会话时按钮携带 <homeDir>/.craftwave；无项目会话禁用", async () => {
    // 造活跃项目会话 Tab
    const project = projectFixture("p1", "/Users/demo/Code/Demo");
    useSessionsStore.setState({
      projects: [project],
      tabs: [
        {
          id: "s1",
          projectId: "p1",
          title: "会话",
          createdAt: 0,
          updatedAt: 0,
          revision: 0,
          messageCount: 1,
          totalTokens: 0,
        },
      ],
      activeId: "s1",
    });
    render(<SettingsPage initialPage="agent" onClose={() => {}} />);
    const button = await screen.findByTestId("open-project-data-dir");
    expect(button.getAttribute("disabled")).toBeNull();
    fireEvent.click(button);
    await waitFor(() =>
      expect(openDirMock).toHaveBeenCalledWith("/Users/demo/Code/Demo/.craftwave"),
    );

    // 切到临时会话（无项目）：按钮禁用并说明原因
    useSessionsStore.setState({
      tabs: [
        {
          id: "s2",
          projectId: undefined,
          title: "临时会话",
          createdAt: 0,
          updatedAt: 0,
          revision: 0,
          messageCount: 1,
          totalTokens: 0,
        },
      ],
      activeId: "s2",
    });
    const disabled = await screen.findByTestId("open-project-data-dir");
    expect(disabled.getAttribute("disabled")).not.toBeNull();
    fireEvent.click(disabled);
    // 禁用态点击不发起调用（仍只有有项目时那一次）
    expect(openDirMock).toHaveBeenCalledTimes(1);
  });
});

describe("自定义提示词（03 号票 agent.customPrompt）", () => {
  it("工作区与智能体页编辑进草稿，保存载荷含 agent 字段；清空后载荷为空串", async () => {
    const { unmount } = render(<SettingsPage initialPage="agent" onClose={() => {}} />);
    const input = await screen.findByTestId("custom-prompt-input");
    fireEvent.change(input, { target: { value: "始终用中文回复" } });
    expect(useSettingsStore.getState().config?.agent?.customPrompt).toBe("始终用中文回复");
    // agent 页脏点出现（pageSlice 切片生效）
    await waitFor(() => expect(screen.getAllByTitle("有未保存的改动").length).toBeGreaterThan(0));
    saveConfigMock.mockImplementation(async (config) => structuredClone(config));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    // 10 号票起 agent 组含 maxConcurrentSubagents（缺省 4 随草稿一起提交）
    expect(saveConfigMock.mock.calls[0]?.[0].agent).toEqual({
      customPrompt: "始终用中文回复",
      maxConcurrentSubagents: 4,
    });

    // 清空：草稿值置 undefined（等价空），提交载荷 customPrompt 为 undefined
    fireEvent.change(input, { target: { value: "" } });
    await waitFor(() =>
      expect(useSettingsStore.getState().config?.agent?.customPrompt).toBeUndefined(),
    );
    unmount();
  });
});

describe("子代理全局并发上限（10 号票，agent.maxConcurrentSubagents）", () => {
  it("工作区与智能体页数字输入编辑进草稿并随保存提交（1–16 提示，缺省 4）", async () => {
    const { unmount } = render(<SettingsPage initialPage="agent" onClose={() => {}} />);
    const input = await screen.findByTestId("subagent-concurrency-input");
    expect((input as HTMLInputElement).value).toBe("4");
    fireEvent.change(input, { target: { value: "6" } });
    expect(useSettingsStore.getState().config?.agent?.maxConcurrentSubagents).toBe(6);
    saveConfigMock.mockImplementation(async (config) => structuredClone(config));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    expect(saveConfigMock.mock.calls.at(-1)?.[0].agent).toMatchObject({
      maxConcurrentSubagents: 6,
    });
    unmount();
  });
});

describe("模型流停滞超时（37 号票 watchdog.stallTimeoutSecs）", () => {
  it("工作区与智能体页数字输入进草稿，保存载荷携带 watchdog 字段", async () => {
    render(<SettingsPage initialPage="agent" onClose={() => {}} />);
    const input = await screen.findByTestId("watchdog-stall-timeout-input");
    expect((input as HTMLInputElement).value).toBe("300");
    // 改为 600：草稿更新 + 脏点出现
    fireEvent.change(input, { target: { value: "600" } });
    expect(useSettingsStore.getState().config?.watchdog.stallTimeoutSecs).toBe(600);
    await waitFor(() => expect(screen.getAllByTitle("有未保存的改动").length).toBeGreaterThan(0));
    saveConfigMock.mockImplementation(async (config) => structuredClone(config));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    expect(saveConfigMock.mock.calls[0]?.[0].watchdog).toEqual({ stallTimeoutSecs: 600 });
  });
});
