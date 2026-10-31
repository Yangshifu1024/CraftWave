/**
 * 操作反馈 Toast store（反馈统一为顶部轻提示）。
 *
 * 设计口径：
 * - 轻提示 = 不打断输入焦点、不抢按键：store 只管数据与出队
 *   计时，渲染面（ToastHost）容器的骨架不接收指针事件，只有卡片本体可点；
 * - 自动出队：成功/信息 3 秒、错误 6 秒（错误要看清/可点开详情，停留更久），
 *   push 时由 store 排定时器（vitest 用 fake timers 可确定性地测出队）；
 * - 同屏最多 3 条堆叠，超出挤掉最老的（连续操作的反馈洪峰不刷屏）；
 * - 错误可带 detail（完整原因），ToastHost 点击错误卡展开 Dialog 详情。
 */

import { create } from "zustand";

/** 提示种类：success 操作成功 / error 失败（可展开详情）/ info 中性提示。 */
export type ToastKind = "success" | "error" | "info";

/** 单条提示。 */
export interface ToastItem {
  /** 自增 id（dismiss 与测试定位用）。 */
  id: number;
  kind: ToastKind;
  /** 一行主文案。 */
  title: string;
  /** 完整详情（错误卡的「查看详情」展开面；可空）。 */
  detail?: string;
  /** 自动消失毫秒数；缺省按 kind 取 DEFAULT_DURATION_BY_KIND。 */
  duration?: number;
}

/** 各类提示的缺省停留时长（毫秒：成功/信息 3s、错误 6s）。 */
export const DEFAULT_DURATION_BY_KIND: Record<ToastKind, number> = {
  success: 3000,
  info: 3000,
  error: 6000,
};

/** 同屏堆叠上限（超出挤掉最老的）。 */
export const MAX_STACK = 3;

interface ToastStore {
  /** 当前在屏提示（末尾最新）。 */
  toasts: ToastItem[];
  /** 入队一条提示，返回其 id。 */
  push: (toast: Omit<ToastItem, "id">) => number;
  /** 出队（自动到期或手动关闭/点击）。 */
  dismiss: (id: number) => void;
}

/** 出队定时器表（模块级：store 单例，timer 跟 id 走）。 */
const timers = new Map<number, ReturnType<typeof setTimeout>>();
/** 自增 id 计数。 */
let nextId = 1;

export const useToastStore = create<ToastStore>((set, get) => ({
  toasts: [],

  push: ({ kind, title, detail, duration }) => {
    const id = nextId;
    nextId += 1;
    // 堆叠上限：slice(-MAX_STACK) 保留最新 3 条，最老的先出（被挤掉的
    // 条目残留的 timer 到期时 dismiss 一个不存在的 id，无副作用）
    set((s) => ({
      toasts: [...s.toasts, { id, kind, title, detail, duration }].slice(-MAX_STACK),
    }));
    timers.set(
      id,
      setTimeout(() => get().dismiss(id), duration ?? DEFAULT_DURATION_BY_KIND[kind]),
    );
    return id;
  },

  dismiss: (id) => {
    const timer = timers.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      timers.delete(id);
    }
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
  },
}));

/**
 * 便捷入队入口（非 hook 场景：事件桥、IPC 回调、纯函数流程里 push）。
 * 组件内也可直接 useToastStore((s) => s.push)。
 */
export function pushToast(kind: ToastKind, title: string, detail?: string): number {
  return useToastStore.getState().push({ kind, title, detail });
}
