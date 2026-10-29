/**
 * 设置页全局状态（§10.1 zustand `settings`：设置草稿 → save_config 落盘）。
 *
 * 草稿模型三概念：
 *  - 草稿（config）：正在编辑、尚未整份保存的配置；
 *  - 基线（baseline）：最近一次保存成功的配置快照；
 *  - 脏（dirty）：草稿与基线不一致（JSON 差集）——据此显示"未保存"圆点、
 *    触发离开拦截，dirty 判定是派生值，不单独存储。
 *
 * 保存路径分两类：供应商新增/Key 录入即时经 save_provider 落盘（成功后以
 * 返回值刷新草稿，基线同步前移、脏判定不受影响）；其余字段经 saveAll 整份提交。
 */

import { create } from "zustand";
import * as client from "@/ipc/client";
import type { AppConfig, ProviderConfig } from "@/ipc/types";
import { newProviderFromPreset } from "@/settings/presets";

/** 兜底默认配置：后端没有任何配置时（全新安装）的草稿起点。 */
export const DEFAULT_CONFIG: AppConfig = {
  schemaVersion: 2,
  providers: [],
  approval: { enabled: true, autoConfirm: false, commandAllowlist: [] },
  ui: { language: "zh-CN", theme: "system" },
  log: { level: "info" },
  notifications: { enabled: true, interventionOnly: false },
};

/** 设置状态仓库的形状：草稿/基线双份 + 读写动作（说明见各字段行内注释）。 */
interface SettingsState {
  /** 已加载草稿（null = 尚未加载） */
  config: AppConfig | null;
  /** 已保存基线（脏判定 = config 与 baseline 的 JSON 差集；随 load/save/供应商即时持久化同步前移） */
  baseline: AppConfig | null;
  /** 配置读取中（设置页顶部提示条用） */
  loading: boolean;
  /** 整份保存中（保存按钮置灰防重复提交） */
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

/** 设置状态仓库实例（zustand store；用法见 stores/ask.ts 文件头说明）。 */
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
      // structuredClone 深拷贝出基线：基线与草稿必须是两份独立对象，
      // 之后改草稿才不会殃及基线（否则脏判定永远为 false）
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
    // 重新 load：等价于把草稿丢掉、回到已保存状态（基线不变）
    set({ error: null });
    await get().load();
  },
}));
