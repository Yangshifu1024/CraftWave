/**
 * 会话模型的视觉能力判定：ProviderModel.vision（「可空 =
 * 未知」）的前端读取面——附件按钮置灰、拖拽/粘贴拦截提示、消息气泡内
 * 「当前模型无法查看历史图片」提示共用同一判定。
 *
 * 数据面：模型清单来自 get_config（会话覆盖模型 id 来自 session-model 的
 * 内存态）。缓存挂在 zustand store：ComposerBar 挂载/会话切换时刷新一次
 * （设置页改 vision 标记后重开会话生效——M1 简化，与 ModelPicker 的
 * 打开时拉取同节奏）。
 *
 * 判定口径与后端 resolve_model 一致：会话覆盖 > 全局 activeModelId；
 * 模型未标记 vision 或查不到 → false（保守：按不支持图片处理）。
 */

import { create } from "zustand";
import { getConfig } from "@/ipc/client";
import { modelOf } from "./session-model";

interface VisionState {
  /** 模型清单缓存：activeModelId + modelId → vision。 */
  activeModelId: string | undefined;
  byModel: Record<string, boolean>;
  /** 拉取配置刷新缓存（IPC 失败保留旧缓存，不打断输入）。 */
  refresh: () => Promise<void>;
}

export const useVisionStore = create<VisionState>((set) => ({
  activeModelId: undefined,
  byModel: {},
  refresh: async () => {
    try {
      const config = await getConfig();
      const byModel: Record<string, boolean> = {};
      for (const provider of config.providers) {
        for (const model of provider.models ?? []) {
          byModel[model.id] = model.vision ?? false;
        }
      }
      set({ activeModelId: config.activeModelId, byModel });
    } catch {
      // IPC 异常（纯浏览器等）：保留旧缓存
    }
  },
}));

/**
 * 当前会话生效模型是否支持图片输入（前端拦截面）：会话覆盖模型优先，
 * 全局默认兜底；未标记/未知模型按 false（与后端 resolve_model 的
 * unwrap_or(false) 同口径——前端置灰与后端拒绝不会互相矛盾）。
 */
export function visionOf(sessionId: string): boolean {
  const { activeModelId, byModel } = useVisionStore.getState();
  const wanted = modelOf(sessionId) ?? activeModelId;
  return wanted !== undefined ? (byModel[wanted] ?? false) : false;
}
