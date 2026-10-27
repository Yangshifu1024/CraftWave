/**
 * 最小设置页（§10.7 供应商组；骨架级，视觉打磨属桌面体验阶段）。
 *
 * 覆盖本票四要素：供应商列表、内置预设选择、自定义 OpenAI 兼容端点表单、
 * Key 录入（save_provider → keyring，config 只落引用）。
 * 外观/安全/诊断组随后续票；脏状态离开三选属桌面体验阶段。
 */

import { useEffect, useState } from "react";
import type { ProviderConfig } from "@/ipc/types";
import {
  BUILTIN_PRESETS,
  newCustomProvider,
  newDraftId,
  newProviderFromPreset,
} from "@/settings/presets";
import { useSettingsStore } from "@/stores/settings";

export function SettingsPage({ onClose }: { onClose: () => void }) {
  const config = useSettingsStore((s) => s.config);
  const loading = useSettingsStore((s) => s.loading);
  const saving = useSettingsStore((s) => s.saving);
  const error = useSettingsStore((s) => s.error);
  const load = useSettingsStore((s) => s.load);
  const upsertProvider = useSettingsStore((s) => s.upsertProvider);
  const removeProvider = useSettingsStore((s) => s.removeProvider);
  const setActiveModel = useSettingsStore((s) => s.setActiveModel);
  const saveAll = useSettingsStore((s) => s.saveAll);
  const resetDraft = useSettingsStore((s) => s.resetDraft);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [presetSlug, setPresetSlug] = useState<string>(BUILTIN_PRESETS[0]?.slug ?? "");

  useEffect(() => {
    if (!config) {
      void load();
    }
  }, [config, load]);

  const selected = config?.providers.find((p) => p.id === selectedId) ?? null;

  const handleAddPreset = () => {
    const created = newProviderFromPreset(presetSlug);
    if (!created || !config) return;
    useSettingsStore.setState({
      config: { ...config, providers: [...config.providers, created] },
    });
    setSelectedId(created.id);
  };

  const handleAddCustom = () => {
    const draft = newCustomProvider();
    if (config) {
      useSettingsStore.setState({
        config: { ...config, providers: [...config.providers, draft] },
      });
    }
    setSelectedId(draft.id);
  };

  return (
    <div className="fixed inset-0 z-50 overflow-auto bg-white text-sm text-neutral-900">
      <header className="flex items-center justify-between border-b border-neutral-200 px-6 py-3">
        <h1 className="text-base font-semibold">设置</h1>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="rounded border border-neutral-300 px-3 py-1 disabled:opacity-50"
            disabled={saving || !config}
            onClick={() => void saveAll()}
          >
            保存全部配置
          </button>
          <button
            type="button"
            className="rounded border border-neutral-300 px-3 py-1"
            onClick={() => {
              void resetDraft();
              onClose();
            }}
          >
            关闭
          </button>
        </div>
      </header>

      {error && <p className="bg-red-50 px-6 py-2 text-red-700">错误：{error}</p>}
      {loading && <p className="px-6 py-2 text-neutral-500">加载配置中…</p>}

      <section className="max-w-3xl px-6 py-4">
        <h2 className="mb-2 font-semibold">供应商</h2>

        <ul className="mb-3 divide-y divide-neutral-100 rounded border border-neutral-200">
          {(config?.providers ?? []).map((p) => (
            <li key={p.id} className="flex items-center justify-between px-3 py-2">
              <button type="button" className="text-left" onClick={() => setSelectedId(p.id)}>
                <span className="font-medium">{p.name || "（未命名）"}</span>
                <span className="ml-2 text-xs text-neutral-500">
                  {p.apiFormat} · {p.apiKey ? "Key 已配置" : "未配 Key"} · {p.models.length} 个模型
                </span>
              </button>
              <button
                type="button"
                className="text-xs text-red-600"
                onClick={() => void removeProvider(p.id)}
              >
                删除
              </button>
            </li>
          ))}
          {config && config.providers.length === 0 && (
            <li className="px-3 py-2 text-neutral-500">暂无供应商，从内置预设或自定义端点添加。</li>
          )}
        </ul>

        <div className="mb-4 flex flex-wrap items-center gap-2">
          <select
            className="rounded border border-neutral-300 px-2 py-1"
            value={presetSlug}
            onChange={(e) => setPresetSlug(e.target.value)}
            aria-label="内置预设"
          >
            {BUILTIN_PRESETS.map((p) => (
              <option key={p.slug} value={p.slug}>
                {p.name}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="rounded border border-neutral-300 px-3 py-1"
            onClick={handleAddPreset}
          >
            从预设添加
          </button>
          <button
            type="button"
            className="rounded border border-neutral-300 px-3 py-1"
            onClick={handleAddCustom}
          >
            自定义 OpenAI 兼容端点
          </button>
        </div>

        {selected && (
          <ProviderEditor
            key={selected.id}
            provider={selected}
            activeModelId={config?.activeModelId}
            onSetActiveModel={setActiveModel}
            onSave={(provider, newKey) => void upsertProvider(provider, newKey)}
          />
        )}
      </section>
    </div>
  );
}

function ProviderEditor({
  provider,
  activeModelId,
  onSetActiveModel,
  onSave,
}: {
  provider: ProviderConfig;
  activeModelId?: string;
  onSetActiveModel: (modelId: string) => void;
  onSave: (provider: ProviderConfig, newKey?: string) => void;
}) {
  const [name, setName] = useState(provider.name);
  const [baseUrl, setBaseUrl] = useState(provider.baseUrl ?? "");
  const [newKey, setNewKey] = useState("");
  const [modelInput, setModelInput] = useState("");

  const draft: ProviderConfig = {
    ...provider,
    name,
    baseUrl: baseUrl.trim() === "" ? undefined : baseUrl.trim(),
  };

  return (
    <div className="rounded border border-neutral-300 p-3">
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <label htmlFor="provider-name" className="w-20">
          名称
        </label>
        <input
          id="provider-name"
          className="flex-1 rounded border border-neutral-300 px-2 py-1"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
      </div>
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <label htmlFor="provider-base-url" className="w-20">
          Base URL
        </label>
        <input
          id="provider-base-url"
          className="flex-1 rounded border border-neutral-300 px-2 py-1 font-mono text-xs"
          placeholder="缺省用 {apiFormat} 官方地址"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
        />
      </div>
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <label htmlFor="provider-new-key" className="w-20">
          API Key
        </label>
        <input
          id="provider-new-key"
          type="password"
          className="flex-1 rounded border border-neutral-300 px-2 py-1"
          placeholder={provider.apiKey ? "已保存（输入即覆写）" : "输入后保存（存于 config.json）"}
          value={newKey}
          onChange={(e) => setNewKey(e.target.value)}
        />
      </div>

      <h3 className="mt-3 mb-1 font-medium">模型</h3>
      <ul className="mb-2 divide-y divide-neutral-100 rounded border border-neutral-200">
        {provider.models.map((m) => (
          <li key={m.id} className="flex items-center justify-between px-3 py-1.5">
            <span className="font-mono text-xs">
              {m.model}
              <span className="ml-2 text-neutral-400">
                ctx {m.contextWindow} · max {m.maxTokens}
              </span>
            </span>
            {activeModelId === m.id ? (
              <span className="text-xs text-green-700">全局默认</span>
            ) : (
              <button
                type="button"
                className="text-xs text-blue-700"
                onClick={() => onSetActiveModel(m.id)}
              >
                设为默认
              </button>
            )}
          </li>
        ))}
        {provider.models.length === 0 && (
          <li className="px-3 py-1.5 text-neutral-500">暂无模型，下方添加。</li>
        )}
      </ul>
      <div className="flex items-center gap-2">
        <input
          className="flex-1 rounded border border-neutral-300 px-2 py-1 font-mono text-xs"
          placeholder="模型 wire id，如 deepseek-chat"
          value={modelInput}
          onChange={(e) => setModelInput(e.target.value)}
        />
        <button
          type="button"
          className="rounded border border-neutral-300 px-3 py-1"
          onClick={() => {
            const model = modelInput.trim();
            if (model === "") return;
            useSettingsStore.setState((s) => {
              if (!s.config) return s;
              return {
                config: {
                  ...s.config,
                  providers: s.config.providers.map((p) =>
                    p.id === provider.id
                      ? {
                          ...p,
                          models: [
                            ...p.models,
                            {
                              id: newDraftId(),
                              model,
                              maxTokens: 32768,
                              contextWindow: 128000,
                            },
                          ],
                        }
                      : p,
                  ),
                },
              };
            });
            setModelInput("");
          }}
        >
          添加模型
        </button>
      </div>

      <button
        type="button"
        className="mt-3 rounded bg-neutral-900 px-3 py-1 text-white disabled:opacity-50"
        disabled={isSaveDisabled(draft)}
        onClick={() => onSave(draft, newKey.trim() === "" ? undefined : newKey)}
      >
        保存供应商{newKey.trim() === "" ? "" : "（含 Key → 钥匙串）"}
      </button>
    </div>
  );
}

/** 骨架级校验：名称与至少一个模型必填（完整校验在 Rust 协调器归一化）。 */
function isSaveDisabled(provider: ProviderConfig): boolean {
  return provider.name.trim() === "" || provider.models.length === 0;
}
