// @vitest-environment happy-dom
/**
 * SettingsPage 布局与离开拦截测试（照抄 CodeWave——交互对标项目——的设置页交互）：
 * 三组 10 页导航、切页标题联动、脏改动三选弹框，及 10 个设置页各留 1 个代表例
 * （通知引导按授权状态循环、审批/白名单/热键/日志/数据目录/网络/agent 关键交互）。
 * IPC 经 vi.mock 替换为 fake（不依赖真实后端）。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
const listLogFilesMock = vi.fn<() => Promise<string[]>>();
const openLogsDirMock = vi.fn<() => Promise<void>>();
const readSessionLogMock =
  vi.fn<
    (args: { sessionId: string; tail?: number }) => Promise<{
      lines: string[];
    }>
  >();
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
    listLogFiles: () => listLogFilesMock(),
    openLogsDir: () => openLogsDirMock(),
    readSessionLog: (args: { sessionId: string; tail?: number }) => readSessionLogMock(args),
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
  update: { enabled: true, intervalHours: 24 },
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
  // openDir 默认成功（三入口用例按需覆盖为 reject）
  openDirMock.mockResolvedValue(undefined);
  // 32 号票日志三命令：缺省空清单/成功打开/空行（查看器用例按需覆盖）
  listLogFilesMock.mockReset();
  openLogsDirMock.mockReset();
  readSessionLogMock.mockReset();
  listLogFilesMock.mockResolvedValue([]);
  openLogsDirMock.mockResolvedValue(undefined);
  readSessionLogMock.mockResolvedValue({ lines: [] });
  useSettingsStore.setState({
    config: structuredClone(BASE_CONFIG),
    baseline: structuredClone(BASE_CONFIG),
    loading: false,
    saving: false,
    error: null,
  });
  // 会话域复位：无活跃 Tab / 无项目（项目入口用例按需造数据）
  useSessionsStore.setState({ tabs: [], activeId: null, projects: [] });
  // 29 号票：保存成功出 Toast
  useToastStore.setState({ toasts: [] });
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

/** 切到安全与审批页。 */
async function openSecurityPage() {
  render(<SettingsPage onClose={() => {}} />);
  await waitFor(() => expect(screen.getByRole("tab", { name: "安全与审批" })).toBeTruthy());
  fireEvent.click(screen.getByRole("tab", { name: "安全与审批" }));
  await waitFor(() => expect(screen.getByText("审批策略")).toBeTruthy());
}

/** 造一份项目注册表条目（homeDir 指向虚拟主目录）。 */
function projectFixture(id: string, homeDir: string): ProjectMeta {
  return { id, name: `项目-${id}`, homeDir, createdAt: 0 };
}

describe("SettingsPage 布局与离开拦截", () => {
  it("渲染三组 10 页导航与默认页标题；切页联动操作条标题", async () => {
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
    fireEvent.click(screen.getByRole("tab", { name: "日志" }));
    expect(screen.getByText("日志", { selector: ".settings-actions-title" })).toBeTruthy();
  });

  it("脏改动三选：切页弹框可放弃跳转；返回工作区可留在原地", async () => {
    const first = render(<SettingsPage onClose={() => {}} />);
    await waitFor(() => expect(screen.getByRole("tab", { name: "日志" })).toBeTruthy());
    await makeDirty();
    fireEvent.click(screen.getByRole("tab", { name: "日志" }));
    expect(screen.getByText("有未保存的设置改动")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "放弃改动" }));
    await waitFor(() =>
      expect(screen.getByText("日志", { selector: ".settings-actions-title" })).toBeTruthy(),
    );
    expect(screen.queryByText("有未保存的改动")).toBeNull();
    first.unmount();

    const onClose = vi.fn();
    const second = render(<SettingsPage onClose={onClose} />);
    await waitFor(() => expect(screen.getByRole("button", { name: "返回工作区" })).toBeTruthy());
    await makeDirty();
    fireEvent.click(screen.getByRole("button", { name: "返回工作区" }));
    expect(screen.getByText("有未保存的设置改动")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "留在原地" }));
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.queryByText("有未保存的设置改动")).toBeNull();
    second.unmount();
  });
});

describe("通知授权引导（19 号票）", () => {
  it("按授权状态出引导与打开系统设置入口（prompt/denied/granted/无入口）", async () => {
    const cases: {
      label: string;
      status: NotificationPermissionStatus;
      check: () => void | Promise<void>;
    }[] = [
      {
        label: "prompt：显示引导，点击走深链",
        status: { status: "prompt", hasSettingsEntry: true },
        check: async () => {
          await waitFor(() =>
            expect(screen.getByTestId("open-notification-settings")).toBeTruthy(),
          );
          expect(screen.getByText(/尚未授予通知权限/)).toBeTruthy();
          fireEvent.click(screen.getByTestId("open-notification-settings"));
          await waitFor(() => expect(openSettingsMock).toHaveBeenCalledTimes(1));
        },
      },
      {
        label: "denied：显示被拒文案",
        status: { status: "denied", hasSettingsEntry: true },
        check: async () => {
          await waitFor(() => expect(screen.getByText(/已被拒绝/)).toBeTruthy());
          expect(screen.getByTestId("open-notification-settings")).toBeTruthy();
        },
      },
      {
        label: "granted：不显示引导，不打扰",
        status: { status: "granted", hasSettingsEntry: true },
        check: async () => {
          await waitFor(() => expect(permissionStatusMock).toHaveBeenCalled());
          expect(screen.queryByTestId("open-notification-settings")).toBeNull();
        },
      },
      {
        label: "prompt 无系统设置入口（Linux）：不出按钮",
        status: { status: "prompt", hasSettingsEntry: false },
        check: async () => {
          await waitFor(() => expect(screen.getByText(/尚未授予通知权限/)).toBeTruthy());
          expect(screen.queryByTestId("open-notification-settings")).toBeNull();
        },
      },
    ];
    for (const { status, check } of cases) {
      permissionStatusMock.mockResolvedValue(status);
      const view = render(<SettingsPage onClose={() => {}} />);
      await check();
      view.unmount();
    }
  });
});

describe("界面页（29 号票 §3.2）", () => {
  it("字号即时预览；保存进入配置，取消关闭后回到基线", async () => {
    const close = vi.fn();
    const view = render(<SettingsPage onClose={close} />);
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
    view.unmount();
    expect(document.documentElement.style.getPropertyValue("--ui-font-size")).toBe("18px");
  });

  it("关闭行为开关保存载荷带 closeToTray=false 且成功出 Toast；快捷键录制归一化随保存提交", async () => {
    // 关闭行为开关 → 保存（29 号票反馈收敛：成功出 Toast）
    saveConfigMock.mockResolvedValue(structuredClone(BASE_CONFIG));
    const first = render(<SettingsPage initialPage="appearance" onClose={() => {}} />);
    const closeSwitch = await screen.findByTestId("close-to-tray-switch");
    fireEvent.click(closeSwitch);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalledTimes(1));
    const payload = saveConfigMock.mock.calls[0]?.[0];
    expect(payload?.ui.closeToTray).toBe(false);
    expect(payload?.ui.hotkey).toEqual({ enabled: true, accelerator: "alt+space" });
    await waitFor(() =>
      expect(useToastStore.getState().toasts.some((t) => t.title === "设置已保存")).toBe(true),
    );
    first.unmount();

    // 快捷键录制：点击进入录制态，按下 Ctrl+Shift+P 归一化写回草稿并随保存提交
    saveConfigMock.mockClear();
    saveConfigMock.mockResolvedValue(structuredClone(BASE_CONFIG));
    const second = render(<SettingsPage initialPage="appearance" onClose={() => {}} />);
    const recorder = await screen.findByTestId("hotkey-recorder");
    expect(recorder.textContent).toContain("Alt+Space");
    fireEvent.click(recorder);
    // 录制态：打标记（设置页 Esc handler 让位依据）+ 提示文案
    expect(recorder.getAttribute("data-hotkey-recording")).toBe("true");
    expect(recorder.textContent).toContain("按下组合键");
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
    second.unmount();
  });
});

describe("数据目录入口（03 号票 open_dir 三按钮）", () => {
  it("关于页 ~/ 语义路径、日志页免路径、工作区页 homeDir（无项目禁用）；失败行内提示", async () => {
    // 关于页「打开数据目录」按 ~/ 语义路径调 open_dir；失败 → 行内提示不打断
    const about = render(<SettingsPage initialPage="about" onClose={() => {}} />);
    const button = await screen.findByTestId("open-data-dir");
    fireEvent.click(button);
    await waitFor(() => expect(openDirMock).toHaveBeenCalledWith("~/.craftwave"));
    openDirMock.mockRejectedValue({ code: "E_PATH_OUTSIDE_ROOTS", message: "路径越界" });
    fireEvent.click(button);
    await waitFor(() => expect(screen.getByText(/E_PATH_OUTSIDE_ROOTS/)).toBeTruthy());
    about.unmount();
    openDirMock.mockResolvedValue(undefined);

    // 日志页「打开日志目录」走 open_logs_dir（32 号票：免路径传参，后端固定 ~/.craftwave/logs）
    const logs = render(<SettingsPage initialPage="logs" onClose={() => {}} />);
    const logsButton = await screen.findByTestId("open-logs-dir");
    fireEvent.click(logsButton);
    await waitFor(() => expect(openLogsDirMock).toHaveBeenCalledWith());
    logs.unmount();

    // 工作区与智能体页：项目会话按钮携带 <homeDir>/.craftwave；临时会话禁用
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
    const agent = render(<SettingsPage initialPage="agent" onClose={() => {}} />);
    const projectButton = await screen.findByTestId("open-project-data-dir");
    expect(projectButton.getAttribute("disabled")).toBeNull();
    fireEvent.click(projectButton);
    await waitFor(() =>
      expect(openDirMock).toHaveBeenCalledWith("/Users/demo/Code/Demo/.craftwave"),
    );
    // 切到临时会话（无项目）：按钮禁用，点击不发起调用
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
    // 禁用态点击不发起调用（此前 about 段 2 次 + agent 段项目会话 1 次 = 3 次）
    expect(openDirMock).toHaveBeenCalledTimes(3);
    agent.unmount();
  });
});

describe("工作区与智能体页（03/10/37 号票）", () => {
  it("自定义提示词/子代理并发/停滞超时编辑进草稿并随保存提交", async () => {
    const view = render(<SettingsPage initialPage="agent" onClose={() => {}} />);
    const prompt = await screen.findByTestId("custom-prompt-input");
    fireEvent.change(prompt, { target: { value: "始终用中文回复" } });
    fireEvent.change(screen.getByTestId("subagent-concurrency-input"), {
      target: { value: "6" },
    });
    fireEvent.change(screen.getByTestId("watchdog-stall-timeout-input"), {
      target: { value: "600" },
    });
    // 草稿值（agent 页脏点出现 = pageSlice 切片生效）
    expect(useSettingsStore.getState().config?.agent?.customPrompt).toBe("始终用中文回复");
    expect(useSettingsStore.getState().config?.agent?.maxConcurrentSubagents).toBe(6);
    expect(useSettingsStore.getState().config?.watchdog.stallTimeoutSecs).toBe(600);
    await waitFor(() => expect(screen.getAllByTitle("有未保存的改动").length).toBeGreaterThan(0));
    saveConfigMock.mockImplementation(async (config) => structuredClone(config));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    const payload = saveConfigMock.mock.calls[0]![0];
    expect(payload.agent).toEqual({ customPrompt: "始终用中文回复", maxConcurrentSubagents: 6 });
    expect(payload.watchdog).toEqual({ stallTimeoutSecs: 600 });
    view.unmount();
  });
});

describe("网络搜索（13 号票，C-12 三通道配置面）", () => {
  it("SearXNG 开关/地址进草稿；连通测试走草稿值且不经保存；保存载荷携带 search 组", async () => {
    const view = render(<SettingsPage initialPage="network" onClose={() => {}} />);
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
    view.unmount();
  });
});

describe("安全与审批页（23/25/26 号票）", () => {
  it("审批四开关渲染；严格计划/git 写策略/记忆/白名单切换写草稿", async () => {
    await openSecurityPage();
    // 审批四开关（FR-14-04 字段级）
    expect(screen.getByLabelText("启用操作审批")).toBeTruthy();
    expect(screen.getByLabelText("自动放行等待")).toBeTruthy();
    expect(screen.getByLabelText("会话内记忆")).toBeTruthy();
    expect(screen.getByLabelText("命令白名单")).toBeTruthy();
    // 严格计划模式开关（25 号票 FR-06-02）：默认关（默认有计划才守门）
    const requirePlan = screen.getByTestId("require-plan-switch");
    expect(requirePlan.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(requirePlan);
    expect(useSettingsStore.getState().config?.approval.requirePlan).toBe(true);
    // git 写策略开关（26 号票 FR-11-03）：默认关（AI 默认禁止 stage/commit）
    const gitSwitch = screen.getByTestId("git-allow-stage-commit-switch");
    expect(gitSwitch.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(gitSwitch);
    expect(useSettingsStore.getState().config?.approval.gitAllowStageCommit).toBe(true);
    // 记忆/白名单关断写草稿
    fireEvent.click(screen.getByLabelText("会话内记忆"));
    fireEvent.click(screen.getByLabelText("命令白名单"));
    const draft = useSettingsStore.getState().config?.approval;
    expect(draft?.sessionMemoryEnabled).toBe(false);
    expect(draft?.allowlistEnabled).toBe(false);
    expect(draft?.enabled).toBe(true);
  });

  it("白名单手填添加与删除；空条目与含 shell 符号条目拒绝", async () => {
    await openSecurityPage();
    const input = screen.getByTestId("allowlist-input");
    fireEvent.change(input, { target: { value: "git status" } });
    fireEvent.click(screen.getByTestId("allowlist-add"));
    let list = useSettingsStore.getState().config?.approval.commandAllowlist ?? [];
    expect(list).toEqual(["git status"]);
    // 空条目拒绝
    fireEvent.click(screen.getByTestId("allowlist-add"));
    expect(screen.getByTestId("allowlist-hint").textContent).toContain("不能为空");
    // 含管道/分隔符拒绝
    fireEvent.change(input, { target: { value: "cat a | grep b" } });
    fireEvent.click(screen.getByTestId("allowlist-add"));
    expect(screen.getByTestId("allowlist-hint").textContent).toContain("无法作为前缀命中");
    // 合法条目提示消失；再加一条并删除第一条
    fireEvent.change(input, { target: { value: "cargo test" } });
    fireEvent.click(screen.getByTestId("allowlist-add"));
    expect(screen.queryByTestId("allowlist-hint")).toBeNull();
    list = useSettingsStore.getState().config?.approval.commandAllowlist ?? [];
    expect(list).toEqual(["git status", "cargo test"]);
    fireEvent.click(screen.getAllByText("删除")[0]);
    list = useSettingsStore.getState().config?.approval.commandAllowlist ?? [];
    expect(list).toEqual(["cargo test"]);
  });
});

describe("会话日志（32 号票）", () => {
  it("级别写进草稿并随保存提交（旁注 verbose 说明）；查看器清单过滤/Tab 标题映射/拉尾部渲染", async () => {
    // 级别下拉写草稿 + 保存
    saveConfigMock.mockImplementation(async (config) => structuredClone(config));
    const first = render(<SettingsPage initialPage="logs" onClose={() => {}} />);
    // 日志组里唯一的 select 是级别下拉
    const levelSelect = await screen
      .findAllByRole("combobox")
      .then((all) => all.find((el) => (el as HTMLSelectElement).value === "info"));
    expect(levelSelect).toBeTruthy();
    fireEvent.change(levelSelect as HTMLSelectElement, { target: { value: "debug" } });
    expect(useSettingsStore.getState().config?.log.level).toBe("debug");
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    expect(saveConfigMock.mock.calls.at(-1)?.[0].log).toEqual({ level: "debug" });
    // 旁注：Debug=详细模式说明（图片仅路径/摘要、已脱敏）
    expect(screen.getByText(/Debug=详细模式/).textContent).toContain("不含图片本体");
    first.unmount();

    // 查看器：清单过滤 sessions/ 前缀、活跃 Tab 标题映射、拉尾部渲染只读日志面
    useSessionsStore.setState({
      tabs: [
        {
          id: "s1",
          title: "修登录页的会话",
          createdAt: 0,
          updatedAt: 0,
          revision: 0,
          messageCount: 1,
          totalTokens: 0,
          authorizedExternalDirs: [],
        },
      ],
    });
    listLogFilesMock.mockResolvedValue(["app.log", "sessions/s1.jsonl", "sessions/s2.jsonl.1"]);
    readSessionLogMock.mockResolvedValue({
      lines: [
        '{"ts":"2026-10-30T08:00:00Z","level":"info","kind":"run","msg":"run 开始"}',
        '{"ts":"2026-10-30T08:00:01Z","level":"error","kind":"tool","msg":"工具执行失败：文件不存在"}',
        "这行是坏行（非 JSON）",
      ],
    });
    const second = render(<SettingsPage initialPage="logs" onClose={() => {}} />);
    await waitFor(() => expect(listLogFilesMock).toHaveBeenCalled());
    const picker = await screen.findByTestId("log-session-select");
    const options = Array.from(picker.querySelectorAll("option")).map((o) => o.textContent);
    expect(options).toContain("修登录页的会话");
    expect(options.join()).toContain("s2");
    expect(options.join()).not.toContain("app.log");
    // 选择 s1 → 拉尾部 500 行 → 只读面渲染三行（坏行原样）
    fireEvent.change(picker, { target: { value: "s1" } });
    await waitFor(() =>
      expect(readSessionLogMock).toHaveBeenCalledWith({ sessionId: "s1", tail: 500 }),
    );
    const linesBox = await screen.findByTestId("log-viewer-lines");
    expect(linesBox.textContent).toContain("run 开始");
    expect(linesBox.textContent).toContain("工具执行失败：文件不存在");
    expect(linesBox.textContent).toContain("这行是坏行（非 JSON）");
    second.unmount();
  });
});
