// @vitest-environment happy-dom
/**
 * SettingsPage 布局与离开拦截测试（照抄 CodeWave——交互对标项目——的设置页交互）：
 * 三组 10 页导航、切页标题联动、脏改动三选弹框、搜索态替掉导航列表。
 * IPC 经 vi.mock 替换为 fake（不依赖真实后端）。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useHotkeyStore } from "@/hotkey";
import type { AppConfig, NotificationPermissionStatus, ProjectMeta } from "@/ipc/types";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useToastStore } from "@/stores/toast";
import { SettingsPage } from "./SettingsPage";

// IPC 测试桩（vi.fn 可按用例指定返回值）
const getConfigMock = vi.fn<() => Promise<AppConfig>>();
const saveConfigMock = vi.fn<(config: AppConfig) => Promise<AppConfig>>();
const permissionStatusMock = vi.fn<() => Promise<NotificationPermissionStatus>>();
const openSettingsMock = vi.fn<() => Promise<void>>();
const openDirMock = vi.fn<(path: string) => Promise<void>>();
const searchTestMock = vi.fn<(args: { target: string }) => Promise<unknown>>();
// 只替换读/存配置、通知引导、open_dir 与搜索连通测试，其余保留 client.ts 真实现
vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    getConfig: () => getConfigMock(),
    saveConfig: (config: AppConfig) => saveConfigMock(config),
    notificationPermissionStatus: () => permissionStatusMock(),
    openNotificationSettings: () => openSettingsMock(),
    openDir: (path: string) => openDirMock(path),
    searchTest: (args: { target: string }) => searchTestMock(args),
  };
});

// 全局热键插件桩（29 号票：保存成功后 applyHotkey 会调插件注册——测试
// 环境无 Tauri，桩成永不失败的 no-op；注册流自身单测在 hotkey.test.ts）
vi.mock("@tauri-apps/plugin-global-shortcut", () => ({
  register: vi.fn(async () => undefined),
  unregisterAll: vi.fn(async () => undefined),
  isRegistered: vi.fn(async () => false),
}));

/** 基准配置样例：草稿与基线都从这里深拷贝，保证两份独立。 */
const BASE_CONFIG: AppConfig = {
  schemaVersion: 2,
  providers: [],
  approval: {
    enabled: true,
    autoConfirm: false,
    sessionMemoryEnabled: true,
    allowlistEnabled: true,
    commandAllowlist: [],
    requirePlan: false,
    gitAllowStageCommit: false,
  },
  // 界面组（29 号票 §3.2 ui 组补齐）：关闭收进托盘缺省开、热键缺省 alt+space
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
  search: {
    searxng: { enabled: false, baseUrl: "http://127.0.0.1:8888", maxResults: 10 },
  },
};

// 每个用例前：重置 mock、把 store 置为"草稿=基线"的干净状态（不脏）
beforeEach(() => {
  searchTestMock.mockReset();
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
  // 29 号票：反馈与热键状态复位（保存成功出 Toast / 热键错误红字）
  useToastStore.setState({ toasts: [] });
  useHotkeyStore.setState({ error: null });
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
          authorizedExternalDirs: [],
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
          authorizedExternalDirs: [],
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

describe("网络搜索（13 号票，C-12 三通道配置面）", () => {
  it("SearXNG 开关/地址编辑进草稿并随保存提交；连通测试按钮走草稿值且不经保存", async () => {
    const { unmount } = render(<SettingsPage initialPage="network" onClose={() => {}} />);
    const enabled = await screen.findByTestId("searxng-enabled-switch");
    const baseUrl = screen.getByTestId("searxng-base-url-input");
    expect((baseUrl as HTMLInputElement).value).toBe("http://127.0.0.1:8888");
    // 开启开关 + 改地址：进草稿
    fireEvent.click(enabled);
    fireEvent.change(baseUrl, { target: { value: "http://127.0.0.1:9999" } });
    expect(useSettingsStore.getState().config?.search.searxng.enabled).toBe(true);
    expect(useSettingsStore.getState().config?.search.searxng.baseUrl).toBe(
      "http://127.0.0.1:9999",
    );
    // 连通测试：按草稿值调 searchTest（不经保存），成功结果显示时延
    searchTestMock.mockResolvedValue({ ok: true, latencyMs: 42 });
    fireEvent.click(screen.getByTestId("searxng-test-button"));
    await waitFor(() => {
      expect(screen.getByTestId("searxng-test-result").textContent).toContain("连接成功");
    });
    expect(searchTestMock).toHaveBeenCalledWith({
      target: "searxng",
      searxngBaseUrl: "http://127.0.0.1:9999",
    });
    // 保存载荷携带 search 组（开关 + 地址）
    saveConfigMock.mockImplementation(async (config) => structuredClone(config));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    expect(saveConfigMock.mock.calls.at(-1)?.[0].search.searxng).toMatchObject({
      enabled: true,
      baseUrl: "http://127.0.0.1:9999",
    });
    unmount();
  });

  it("启用后显示 settings.yml format=json 引导；Tavily Key 编辑进草稿，连通测试失败显示引导文案", async () => {
    const { unmount } = render(<SettingsPage initialPage="network" onClose={() => {}} />);
    await screen.findByTestId("searxng-enabled-switch");
    // 开关开启后引导出现
    fireEvent.click(screen.getByTestId("searxng-enabled-switch"));
    expect(screen.getByTestId("searxng-format-hint").textContent).toContain("settings.yml");
    // Tavily Key 输入进草稿
    const keyInput = screen.getByTestId("tavily-api-key-input");
    fireEvent.change(keyInput, { target: { value: "tvly-test-key" } });
    expect(useSettingsStore.getState().config?.search.tavilyApiKey).toBe("tvly-test-key");
    // 连通测试失败：ok=false + 引导文案展示
    searchTestMock.mockResolvedValue({
      ok: false,
      error: "Tavily（Key 无效（401））：Invalid API key",
    });
    fireEvent.click(screen.getByTestId("tavily-test-button"));
    await waitFor(() => {
      expect(screen.getByTestId("tavily-test-result").textContent).toContain("401");
    });
    unmount();
  });
});

describe("安全与审批页（23 号票：四开关 + 白名单管理）", () => {
  /** 切到安全与审批页。 */
  async function openSecurityPage() {
    render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(screen.getByRole("tab", { name: "安全与审批" })).toBeTruthy());
    fireEvent.click(screen.getByRole("tab", { name: "安全与审批" }));
    await waitFor(() => expect(screen.getByText("审批策略")).toBeTruthy());
  }

  it("渲染审批四开关（FR-14-04 字段级）", async () => {
    await openSecurityPage();
    expect(screen.getByLabelText("启用操作审批")).toBeTruthy();
    expect(screen.getByLabelText("自动放行等待")).toBeTruthy();
    expect(screen.getByLabelText("会话内记忆")).toBeTruthy();
    expect(screen.getByLabelText("命令白名单")).toBeTruthy();
  });

  it("严格计划模式开关（25 号票 FR-06-02）：默认关、可切换写入草稿", async () => {
    await openSecurityPage();
    const toggle = screen.getByTestId("require-plan-switch");
    // 默认关（默认有计划才守门）
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    const draft = useSettingsStore.getState().config?.approval;
    expect(draft?.requirePlan).toBe(true);
    expect(toggle.getAttribute("aria-checked")).toBe("true");
  });

  it("git 写策略开关（26 号票 FR-11-03）：默认关、可切换写入草稿", async () => {
    await openSecurityPage();
    const toggle = screen.getByTestId("git-allow-stage-commit-switch");
    // 默认关：AI 默认禁止 stage/commit（版本历史只能由用户推动）
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    const draft = useSettingsStore.getState().config?.approval;
    expect(draft?.gitAllowStageCommit).toBe(true);
    expect(toggle.getAttribute("aria-checked")).toBe("true");
  });

  it("四开关逐项可切换并写入草稿", async () => {
    await openSecurityPage();
    fireEvent.click(screen.getByLabelText("会话内记忆"));
    fireEvent.click(screen.getByLabelText("命令白名单"));
    const draft = useSettingsStore.getState().config?.approval;
    expect(draft?.sessionMemoryEnabled).toBe(false);
    expect(draft?.allowlistEnabled).toBe(false);
    expect(draft?.enabled).toBe(true);
  });

  it("白名单手填添加与删除", async () => {
    await openSecurityPage();
    const input = screen.getByTestId("allowlist-input");
    fireEvent.change(input, { target: { value: "git status" } });
    fireEvent.click(screen.getByTestId("allowlist-add"));
    let list = useSettingsStore.getState().config?.approval.commandAllowlist ?? [];
    expect(list).toEqual(["git status"]);
    // 再加一条并删除第一条
    fireEvent.change(input, { target: { value: "cargo test" } });
    fireEvent.click(screen.getByTestId("allowlist-add"));
    list = useSettingsStore.getState().config?.approval.commandAllowlist ?? [];
    expect(list).toEqual(["git status", "cargo test"]);
    fireEvent.click(screen.getAllByText("删除")[0]);
    list = useSettingsStore.getState().config?.approval.commandAllowlist ?? [];
    expect(list).toEqual(["cargo test"]);
  });

  it("白名单词边界校验提示：空条目与含 shell 符号条目拒绝", async () => {
    await openSecurityPage();
    const input = screen.getByTestId("allowlist-input");
    // 空条目
    fireEvent.click(screen.getByTestId("allowlist-add"));
    expect(screen.getByTestId("allowlist-hint").textContent).toContain("不能为空");
    // 含管道/分隔符
    fireEvent.change(input, { target: { value: "cat a | grep b" } });
    fireEvent.click(screen.getByTestId("allowlist-add"));
    expect(screen.getByTestId("allowlist-hint").textContent).toContain("无法作为前缀命中");
    // 合法条目后提示消失
    fireEvent.change(input, { target: { value: "git status" } });
    fireEvent.click(screen.getByTestId("allowlist-add"));
    expect(screen.queryByTestId("allowlist-hint")).toBeNull();
  });
});

describe("命令审计模型（36 号票，agent.commandAudit.modelId）", () => {
  it("缺省显示「跟随会话模型」；选择模型写入草稿并随保存提交", async () => {
    // 造一个带模型的 provider 供下拉选择
    const config = structuredClone(BASE_CONFIG);
    config.providers = [
      {
        id: "prov-1",
        name: "Mock 供应商",
        apiFormat: "openai_chat",
        models: [
          { id: "model-1", model: "gpt-x", maxTokens: 32768, contextWindow: 128000 },
          { id: "model-2", model: "gpt-y", maxTokens: 32768, contextWindow: 128000 },
        ],
      },
    ];
    useSettingsStore.setState({ config, baseline: structuredClone(config) });

    const { unmount } = render(<SettingsPage initialPage="agent" onClose={() => {}} />);
    const select = await screen.findByTestId("command-audit-model-select");
    expect((select as HTMLSelectElement).value).toBe("");
    expect(select.textContent).toContain("跟随会话模型");

    // 选择专用审计模型 → 草稿写入 commandAudit.modelId
    fireEvent.change(select, { target: { value: "model-2" } });
    expect(useSettingsStore.getState().config?.agent?.commandAudit?.modelId).toBe("model-2");
    saveConfigMock.mockImplementation(async (c) => structuredClone(c));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    expect(saveConfigMock.mock.calls.at(-1)?.[0].agent?.commandAudit).toEqual({
      modelId: "model-2",
    });
    unmount();
  });

  it("已配置时回填显示；切回「跟随会话模型」清空 commandAudit", async () => {
    const config = structuredClone(BASE_CONFIG);
    config.providers = [
      {
        id: "prov-1",
        name: "Mock 供应商",
        apiFormat: "openai_chat",
        models: [{ id: "model-1", model: "gpt-x", maxTokens: 32768, contextWindow: 128000 }],
      },
    ];
    config.agent = { maxConcurrentSubagents: 4, commandAudit: { modelId: "model-1" } };
    useSettingsStore.setState({ config, baseline: structuredClone(config) });

    const { unmount } = render(<SettingsPage initialPage="agent" onClose={() => {}} />);
    const select = await screen.findByTestId("command-audit-model-select");
    expect((select as HTMLSelectElement).value).toBe("model-1");

    fireEvent.change(select, { target: { value: "" } });
    expect(useSettingsStore.getState().config?.agent?.commandAudit?.modelId).toBeUndefined();
    unmount();
  });
});

describe("窗口行为（29 号票：关闭按钮行为 + 全局快捷键）", () => {
  it("界面页渲染关闭行为开关与快捷键区；enabled 关时录制钮隐藏", async () => {
    render(<SettingsPage initialPage="appearance" onClose={() => {}} />);
    const closeSwitch = await screen.findByTestId("close-to-tray-switch");
    expect(
      closeSwitch.getAttribute("aria-checked") ?? closeSwitch.getAttribute("data-state"),
    ).toBeTruthy();
    const hotkeySwitch = screen.getByTestId("hotkey-enabled-switch");
    expect(hotkeySwitch).toBeTruthy();
    // 缺省开：录制钮显示当前组合键
    const recorder = screen.getByTestId("hotkey-recorder");
    expect(recorder.textContent).toContain("Alt+Space");
    // 关掉启用开关：录制钮隐藏
    fireEvent.click(hotkeySwitch);
    expect(screen.queryByTestId("hotkey-recorder")).toBeNull();
  });

  it("关闭行为开关切换写草稿；保存载荷带 closeToTray=false 且成功出 Toast", async () => {
    const saved = structuredClone(BASE_CONFIG);
    saveConfigMock.mockResolvedValue(saved);
    render(<SettingsPage initialPage="appearance" onClose={() => {}} />);
    const closeSwitch = await screen.findByTestId("close-to-tray-switch");
    fireEvent.click(closeSwitch);
    // 保存（操作条「保存」按钮）
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalledTimes(1));
    const payload = saveConfigMock.mock.calls[0]?.[0];
    expect(payload?.ui.closeToTray).toBe(false);
    expect(payload?.ui.hotkey).toEqual({ enabled: true, accelerator: "alt+space" });
    // 保存成功 Toast（29 号票反馈收敛）
    await waitFor(() =>
      expect(useToastStore.getState().toasts.some((t) => t.title === "设置已保存")).toBe(true),
    );
  });

  it("快捷键录制：点击进入录制态，按下 Ctrl+Shift+P 归一化写回草稿并随保存提交", async () => {
    const saved = structuredClone(BASE_CONFIG);
    saveConfigMock.mockResolvedValue(saved);
    render(<SettingsPage initialPage="appearance" onClose={() => {}} />);
    const recorder = await screen.findByTestId("hotkey-recorder");
    fireEvent.click(recorder);
    // 录制态：打标记（设置页 Esc handler 让位依据）+ 提示文案
    expect(recorder.getAttribute("data-hotkey-recording")).toBe("true");
    expect(recorder.textContent).toContain("按下组合键");
    // 按下组合键（修饰键顺序无关，归一化为固定序）
    fireEvent.keyDown(window, {
      key: "p",
      ctrlKey: true,
      shiftKey: true,
      cancelable: true,
    });
    await waitFor(() => expect(recorder.textContent).toContain("Ctrl+Shift+P"));
    expect(recorder.getAttribute("data-hotkey-recording")).toBe("false");
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalledTimes(1));
    expect(saveConfigMock.mock.calls[0]?.[0]?.ui.hotkey.accelerator).toBe("ctrl+shift+p");
  });

  it("录制态 Esc 取消：恢复显示原组合键，草稿不变", async () => {
    render(<SettingsPage initialPage="appearance" onClose={() => {}} />);
    const recorder = await screen.findByTestId("hotkey-recorder");
    fireEvent.click(recorder);
    expect(recorder.getAttribute("data-hotkey-recording")).toBe("true");
    fireEvent.keyDown(window, { key: "Escape", cancelable: true });
    await waitFor(() => expect(recorder.textContent).toContain("Alt+Space"));
    // 取消不产生脏改动（快捷键未被改写，Esc 也不会关掉设置页）
    expect(useSettingsStore.getState().config?.ui.hotkey.accelerator).toBe("alt+space");
  });

  it("注册失败红字：hotkey store 错误态在界面页显示被占用提示", async () => {
    useHotkeyStore.setState({ error: "shortcut already registered" });
    render(<SettingsPage initialPage="appearance" onClose={() => {}} />);
    const error = await screen.findByTestId("hotkey-error");
    expect(error.textContent).toContain("被其他应用占用");
  });
});
