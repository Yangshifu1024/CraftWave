/**
 * 内置供应商预设消费层（03 号票单源定稿，§3.3 预设单源注记）。
 *
 * 预设是"一键新增常用供应商"的静态模板（名称/端点/模型建议，不含凭据）；
 * slug 是每个预设的稳定标识键（如 "openai"）。
 *
 * 单源口径：清单内容与类型都来自 Rust 权威 `src-tauri/src/providers/presets.rs`
 * 的生成物——类型声明（ProviderPreset/PresetModel.ts，ts-rs 导出）与清单
 * 常量（BuiltinPresets.ts，cargo test 序列化 builtin_presets() 产出）都在
 * `ui/src/ipc/generated/`，本模块只 re-export 并提供实例化辅助（草稿层新 id
 * 与 ProviderConfig 实例化）。M1 时代的同源手写副本与"slug 两侧测试钉住"
 * 临时防线已随单源消亡；改 Rust 预设 → cargo test 重新生成 → 前端即时感知。
 */

import { BUILTIN_PRESETS } from "@/ipc/generated/BuiltinPresets";
import type { ProviderConfig, ProviderPreset } from "@/ipc/types";

export type { PresetModel } from "@/ipc/generated/PresetModel";
export type { ProviderPreset };
export { BUILTIN_PRESETS };

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

/** 从预设实例化 ProviderConfig（草稿用；凭据经 save_provider 提交）。
 *  本地预设（FR-02-04，19 号票）随预设带出 local=true——免 Key、离线可用。 */
export function newProviderFromPreset(slug: string): ProviderConfig | null {
  const preset = findPreset(slug);
  if (!preset) return null;
  return {
    id: newUuid(),
    name: preset.name,
    apiFormat: preset.apiFormat,
    baseUrl: preset.baseUrl,
    local: preset.local,
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
