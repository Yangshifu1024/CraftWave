// @vitest-environment happy-dom
/**
 * 首启引导向导测试（30 号票，FR-13-18）。
 *
 * mock 唯一 invoke 出口（@/ipc/client）与 sessions/settings 两个 store 的
 * 初始态，断言外部行为面：
 * 1. 四步导航（前进 / 单步跳过 / 整包关闭）与 step 记账（setOnboarding 落
 *    sessions store，经既有防抖通道落盘）；
 * 2. 语言步 save_config 写 config.ui.language；
 * 3. 供应商步 save_provider（newKey 通道）+ test_provider 三分支（连通 /
 *    探测失败「仍要保存」与「跳过」并存 / 命令未注册降级「稍后在设置中测试」）；
 * 4. 项目步 select_dir + save_project（名称留空取目录名兜底）；
 * 5. 末步「完成」→ completed=true 收口；「继续配置」横幅三态渲染。
 *
 * SessionWorkspace 的自动弹接线（恢复会话列表后 shouldAutoOpen 判定）依赖
 * assistant-ui 重资源树，不在本文件覆盖——判定纯函数在 onboarding-state
 * 测试钉住，真实首启弹层走用户 GUI 验收面（票内决议记录诚实声明）。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig, ProviderConfig, TestProviderResult } from "@/ipc/types";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useToastStore } from "@/stores/toast";
import { OnboardingResumeBanner, OnboardingWizard } from "./OnboardingWizard";

// ---- IPC 测试桩（vi.fn 可按用例指定返回值） ----
const getConfigMock = vi.fn<() => Promise<AppConfig>>();
const saveConfigMock = vi.fn<(config: AppConfig) => Promise<AppConfig>>();
const saveProviderMock =
  vi.fn<(provider: ProviderConfig, newKey?: string) => Promise<ProviderConfig>>();
const testProviderMock =
  vi.fn<(args: { providerId: string; baseUrlOverride?: string }) => Promise<TestProviderResult>>();
const saveProjectMock =
  vi.fn<(args: { project: { name: string; homeDir: string } }) => Promise<unknown>>();
const selectDirMock = vi.fn<() => Promise<string | null>>();
const listProjectsMock = vi.fn<() => Promise<never[]>>();
const listSessionsMock = vi.fn<() => Promise<never[]>>();
const setUiStateMock = vi.fn<(state: unknown) => Promise<void>>();

vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    getConfig: () => getConfigMock(),
    saveConfig: (config: AppConfig) => saveConfigMock(config),
    saveProvider: (provider: ProviderConfig, newKey?: string) => saveProviderMock(provider, newKey),
    testProvider: (args: { providerId: string; baseUrlOverride?: string }) =>
      testProviderMock(args),
    saveProject: (args: { project: { name: string; homeDir: string } }) => saveProjectMock(args),
    selectDir: () => selectDirMock(),
    listProjects: () => listProjectsMock(),
    listSessions: () => listSessionsMock(),
    getUiState: () => Promise.resolve(null),
    setUiState: (state: unknown) => setUiStateMock(state),
  };
});

import { BUILTIN_PRESETS } from "@/settings/presets";

/** 基准配置样例（结构对齐 SettingsPage.test.tsx 的 BASE_CONFIG）。 */
function baseConfig(): AppConfig {
  return {
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
}

beforeEach(() => {
  vi.clearAllMocks();
  getConfigMock.mockResolvedValue(baseConfig());
  // 整份保存原样返回（真实后端是归一化结果，测试里等价）
  saveConfigMock.mockImplementation(async (config) => config);
  // 供应商保存原样返回（真实后端保留 id）
  saveProviderMock.mockImplementation(async (provider) => provider);
  // 左栏刷新面：空列表（项目步保存后 refresh 调用）
  listProjectsMock.mockResolvedValue([]);
  listSessionsMock.mockResolvedValue([]);
  setUiStateMock.mockResolvedValue(undefined);
  useSettingsStore.setState({
    config: baseConfig(),
    baseline: baseConfig(),
    loading: false,
    saving: false,
    error: null,
  });
  useSessionsStore.setState({
    onboarding: null,
    tabs: [],
    projects: [],
    sessions: [],
  });
  useToastStore.setState({ toasts: [] });
});

afterEach(cleanup);

describe("四步导航与记账", () => {
  it("语言步：选 English 后「下一步」把 config.ui.language=en-US 经 save_config 落盘，记账 step=1", async () => {
    const onClose = vi.fn();
    render(<OnboardingWizard initialStep={0} onClose={onClose} />);
    fireEvent.click(screen.getByTestId("onboarding-language-English"));
    fireEvent.click(screen.getByTestId("onboarding-language-next"));
    await waitFor(() => {
      expect(saveConfigMock).toHaveBeenCalledTimes(1);
      expect(saveConfigMock.mock.calls[0][0].ui.language).toBe("en-US");
    });
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 1,
      });
    });
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText("配置 AI 供应商")).toBeTruthy();
  });

  it("语言步右上「跳过」：单步前进带 skippedAt，不触发语言写盘", async () => {
    render(<OnboardingWizard initialStep={0} onClose={() => {}} />);
    fireEvent.click(screen.getByTestId("onboarding-skip-step"));
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 1,
      });
    });
    expect(useSessionsStore.getState().onboarding?.skippedAt).toBeDefined();
    expect(saveConfigMock).not.toHaveBeenCalled();
    expect(screen.getByText("配置 AI 供应商")).toBeTruthy();
  });

  it("顶部 × 整包关闭：completed=false、step 保持当前步、onClose 回调", () => {
    const onClose = vi.fn();
    render(<OnboardingWizard initialStep={0} onClose={onClose} />);
    fireEvent.click(screen.getByTestId("onboarding-close"));
    expect(useSessionsStore.getState().onboarding).toEqual({
      completed: false,
      step: 0,
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("从记录 step 续走：initialStep=2 直接渲染项目步", () => {
    useSessionsStore.setState({ onboarding: { completed: false, step: 2 } });
    render(<OnboardingWizard initialStep={2} onClose={() => {}} />);
    expect(screen.getByText("打开或创建项目")).toBeTruthy();
  });
});

describe("供应商步：保存并测试三分支", () => {
  /** 填好一个远端供应商表单（DeepSeek 预设 + Key）。 */
  async function fillProviderForm() {
    render(<OnboardingWizard initialStep={1} onClose={() => {}} />);
    fireEvent.click(screen.getByText("DeepSeek"));
    fireEvent.change(screen.getByPlaceholderText("sk-…"), {
      target: { value: "sk-test-key" },
    });
    fireEvent.click(screen.getByTestId("onboarding-provider-save"));
  }

  it("连通：save_provider（newKey 通道）后 test_provider ok → 显示「下一步」并前进记账", async () => {
    testProviderMock.mockResolvedValue({ ok: true, latencyMs: 42, models: ["deepseek-chat"] });
    await fillProviderForm();
    // 保存走 newKey 通道（upsertProvider 即时落盘）
    await waitFor(() => expect(saveProviderMock).toHaveBeenCalledTimes(1));
    const [provider, newKey] = saveProviderMock.mock.calls[0];
    expect(provider.name).toBe("DeepSeek");
    expect(newKey).toBe("sk-test-key");
    expect(provider.models.length).toBeGreaterThan(0);
    // 探测以保存后的供应商 id 发起
    await waitFor(() =>
      expect(testProviderMock).toHaveBeenCalledWith({
        providerId: provider.id,
        baseUrlOverride: undefined,
      }),
    );
    // 连通成功 → 结果行 + 主按钮转「下一步」
    await waitFor(() => {
      expect(screen.getByTestId("connection-test-ok").textContent).toContain("已连通 · 42ms");
    });
    fireEvent.click(screen.getByTestId("onboarding-provider-next"));
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 2,
      });
    });
  });

  it("探测失败：红字分类文案 +「仍要保存并重测」与「跳过此步」并存；跳过后已存供应商保留", async () => {
    testProviderMock.mockResolvedValue({
      ok: false,
      error: { code: "E_PROVIDER_AUTH", message: "401 invalid key" },
    });
    await fillProviderForm();
    await waitFor(() => {
      expect(screen.getByTestId("connection-test-error").textContent).toContain(
        "认证失败：API Key 无效或无权限",
      );
    });
    // 两个出口并存（票面：「仍要保存」与「跳过」并存）
    expect(screen.getByTestId("onboarding-provider-keep")).toBeTruthy();
    expect(screen.getByTestId("onboarding-provider-save")).toBeTruthy();
    // 跳过此步：供应商已保存过（saveProvider 一次），step 前进且记录 completed=false
    fireEvent.click(screen.getByTestId("onboarding-provider-keep"));
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 2,
      });
    });
    expect(saveProviderMock).toHaveBeenCalledTimes(1);
  });

  it("未保存即跳过 → 无 provider/project 写调用（引导不产生脏数据的验收面）", async () => {
    render(<OnboardingWizard initialStep={1} onClose={() => {}} />);
    fireEvent.click(screen.getByTestId("onboarding-skip-step"));
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 2,
      });
    });
    expect(saveProviderMock).not.toHaveBeenCalled();
    expect(testProviderMock).not.toHaveBeenCalled();
    expect(saveProjectMock).not.toHaveBeenCalled();
  });

  it("test_provider 未注册（reject）：降级提示「稍后在设置中测试」+「下一步」，不阻塞向导", async () => {
    testProviderMock.mockRejectedValue({ code: "E_NOT_FOUND", message: "unknown command" });
    await fillProviderForm();
    await waitFor(() => {
      expect(screen.getByTestId("onboarding-provider-unavailable").textContent).toContain(
        "稍后在设置中测试",
      );
    });
    fireEvent.click(screen.getByTestId("onboarding-provider-next"));
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 2,
      });
    });
  });

  it("保存失败（save_provider reject）：Toast 错误提示且停留本步", async () => {
    saveProviderMock.mockRejectedValue({ code: "E_IO", message: "写盘失败" });
    await fillProviderForm();
    await waitFor(() => {
      expect(
        useToastStore
          .getState()
          .toasts.some((t) => t.kind === "error" && t.title === "供应商保存失败"),
      ).toBe(true);
    });
    expect(testProviderMock).not.toHaveBeenCalled();
    expect(useSessionsStore.getState().onboarding).toBeNull();
  });
});

describe("项目步与安全简介步", () => {
  it("select_dir 选目录 + 名称留空 → save_project 以目录名兜底，前进记账", async () => {
    saveProjectMock.mockResolvedValue({
      id: "p-1",
      name: "demo",
      homeDir: "/tmp/demo",
      createdAt: 1,
    });
    selectDirMock.mockResolvedValue("/tmp/demo");
    render(<OnboardingWizard initialStep={2} onClose={() => {}} />);
    fireEvent.click(screen.getByTestId("onboarding-project-pick"));
    await waitFor(() => expect(screen.getByText("/tmp/demo")).toBeTruthy());
    fireEvent.click(screen.getByTestId("onboarding-project-save"));
    await waitFor(() => {
      expect(saveProjectMock).toHaveBeenCalledWith({
        project: { name: "demo", homeDir: "/tmp/demo" },
      });
    });
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 3,
      });
    });
    expect(screen.getByText("权限与安全简介")).toBeTruthy();
  });

  it("安全简介步：四档说明 + 无右上「跳过」（纯展示）+「完成」收口 completed=true", () => {
    const onClose = vi.fn();
    render(<OnboardingWizard initialStep={3} onClose={onClose} />);
    // 四档各一句话（Shift+Tab 循环的四个档位）
    expect(screen.getByText("计划")).toBeTruthy();
    expect(screen.getByText("逐项确认")).toBeTruthy();
    expect(screen.getByText("自动编辑")).toBeTruthy();
    expect(screen.getByText("完全信任")).toBeTruthy();
    // 末步不提供单步跳过（票面：仅一个「完成」钮，纯展示无写操作）
    expect(screen.queryByTestId("onboarding-skip-step")).toBeNull();
    fireEvent.click(screen.getByTestId("onboarding-finish"));
    expect(useSessionsStore.getState().onboarding).toEqual({ completed: true, step: 3 });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("「继续初始配置」横幅三态渲染", () => {
  it("无记录（null）不渲染——首启走自动弹，横幅不抢戏", () => {
    render(<OnboardingResumeBanner state={null} onOpen={() => {}} />);
    expect(screen.queryByTestId("onboarding-resume-banner")).toBeNull();
  });

  it("未完成记录渲染；点击回调（父层从记录步重开向导）", () => {
    const onOpen = vi.fn();
    render(<OnboardingResumeBanner state={{ completed: false, step: 1 }} onOpen={onOpen} />);
    expect(screen.getByTestId("onboarding-resume-banner").textContent).toContain("继续初始配置");
    fireEvent.click(screen.getByTestId("onboarding-resume-open"));
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("已完成记录不渲染——横幅随 completed 消失（验收口径）", () => {
    render(<OnboardingResumeBanner state={{ completed: true, step: 3 }} onOpen={() => {}} />);
    expect(screen.queryByTestId("onboarding-resume-banner")).toBeNull();
  });
});

describe("预设清单单源回归（与设置页共用）", () => {
  it("向导预设 chips 来自 BUILTIN_PRESETS 同一清单（本地预设带「本地」标注）", () => {
    render(<OnboardingWizard initialStep={1} onClose={() => {}} />);
    for (const preset of BUILTIN_PRESETS) {
      expect(screen.getByText(`${preset.name}${preset.local ? " · 本地" : ""}`)).toBeTruthy();
    }
  });
});
