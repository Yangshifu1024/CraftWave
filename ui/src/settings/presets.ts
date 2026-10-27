/**
 * 内置供应商预设（TS 侧镜像 src-tauri/src/providers/presets.rs）。
 *
 * 预设是"一键新增常用供应商"的静态模板（名称/端点/模型建议，不含凭据）；
 * slug 是每个预设的稳定标识键（如 "openai"），两侧清单靠它对齐。
 *
 * 预设集是静态 UX 数据：Rust 侧为权威清单（presets.rs，含测试）；
 * §3.3 命令表无 list_presets 命令，M1 骨架由前端持同源副本，
 * 两侧 slug 对齐，漂移由票 Answer 记录、后续票再收敛。
 */

import type { ApiFormat, ProviderConfig } from "@/ipc/types";

/** 预设里的单个模型建议（实例化时转成 ProviderModel，id 现生成）。 */
export interface PresetModelSuggestion {
  /** 模型 wire id（服务商接口里的模型标识，即显示名） */
  model: string;
  /** 上下文窗口（token 数） */
  contextWindow: number;
  /** 单次回复输出上限（token 数） */
  maxTokens: number;
  /** 是否支持图像输入 */
  vision: boolean;
}

/** 一个供应商预设的完整模板。 */
export interface ProviderPreset {
  /** 前端稳定键（与 Rust 侧 slug 一致） */
  slug: string;
  /** 展示名（实例化后成为供应商名称） */
  name: string;
  apiFormat: ApiFormat;
  baseUrl: string;
  models: PresetModelSuggestion[];
}

/** 内置供应商预设全集（顺序即设置页"从预设添加"的展示顺序）。 */
export const BUILTIN_PRESETS: ProviderPreset[] = [
  {
    slug: "openai",
    name: "OpenAI",
    apiFormat: "openai_chat",
    baseUrl: "https://api.openai.com/v1",
    models: [
      { model: "gpt-4.1", contextWindow: 1_047_576, maxTokens: 32_768, vision: true },
      { model: "gpt-4o-mini", contextWindow: 128_000, maxTokens: 16_384, vision: true },
    ],
  },
  {
    slug: "anthropic",
    name: "Anthropic",
    apiFormat: "anthropic_messages",
    baseUrl: "https://api.anthropic.com",
    models: [
      { model: "claude-sonnet-4-5", contextWindow: 200_000, maxTokens: 64_000, vision: true },
      { model: "claude-haiku-4-5", contextWindow: 200_000, maxTokens: 64_000, vision: true },
    ],
  },
  {
    slug: "deepseek",
    name: "DeepSeek",
    apiFormat: "openai_chat",
    baseUrl: "https://api.deepseek.com",
    models: [
      { model: "deepseek-chat", contextWindow: 128_000, maxTokens: 8_192, vision: false },
      { model: "deepseek-reasoner", contextWindow: 128_000, maxTokens: 8_192, vision: false },
    ],
  },
  {
    slug: "moonshot",
    name: "Kimi (Moonshot)",
    apiFormat: "openai_chat",
    baseUrl: "https://api.moonshot.cn/v1",
    models: [
      { model: "kimi-k2-0905-preview", contextWindow: 256_000, maxTokens: 8_192, vision: false },
    ],
  },
  {
    slug: "zhipu",
    name: "智谱 GLM",
    apiFormat: "openai_chat",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    models: [
      { model: "glm-5.3", contextWindow: 200_000, maxTokens: 128_000, vision: true },
      { model: "glm-5.3-flash", contextWindow: 200_000, maxTokens: 32_768, vision: true },
      { model: "glm-4.6", contextWindow: 200_000, maxTokens: 128_000, vision: true },
    ],
  },
  {
    slug: "openrouter",
    name: "OpenRouter",
    apiFormat: "openai_chat",
    baseUrl: "https://openrouter.ai/api/v1",
    models: [
      { model: "openrouter/auto", contextWindow: 128_000, maxTokens: 32_768, vision: false },
    ],
  },
];

/** 按 slug 查找预设（找不到返回 undefined）。 */
export function findPreset(slug: string): ProviderPreset | undefined {
  return BUILTIN_PRESETS.find((p) => p.slug === slug);
}

/** 生成随机 uuid：优先用 crypto.randomUUID，老环境回退随机串。 */
function newUuid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `tmp-${Math.random().toString(36).slice(2)}${Date.now().toString(36)}`;
}

/** 草稿层新 id（供应商/模型共用 uuid 语义，§3.2）。 */
export function newDraftId(): string {
  return newUuid();
}

/** 从预设实例化 ProviderConfig（草稿用；凭据经 save_provider 提交）。 */
export function newProviderFromPreset(slug: string): ProviderConfig | null {
  const preset = findPreset(slug);
  if (!preset) return null;
  return {
    id: newUuid(),
    name: preset.name,
    apiFormat: preset.apiFormat,
    baseUrl: preset.baseUrl,
    models: preset.models.map((m) => ({
      id: newUuid(),
      model: m.model,
      maxTokens: m.maxTokens,
      contextWindow: m.contextWindow,
      vision: m.vision,
    })),
  };
}

/** 自定义 OpenAI 兼容端点空表单（apiFormat 固定 openai_chat，baseUrl 用户填写）。 */
export function newCustomProvider(): ProviderConfig {
  return {
    id: newUuid(),
    name: "",
    apiFormat: "openai_chat",
    models: [],
  };
}
