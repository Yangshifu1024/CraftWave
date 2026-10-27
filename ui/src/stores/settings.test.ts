/**
 * settings store 单测（§10.1 zustand settings 草稿模型）。
 * mock 唯一 invoke 出口（@/ipc/client），断言草稿流转与调用契约。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig, ProviderConfig } from "@/ipc/types";

const saveConfig = vi.fn();
const saveProvider = vi.fn();
const deleteProvider = vi.fn();
const getConfig = vi.fn();

vi.mock("@/ipc/client", () => ({
  COMMANDS: {
    getConfig: "get_config",
    saveConfig: "save_config",
    saveProvider: "save_provider",
    deleteProvider: "delete_provider",
  },
  getConfig: (...args: unknown[]) => getConfig(...args),
  saveConfig: (...args: unknown[]) => saveConfig(...args),
  saveProvider: (...args: unknown[]) => saveProvider(...args),
  deleteProvider: (...args: unknown[]) => deleteProvider(...args),
  toIpcError: (err: unknown) =>
    typeof err === "object" && err !== null && "code" in err
      ? (err as { code: string; message: string })
      : { code: "E_TEST", message: String(err) },
}));

import { newProviderFromPreset } from "@/settings/presets";
import { useSettingsStore } from "@/stores/settings";

function providerFixture(id: string): ProviderConfig {
  return {
    id,
    name: `供应商-${id}`,
    apiFormat: "openai_chat",
    baseUrl: "https://api.example.com/v1",
    credentialCount: 0,
    models: [{ id: `m-${id}`, model: "test-model", maxTokens: 8192, contextWindow: 128000 }],
  };
}

function configFixture(providers: ProviderConfig[]): AppConfig {
  return {
    schemaVersion: 2,
    providers,
    approval: { enabled: true, autoConfirm: false, commandAllowlist: [] },
    ui: { language: "zh-CN", theme: "system" },
    log: { level: "info" },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  useSettingsStore.setState({
    config: null,
    loading: false,
    saving: false,
    error: null,
  });
});

describe("settings store", () => {
  it("load 拉取 AppConfig 进草稿", async () => {
    const config = configFixture([providerFixture("p1")]);
    getConfig.mockResolvedValue(config);
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().config?.providers).toHaveLength(1);
    expect(getConfig).toHaveBeenCalledWith();
  });

  it("addFromPreset 入草稿且凭据字段为空", () => {
    useSettingsStore.setState({ config: configFixture([]) });
    useSettingsStore.getState().addFromPreset("deepseek");
    const providers = useSettingsStore.getState().config?.providers ?? [];
    expect(providers).toHaveLength(1);
    expect(providers[0].name).toBe("DeepSeek");
    expect(providers[0].credentialCount).toBe(0);
    expect(providers[0].credentialRef).toBeUndefined();
  });

  it("upsertProvider 调 save_provider 并以返回值刷新草稿", async () => {
    const draft = providerFixture("p1");
    useSettingsStore.setState({ config: configFixture([]) });
    const saved = { ...draft, credentialRef: "craftwave.provider.p1", credentialCount: 1 };
    saveProvider.mockResolvedValue(saved);

    const ok = await useSettingsStore.getState().upsertProvider(draft, "sk-live");
    expect(ok).toBe(true);
    expect(saveProvider).toHaveBeenCalledWith(draft, "sk-live");
    expect(useSettingsStore.getState().config?.providers[0]).toEqual(saved);
  });

  it("upsertProvider 失败置错误且草稿不变", async () => {
    const draft = providerFixture("p1");
    useSettingsStore.setState({ config: configFixture([]) });
    saveProvider.mockRejectedValue({ code: "E_KEYRING", message: "锁定" });

    const ok = await useSettingsStore.getState().upsertProvider(draft, "sk-x");
    expect(ok).toBe(false);
    expect(useSettingsStore.getState().error).toContain("锁定");
    expect(useSettingsStore.getState().config?.providers).toHaveLength(0);
  });

  it("removeProvider 调 delete_provider 并移除草稿项", async () => {
    useSettingsStore.setState({ config: configFixture([providerFixture("p1")]) });
    deleteProvider.mockResolvedValue(undefined);
    const ok = await useSettingsStore.getState().removeProvider("p1");
    expect(ok).toBe(true);
    expect(deleteProvider).toHaveBeenCalledWith("p1");
    expect(useSettingsStore.getState().config?.providers).toHaveLength(0);
  });

  it("saveAll 提交整份草稿并以归一化结果刷新", async () => {
    const draft = configFixture([providerFixture("p1")]);
    useSettingsStore.setState({ config: draft });
    const normalized: AppConfig = { ...draft };
    saveConfig.mockResolvedValue(normalized);
    useSettingsStore.getState().setActiveModel("m-p1");
    const ok = await useSettingsStore.getState().saveAll();
    expect(ok).toBe(true);
    expect(saveConfig).toHaveBeenCalledWith({ ...draft, activeModelId: "m-p1" });
    expect(useSettingsStore.getState().config).toEqual(normalized);
  });

  it("resetDraft 重新拉取丢弃未保存改动", async () => {
    const config = configFixture([]);
    getConfig.mockResolvedValue(config);
    useSettingsStore.setState({ config: configFixture([providerFixture("draft")]) });
    await useSettingsStore.getState().resetDraft();
    expect(useSettingsStore.getState().config?.providers).toHaveLength(0);
  });

  it("预设实例化稳定：slug 齐全且全部 https", () => {
    for (const slug of ["openai", "anthropic", "deepseek", "moonshot", "zhipu", "openrouter"]) {
      const provider = newProviderFromPreset(slug);
      expect(provider, `预设 ${slug} 存在`).not.toBeNull();
      expect(provider?.baseUrl).toMatch(/^https:\/\//);
      expect(provider?.models.length).toBeGreaterThan(0);
    }
    expect(newProviderFromPreset("nope")).toBeNull();
  });
});
