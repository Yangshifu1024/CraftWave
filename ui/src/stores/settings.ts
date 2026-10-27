/**
 * settings store（§10.1 zustand `settings`：设置草稿 → save_config）。
 *
 * 草稿模型：load 拉取 AppConfig 为草稿；供应商新增/Key 录入即时经
 * save_provider 落协调器+keyring（成功后以返回值刷新草稿）；
 * 其余字段经 saveAll 整份提交。dirty 由草稿与上次已保存引用对比派生。
 */

import { create } from "zustand";
import * as client from "@/ipc/client";
import type { AppConfig, ProviderConfig } from "@/ipc/types";
import { newProviderFromPreset } from "@/settings/presets";

export const DEFAULT_CONFIG: AppConfig = {
  schemaVersion: 2,
  providers: [],
  approval: { enabled: true, autoConfirm: false, commandAllowlist: [] },
  ui: { language: "zh-CN", theme: "system" },
  log: { level: "info" },
};

interface SettingsState {
  /** 已加载草稿（null = 尚未加载） */
  config: AppConfig | null;
  /** 已保存基线（脏判定 = config 与 baseline 的 JSON 差集；随 load/save/供应商即时持久化同步前移） */
  baseline: AppConfig | null;
  loading: boolean;
  saving: boolean;
  /** 最近一次操作的 §3.1 错误负载（骨架级：字符串展示） */
  error: string | null;
  load: () => Promise<void>;
  /** 从内置预设实例化一个供应商（仅入草稿，经 saveAll 落盘） */
  addFromPreset: (slug: string) => void;
  /** 新增/更新供应商并立即持久化（Key 明文存 config.json，§9.4） */
  upsertProvider: (provider: ProviderConfig, newKey?: string) => Promise<boolean>;
  /** 删除供应商（即时持久化） */
  removeProvider: (providerId: string) => Promise<boolean>;
  /** 草稿层设置全局默认模型 */
  setActiveModel: (modelId: string) => void;
  /** 草稿层局部覆写（ui/proxy/approval/log 等非供应商字段） */
  patchConfig: (patch: Partial<AppConfig>) => void;
  /** 整份保存（save_config，返回归一化结果刷新草稿） */
  saveAll: () => Promise<boolean>;
  /** 关闭设置页时丢弃未保存草稿并复位错误 */
  resetDraft: () => Promise<void>;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  config: null,
  baseline: null,
  loading: false,
  saving: false,
  error: null,

  load: async () => {
    set({ loading: true, error: null });
    try {
      const config = await client.getConfig();
      set({ config, baseline: structuredClone(config), loading: false });
    } catch (err) {
      set({ error: client.toIpcError(err).message, loading: false });
    }
  },

  addFromPreset: (slug) => {
    const provider = newProviderFromPreset(slug);
    if (!provider) return;
    const current = get().config ?? DEFAULT_CONFIG;
    set({ config: { ...current, providers: [...current.providers, provider] } });
  },

  upsertProvider: async (provider, newKey) => {
    set({ saving: true, error: null });
    try {
      const saved = await client.saveProvider(provider, newKey);
      const current = get().config;
      if (current) {
        const providers = current.providers.some((p) => p.id === saved.id)
          ? current.providers.map((p) => (p.id === saved.id ? saved : p))
          : [...current.providers, saved];
        const next = { ...current, providers };
        // 供应商走即时持久化：config 与 baseline 一起前移，脏判定不受影响
        set({ config: next, baseline: structuredClone(next), saving: false });
      } else {
        set({ saving: false });
      }
      return true;
    } catch (err) {
      set({ error: client.toIpcError(err).message, saving: false });
      return false;
    }
  },

  removeProvider: async (providerId) => {
    set({ saving: true, error: null });
    try {
      await client.deleteProvider(providerId);
      const current = get().config;
      if (current) {
        const next = {
          ...current,
          providers: current.providers.filter((p) => p.id !== providerId),
        };
        set({ config: next, baseline: structuredClone(next), saving: false });
      } else {
        set({ saving: false });
      }
      return true;
    } catch (err) {
      set({ error: client.toIpcError(err).message, saving: false });
      return false;
    }
  },

  setActiveModel: (modelId) => {
    const current = get().config;
    if (!current) return;
    set({ config: { ...current, activeModelId: modelId } });
  },

  patchConfig: (patch) => {
    const current = get().config;
    if (!current) return;
    set({ config: { ...current, ...patch } });
  },

  saveAll: async () => {
    const current = get().config;
    if (!current) return false;
    set({ saving: true, error: null });
    try {
      const normalized = await client.saveConfig(current);
      set({ config: normalized, baseline: structuredClone(normalized), saving: false });
      return true;
    } catch (err) {
      set({ error: client.toIpcError(err).message, saving: false });
      return false;
    }
  },

  resetDraft: async () => {
    set({ error: null });
    await get().load();
  },
}));
