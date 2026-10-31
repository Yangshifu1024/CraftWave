// @vitest-environment happy-dom
/**
 * 供应商页连接测试与本地预设单测（19 号票，FR-02-09/10/04）。
 *
 * mock 唯一 invoke 出口（@/ipc/client），断言三个外部行为面：
 * 1. 测试按钮状态机（成功结果显示延迟/模型数；失败显示七类分类人话文案，
 *    脱敏不回显原始 message）；
 * 2. 探测错误分类 → 人话文案映射表（纯函数）；
 * 3. 本地预设（免 Key 放行）与远端预设（Key 必填）。
 * 精简版：每行为面留代表 1 例。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig, ProviderConfig, TestProviderResult } from "@/ipc/types";

// 各 IPC 命令的测试桩（vi.fn 可按用例指定返回值或抛错）
const testProvider =
  vi.fn<(args: { providerId: string; baseUrlOverride?: string }) => Promise<TestProviderResult>>();

// 整体替换 IPC 客户端为 fake：组件代码以为在调后端，实际打到上面的桩
vi.mock("@/ipc/client", () => ({
  COMMANDS: {
    getConfig: "get_config",
    saveConfig: "save_config",
    saveProvider: "save_provider",
    deleteProvider: "delete_provider",
    testProvider: "test_provider",
  },
  getConfig: vi.fn(),
  saveConfig: vi.fn(),
  saveProvider: vi.fn(),
  deleteProvider: vi.fn(),
  testProvider: (args: { providerId: string; baseUrlOverride?: string }) => testProvider(args),
  toIpcError: (err: unknown) =>
    typeof err === "object" && err !== null && "code" in err
      ? (err as { code: string; message: string })
      : { code: "E_TEST", message: String(err) },
}));

import { ProvidersPage, probeErrorLabel } from "@/settings/ProvidersPage";
import { useSettingsStore } from "@/stores/settings";

/** 造一个已保存的远端供应商样例（openai_chat + Key）。 */
function savedProvider(overrides: Partial<ProviderConfig> = {}): ProviderConfig {
  return {
    id: "prov-1",
    name: "示例供应商",
    apiFormat: "openai_chat",
    baseUrl: "https://api.example.com/v1",
    apiKey: "sk-test",
    local: false,
    models: [{ id: "m-1", model: "demo", maxTokens: 8192, contextWindow: 128000 }],
    ...overrides,
  };
}

/** 造一份完整 AppConfig 样例。 */
function configFixture(providers: ProviderConfig[]): AppConfig {
  return {
    schemaVersion: 2,
    providers,
    activeModelId: providers[0]?.models[0]?.id,
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
    compaction: { threshold: 0.6, timeoutSecs: 180 },
    tools: { maxParallel: 4, batchReadLimit: 20 },
    watchdog: { stallTimeoutSecs: 300 },
    network: { allowPrivateNetwork: false },
    search: {
      searxng: { enabled: false, baseUrl: "http://127.0.0.1:8888", maxResults: 10 },
    },
    update: { enabled: true, intervalHours: 24 },
    notifications: { enabled: true, interventionOnly: true },
  };
}

afterEach(cleanup);

beforeEach(() => {
  testProvider.mockReset();
  useSettingsStore.setState({
    config: configFixture([savedProvider()]),
    baseline: configFixture([savedProvider()]),
  });
});

describe("供应商页（19 号票）", () => {
  it("测试连接状态机（FR-02-09）：成功显示延迟与模型数；失败显示人话文案且脱敏", async () => {
    // 成功：延迟 + 模型数
    testProvider.mockResolvedValue({
      ok: true,
      latencyMs: 123,
      models: ["a", "b"],
    });
    const ok = render(<ProvidersPage />);
    fireEvent.click(screen.getByRole("button", { name: "测试连接" }));
    await waitFor(() => {
      expect(screen.getByTestId("connection-test-ok").textContent).toContain("已连通 · 123ms");
      expect(screen.getByTestId("connection-test-ok").textContent).toContain("2 个模型");
    });
    expect(testProvider).toHaveBeenCalledWith({
      providerId: "prov-1",
      baseUrlOverride: undefined,
    });
    ok.unmount();

    // 失败：七类分类的人话文案（auth 分类映射）；
    // 脱敏红线：人话文案不回显后端原始 message（原始值在 title 排查面）
    testProvider.mockResolvedValue({
      ok: false,
      error: { code: "E_PROVIDER_AUTH", message: "401：invalid key" },
    });
    const failed = render(<ProvidersPage />);
    fireEvent.click(screen.getByRole("button", { name: "测试连接" }));
    await waitFor(() => {
      const row = screen.getByTestId("connection-test-error");
      expect(row.textContent).toContain("认证失败：API Key 无效或无权限");
      expect(row.textContent).not.toContain("invalid key");
    });
    failed.unmount();
  });

  it("探测错误分类 → 人话文案映射表：七类各有人话文案，未知 code 兜底原样带出", () => {
    expect(probeErrorLabel("E_PROVIDER_AUTH")).toContain("API Key");
    expect(probeErrorLabel("E_PROVIDER_BILLING")).toContain("余额");
    expect(probeErrorLabel("E_PROVIDER_RATE_LIMITED")).toContain("限流");
    expect(probeErrorLabel("E_PROVIDER_BAD_REQUEST")).toContain("/v1");
    expect(probeErrorLabel("E_PROVIDER_SERVER")).toContain("服务端");
    expect(probeErrorLabel("E_PROVIDER_PROTOCOL")).toContain("协议");
    expect(probeErrorLabel("E_PROVIDER_NETWORK")).toContain("无法连接");
    expect(probeErrorLabel("E_UNKNOWN_KIND")).toContain("E_UNKNOWN_KIND");
  });

  it("本地预设 chips 填充且免 Key（FR-02-04）；远端预设仍要求 Key", () => {
    // 本地预设 chip（Ollama 带「本地」标注）点击填充
    const local = render(<ProvidersPage />);
    fireEvent.click(screen.getByRole("button", { name: /添加供应商/ }));
    fireEvent.click(screen.getByText("Ollama（本地） · 本地"));
    expect(screen.getByDisplayValue("http://127.0.0.1:11434/v1")).toBeTruthy();
    expect(screen.getByDisplayValue("Ollama（本地）")).toBeTruthy();
    // 免 Key：Key 输入框不出现，显示「本地 / 离线可用」与免 Key 说明
    expect(screen.queryByPlaceholderText("sk-…")).not.toBeTruthy();
    expect(screen.getAllByText("无需 API Key").length).toBeGreaterThan(0);
    // 预设建议模型已带入
    expect(screen.getByText("qwen3:8b")).toBeTruthy();
    local.unmount();

    // 远端预设：Key 必填说明仍在（未填时字段校验红字可被提交动作触发）
    const remote = render(<ProvidersPage />);
    fireEvent.click(screen.getByRole("button", { name: /添加供应商/ }));
    fireEvent.click(screen.getByText("DeepSeek"));
    expect(screen.getByPlaceholderText("sk-…")).toBeTruthy();
    remote.unmount();
  });
});
