/**
 * 全局热键编排（29 号票，FR-13-11/§10.7）：系统级一键唤起/隐藏主窗。
 *
 * 分层（与后端约定，见 Cargo.toml 插件注记）：快捷键的注册发起面在本模块
 * ——经官方插件 JS API（@tauri-apps/plugin-global-shortcut）invoke 插件命令，
 * 注册冲突（组合键被其他应用占用）的错误经 Promise 原生拿到，设置页红字
 * + Toast 零新增契约面；快捷键触发后的动作在 Rust 侧（lib.rs 注册插件时
 * 挂的 with_handler → shell::toggle_main_window），单一职责。
 *
 * 归一化（与后端 providers/types.rs::normalize_accelerator 同一套规则）：
 * 存储形态 = 小写、修饰符固定序 [ctrl, alt, shift, super] 在前 + 主键在后。
 * 保存边界（Rust normalize）是权威校验；前端归一化用于录制时预览与比较，
 * 两侧规则一致保证录制值与已存值可以比对去重。
 *
 * 幂等（applyHotkey）：WebView 重载/设置页保存后重复调用安全——先注销全部
 * 旧注册再按配置注册；enabled=false 或注册失败时完全不占系统热键资源、
 * 不进重试循环（失败态留在 store 由设置页呈现，下次保存再试）。
 */

import { isRegistered, register, unregisterAll } from "@tauri-apps/plugin-global-shortcut";
import { create } from "zustand";
import { t } from "@/i18n/core";
import { pushToast } from "@/stores/toast";

/** 修饰符固定序（存储形态；与后端 HOTKEY_MODIFIER_ORDER 一致）。 */
const MODIFIER_ORDER = ["ctrl", "alt", "shift", "super"] as const;

/** 修饰符别名 → 归一名（与后端 normalize_accelerator 同表）。 */
const MODIFIER_ALIASES: Record<string, string> = {
  ctrl: "ctrl",
  control: "ctrl",
  alt: "alt",
  option: "alt",
  opt: "alt",
  shift: "shift",
  super: "super",
  cmd: "super",
  command: "super",
  meta: "super",
  win: "super",
};

/** 录制键显示名映射（修饰符外的常见键在 UI 上的写法；其余保留原名）。 */
const KEY_LABELS: Record<string, string> = {
  space: "Space",
  enter: "Enter",
  escape: "Esc",
  tab: "Tab",
  backspace: "Backspace",
  delete: "Delete",
  arrowup: "↑",
  arrowdown: "↓",
  arrowleft: "←",
  arrowright: "→",
};

/**
 * 组合键归一化：非法（空串/空段/纯修饰符无主键/多主键/主键含空白）返回
 * null；合法返回存储形态（小写、修饰符固定序在前）。与后端保存边界的
 * 校验一致——前端只是提前给出相同结论，不承担权威。
 */
export function normalizeAccelerator(raw: string): string | null {
  const modifiers = new Set<string>();
  let mainKey: string | null = null;
  for (const segment of raw.split("+")) {
    const key = segment.trim().toLowerCase();
    if (key === "") return null; // "alt++space" 之类的空段
    const alias = MODIFIER_ALIASES[key];
    if (alias !== undefined) {
      modifiers.add(alias);
    } else if (mainKey === null && !/\s/.test(key)) {
      mainKey = key;
    } else {
      return null; // 第二个主键，或主键含空白
    }
  }
  if (mainKey === null) return null; // 纯修饰符组合
  const parts: string[] = MODIFIER_ORDER.filter((m) => modifiers.has(m));
  parts.push(mainKey);
  return parts.join("+");
}

/** 存储形态 → 显示文案（修饰符与键名首字母大写，"ctrl+shift+p" → "Ctrl+Shift+P"）。 */
export function formatAccelerator(normalized: string): string {
  return normalized
    .split("+")
    .map((part) => KEY_LABELS[part] ?? part.charAt(0).toUpperCase() + part.slice(1))
    .join("+");
}

/**
 * 键盘事件 → 存储形态组合键（设置页录制态的捕获入口）。纯修饰符按下
 * （还没按到主键）返回 null，录制态继续等待；Esc 不在此处理（录制态的
 * 取消语义由设置页拦截，避免「Esc 取消录制」被当成组合键）。
 */
export function eventToAccelerator(event: KeyboardEvent): string | null {
  // push 序即 MODIFIER_ORDER 固定序，主键天然在最后——无需再排序
  const modifiers: string[] = [];
  if (event.ctrlKey) modifiers.push("ctrl");
  if (event.altKey) modifiers.push("alt");
  if (event.shiftKey) modifiers.push("shift");
  if (event.metaKey) modifiers.push("super");
  let key = event.key.toLowerCase();
  // 修饰键自身按下时 event.key 是修饰键名——纯修饰符，等主键；
  // Esc 同样不入配置（录制态的取消语义专用，见 HotkeyRecorder）
  if (["control", "alt", "shift", "meta", "escape"].includes(key)) return null;
  // 单字符字母键统一小写（event.key 已小写；数字/符号原样）
  if (key === " ") key = "space";
  if (
    !/^([a-z0-9]|f[1-9]|f1[0-9]|f2[0-4]|space|tab|enter|backspace|delete|arrowup|arrowdown|arrowleft|arrowright|[`\-=[\];'\\,./])$/.test(
      key,
    )
  ) {
    return null; // 录制面不认识的键（多媒体键等）不入配置
  }
  modifiers.push(key);
  return modifiers.join("+");
}

/** 热键注册状态（设置页红字数据面；error 非 null = 最近一次注册失败）。 */
interface HotkeyState {
  /** 最近一次注册尝试的错误（人话文案；null = 当前配置注册正常）。 */
  error: string | null;
  setError: (error: string | null) => void;
}

export const useHotkeyStore = create<HotkeyState>((set) => ({
  error: null,
  setError: (error) => set({ error }),
}));

/** 当前已注册的组合键（模块级记忆：换键/关闭开关时先注销）。 */
let registered: string | null = null;

/**
 * 应用热键配置（幂等入口）：enabled=开 → 先注销旧注册再注册新键；关 →
 * 只注销。注册失败（被其他应用占用等）不抛出——错误进 store（设置页
 * 红字）+ Toast 提示，应用不崩、不重试。
 */
export async function applyHotkey(config: {
  enabled: boolean;
  accelerator: string;
}): Promise<void> {
  // 防御：空配置/格式非法按未启用处理（保存边界已拒绝非法值，这里兜底）
  const normalized = normalizeAccelerator(config.accelerator);
  if (normalized === null) {
    useHotkeyStore.getState().setError(null);
    return;
  }
  // 先注销既有注册（换键/关闭开关/WebView 重载重放，unregister-all 幂等）
  if (registered !== null) {
    const previous = registered;
    registered = null;
    try {
      if (await isRegistered(previous)) {
        await unregisterAll();
      }
    } catch {
      // 已不存在：视为注销完成
    }
  }
  if (!config.enabled) {
    useHotkeyStore.getState().setError(null);
    return;
  }
  try {
    // handler 传空函数（JS 绑定必填参数）：动作统一在 Rust with_handler
    // （toggle 主窗），JS 侧不做任何事
    await register(normalized, () => {});
    registered = normalized;
    useHotkeyStore.getState().setError(null);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    useHotkeyStore.getState().setError(message);
    pushToast(
      "error",
      t("toasts.hotkeyOccupied"),
      t("toasts.hotkeyOccupiedDetail", {
        accelerator: formatAccelerator(normalized),
        message,
      }),
    );
  }
}
