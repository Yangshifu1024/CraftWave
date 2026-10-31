// @vitest-environment happy-dom
/**
 * 首启引导向导测试（30 号票，FR-13-18）。
 *
 * mock 唯一 invoke 出口（@/ipc/client）与 sessions/settings 两个 store 的
 * 初始态，断言外部行为面（原逐分支用例已合并为四步向导的四个剧本，覆盖
 * 语义不减）：
 * 1. 四步导航剧本：顶部 × 整包关闭 / 语言步 save_config 落盘 / 单步跳过
 *    skippedAt 记账 / 从记录 step 续走——记账经 setOnboarding 落 sessions
 *    store，走既有防抖通道落盘；
 * 2. 供应商步剧本：save_provider（newKey 通道）+ test_provider 连通 /
 *    探测失败「仍要保存」与「跳过」并存 / 未保存即跳过无脏写；
 * 3. 项目步 select_dir + save_project（名称留空取目录名兜底）→ 末步
 *    「完成」completed=true 收口；
 * 4. 「继续配置」横幅三态渲染 + 预设 chips 与 BUILTIN_PRESETS 同源。
 *
 * SessionWorkspace 的自动弹接线（恢复会话列表后 shouldAutoOpen 判定）依赖
 * assistant-ui 重资源树，不在本文件覆盖——判定纯函数在 onboarding-state
 * 测试钉住，真实首启弹层走用户 GUI 验收面（票内决议记录诚实声明）。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLang, t } from "@/i18n/core";
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
    update: { enabled: true, intervalHours: 24 },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  // 31 号票 i18n：语言是模块级全局态——语言步用例切 en-US 后须复位，
  // 避免泄漏到后续用例（组件文案按 zh 基准断言）
  setLang("zh-CN");
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

describe("OnboardingWizard（30 号票四步向导）", () => {
  it("导航与记账剧本：顶部 × 整包关闭、语言步落盘 en-US、单步跳过带 skippedAt、从记录 step 续走", async () => {
    // 顶部 × 整包关闭：completed=false、step 保持当前步、onClose 回调
    let wizard = render(<OnboardingWizard initialStep={0} onClose={() => {}} />);
    fireEvent.click(screen.getByTestId("onboarding-close"));
    expect(useSessionsStore.getState().onboarding).toEqual({
      completed: false,
      step: 0,
    });
    wizard.unmount();

    // 语言步：选 English 后「下一步」把 config.ui.language=en-US 经 save_config 落盘，记账 step=1
    useSessionsStore.setState({ onboarding: null });
    const onClose = vi.fn();
    wizard = render(<OnboardingWizard initialStep={0} onClose={onClose} />);
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
    expect(useSessionsStore.getState().onboarding?.skippedAt).toBeUndefined();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText(t("onboarding.providerTitle"))).toBeTruthy();
    wizard.unmount();

    // 语言步右上「跳过」：单步前进带 skippedAt，不触发语言写盘
    useSessionsStore.setState({ onboarding: null });
    wizard = render(<OnboardingWizard initialStep={0} onClose={() => {}} />);
    fireEvent.click(screen.getByTestId("onboarding-skip-step"));
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 1,
      });
    });
    expect(useSessionsStore.getState().onboarding?.skippedAt).toBeDefined();
    expect(saveConfigMock).toHaveBeenCalledTimes(1); // 仅语言步那一次，跳过未再写盘
    expect(screen.getByText(t("onboarding.providerTitle"))).toBeTruthy();
    wizard.unmount();

    // 从记录 step 续走：initialStep=2 直接渲染项目步
    useSessionsStore.setState({ onboarding: { completed: false, step: 2 } });
    render(<OnboardingWizard initialStep={2} onClose={() => {}} />);
    expect(screen.getByText(t("onboarding.projectTitle"))).toBeTruthy();
  });

  it("供应商步剧本：连通（save_provider newKey 通道 + test_provider ok + 前进）、探测失败双出口与跳过保留、未保存跳过无脏写", async () => {
    /** 填好一个远端供应商表单（DeepSeek 预设 + Key）。 */
    const fillProviderForm = () => {
      useSessionsStore.setState({ onboarding: null });
      const wizard = render(<OnboardingWizard initialStep={1} onClose={() => {}} />);
      fireEvent.click(screen.getByText("DeepSeek"));
      fireEvent.change(screen.getByPlaceholderText("sk-…"), {
        target: { value: "sk-test-key" },
      });
      fireEvent.click(screen.getByTestId("onboarding-provider-save"));
      return wizard;
    };

    // 连通：保存走 newKey 通道（upsertProvider 即时落盘），探测以保存后的
    // 供应商 id 发起；连通成功 → 结果行 + 主按钮转「下一步」并前进记账
    testProviderMock.mockResolvedValue({ ok: true, latencyMs: 42, models: ["deepseek-chat"] });
    let wizard = fillProviderForm();
    await waitFor(() => expect(saveProviderMock).toHaveBeenCalledTimes(1));
    const [provider, newKey] = saveProviderMock.mock.calls[0];
    expect(provider.name).toBe("DeepSeek");
    expect(newKey).toBe("sk-test-key");
    expect(provider.models.length).toBeGreaterThan(0);
    await waitFor(() =>
      expect(testProviderMock).toHaveBeenCalledWith({
        providerId: provider.id,
        baseUrlOverride: undefined,
      }),
    );
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
    wizard.unmount();

    // 探测失败：红字分类文案 +「仍要保存并重测」与「跳过此步」并存
    //（票面：两者并存）；跳过后已存供应商保留（saveProvider 共连通段那一次）
    testProviderMock.mockResolvedValue({
      ok: false,
      error: { code: "E_PROVIDER_AUTH", message: "401 invalid key" },
    });
    wizard = fillProviderForm();
    await waitFor(() => {
      expect(screen.getByTestId("connection-test-error").textContent).toContain(
        "认证失败：API Key 无效或无权限",
      );
    });
    expect(screen.getByTestId("onboarding-provider-keep")).toBeTruthy();
    expect(screen.getByTestId("onboarding-provider-save")).toBeTruthy();
    // 跳过此步：供应商已保存过，step 前进且记录 completed=false
    fireEvent.click(screen.getByTestId("onboarding-provider-keep"));
    await waitFor(() => {
      expect(useSessionsStore.getState().onboarding).toMatchObject({
        completed: false,
        step: 2,
      });
    });
    expect(saveProviderMock).toHaveBeenCalledTimes(2); // 连通段 1 次 + 本段 1 次，跳过不再写
    wizard.unmount();

    // 未保存即跳过 → 无 provider/project 写调用（引导不产生脏数据的验收面）
    useSessionsStore.setState({ onboarding: null });
    wizard = render(<OnboardingWizard initialStep={1} onClose={() => {}} />);
    saveProviderMock.mockClear();
    testProviderMock.mockClear();
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
    wizard.unmount();
  });

  it("项目步与安全简介步收口剧本：select_dir + 名称留空取目录名兜底保存 → 末步四档说明、无单步跳过、「完成」completed=true + onClose", async () => {
    const onClose = vi.fn();
    saveProjectMock.mockResolvedValue({
      id: "p-1",
      name: "demo",
      homeDir: "/tmp/demo",
      createdAt: 1,
    });
    selectDirMock.mockResolvedValue("/tmp/demo");
    render(<OnboardingWizard initialStep={2} onClose={onClose} />);
    // 项目步：选目录 → 名称留空保存 → 以目录名兜底 + 前进记账
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
    // 安全简介步：四档各一句话（Shift+Tab 循环的四个档位）
    expect(screen.getByText(t("onboarding.securityTitle"))).toBeTruthy();
    expect(screen.getByText(t("onboarding.tierPlanLabel"))).toBeTruthy();
    expect(screen.getByText(t("onboarding.tierConfirmEachLabel"))).toBeTruthy();
    expect(screen.getByText(t("onboarding.tierAutoEditLabel"))).toBeTruthy();
    expect(screen.getByText(t("onboarding.tierFullAccessLabel"))).toBeTruthy();
    // 末步不提供单步跳过（票面：仅一个「完成」钮，纯展示无写操作）
    expect(screen.queryByTestId("onboarding-skip-step")).toBeNull();
    // 「完成」收口 completed=true + 关闭回调
    fireEvent.click(screen.getByTestId("onboarding-finish"));
    expect(useSessionsStore.getState().onboarding).toEqual({ completed: true, step: 3 });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("「继续初始配置」横幅三态渲染 + 预设 chips 与 BUILTIN_PRESETS 单源（本地预设带「本地」标注）", () => {
    // 无记录（null）不渲染——首启走自动弹，横幅不抢戏
    let banner = render(<OnboardingResumeBanner state={null} onOpen={() => {}} />);
    expect(screen.queryByTestId("onboarding-resume-banner")).toBeNull();
    banner.unmount();

    // 未完成记录渲染；点击回调（父层从记录步重开向导）
    const onOpen = vi.fn();
    banner = render(
      <OnboardingResumeBanner state={{ completed: false, step: 1 }} onOpen={onOpen} />,
    );
    expect(screen.getByTestId("onboarding-resume-banner").textContent).toContain(
      t("onboarding.bannerResume").replace(" →", ""),
    );
    fireEvent.click(screen.getByTestId("onboarding-resume-open"));
    expect(onOpen).toHaveBeenCalledTimes(1);
    banner.unmount();

    // 已完成记录不渲染——横幅随 completed 消失（验收口径）
    banner = render(
      <OnboardingResumeBanner state={{ completed: true, step: 3 }} onOpen={() => {}} />,
    );
    expect(screen.queryByTestId("onboarding-resume-banner")).toBeNull();
    banner.unmount();

    // 向导预设 chips 来自 BUILTIN_PRESETS 同一清单（与设置页共用）
    render(<OnboardingWizard initialStep={1} onClose={() => {}} />);
    for (const preset of BUILTIN_PRESETS) {
      expect(
        screen.getByText(`${preset.name}${preset.local ? t("onboarding.presetLocal") : ""}`),
      ).toBeTruthy();
    }
  });
});
