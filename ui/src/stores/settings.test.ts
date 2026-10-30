/**
 * settings store 单测（§10.1 zustand settings 草稿模型）。
 * mock 唯一 invoke 出口（@/ipc/client，即所有后端调用的收口模块），
 * 断言草稿流转与调用契约——不依赖真实后端。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig, ProviderConfig } from "@/ipc/types";

// 各 IPC 命令的测试桩（vi.fn 可按用例指定返回值或抛错）
const saveConfig = vi.fn();
const saveProvider = vi.fn();
const deleteProvider = vi.fn();
const getConfig = vi.fn();

// 整体替换 IPC 客户端为 fake：store 代码以为在调后端，实际打到上面的桩
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

import { BUILTIN_PRESETS, newProviderFromPreset } from "@/settings/presets";
import { useSettingsStore } from "@/stores/settings";

/** 造一个供应商配置样例（id 用于区分，其余字段值不参与断言）。 */
function providerFixture(id: string): ProviderConfig {
  return {
    id,
    name: `供应商-${id}`,
    apiFormat: "openai_chat",
    baseUrl: "https://api.example.com/v1",
    models: [{ id: `m-${id}`, model: "test-model", maxTokens: 8192, contextWindow: 128000 }],
  };
}

/** 造一份完整 AppConfig 样例（以传入的供应商列表为准，其余用默认值）。 */
function configFixture(providers: ProviderConfig[]): AppConfig {
  return {
    schemaVersion: 2,
    providers,
    approval: { enabled: true, autoConfirm: false, commandAllowlist: [] },
    ui: { language: "zh-CN", theme: "system" },
    log: { level: "info" },
    compaction: { threshold: 0.6, timeoutSecs: 180 },
    tools: { maxParallel: 4 },
    watchdog: { stallTimeoutSecs: 300 },
    notifications: { enabled: true, interventionOnly: false },
  };
}

// 每个用例前：清空 mock 调用记录，并把 store 重置为"草稿未加载"的初始状态
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
    expect(providers[0].apiKey).toBeUndefined();
  });

  it("upsertProvider 调 save_provider 并以返回值刷新草稿", async () => {
    const draft = providerFixture("p1");
    useSettingsStore.setState({ config: configFixture([]) });
    const saved = { ...draft, apiKey: "sk-x" };
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

  it("预设清单来自生成物：BUILTIN_PRESETS 与 Rust 权威清单同源（03 号票单源）", () => {
    // 单源定稿后清单不再手写副本：消费面直接来自 ui/src/ipc/generated/
    // BuiltinPresets.ts（cargo test 从 providers/presets.rs 序列化导出）。
    // 这里钉住消费链路：6 家 slug 齐全、字段形态与 ProviderConfig 实例化兼容。
    expect(BUILTIN_PRESETS.map((p) => p.slug)).toEqual([
      "openai",
      "anthropic",
      "deepseek",
      "moonshot",
      "zhipu",
      "openrouter",
    ]);
    for (const preset of BUILTIN_PRESETS) {
      expect(preset.baseUrl).toMatch(/^https:\/\//);
      expect(preset.models.length).toBeGreaterThan(0);
      for (const model of preset.models) {
        expect(model.model).not.toBe("");
        expect(model.contextWindow).toBeGreaterThan(0);
        expect(model.maxTokens).toBeGreaterThan(0);
        expect(typeof model.vision).toBe("boolean");
      }
    }
  });

  it("customPrompt 草稿-提交-回显（03 号票 agent.customPrompt）", async () => {
    const draft = configFixture([]);
    useSettingsStore.setState({ config: structuredClone(draft) });
    // 草稿层写入 agent.customPrompt（工作区与智能体页的文本框路径）
    useSettingsStore.getState().patchConfig({
      agent: { maxConcurrentSubagents: 4, customPrompt: "始终用中文回复" },
    });
    expect(useSettingsStore.getState().config?.agent?.customPrompt).toBe("始终用中文回复");
    // 草稿脏：与基线出现差集（agent 组是 agent 页切片）
    expect(useSettingsStore.getState().config).not.toEqual(useSettingsStore.getState().baseline);
    // 提交：save_config 载荷带 agent 字段；归一化结果回写草稿与基线
    const normalized: AppConfig = {
      ...draft,
      agent: { maxConcurrentSubagents: 4, customPrompt: "始终用中文回复" },
    };
    saveConfig.mockResolvedValue(normalized);
    const ok = await useSettingsStore.getState().saveAll();
    expect(ok).toBe(true);
    expect(saveConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        agent: { maxConcurrentSubagents: 4, customPrompt: "始终用中文回复" },
      }),
    );
    expect(useSettingsStore.getState().config?.agent?.customPrompt).toBe("始终用中文回复");
    expect(useSettingsStore.getState().baseline?.agent?.customPrompt).toBe("始终用中文回复");

    // 清空（空串 → 归一化后零注入）：后端 normalize 空白归 None，回显无该键
    useSettingsStore
      .getState()
      .patchConfig({ agent: { maxConcurrentSubagents: 4, customPrompt: "" } });
    saveConfig.mockResolvedValue(draft);
    await useSettingsStore.getState().saveAll();
    expect(saveConfig).toHaveBeenLastCalledWith(
      expect.objectContaining({ agent: { maxConcurrentSubagents: 4, customPrompt: "" } }),
    );
    expect(useSettingsStore.getState().config?.agent?.customPrompt).toBeUndefined();
  });
});
