// @vitest-environment happy-dom
/**
 * 全局热键编排测试（29 号票，FR-13-11）：归一化/显示格式化/录制捕获纯
 * 函数表驱动 + applyHotkey 的注册流（@tauri-apps/plugin-global-shortcut
 * 经 vi.mock 注入桩——不触真实系统热键）。原逐分支用例已合并为三个剧本，
 * 覆盖语义不减。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { useToastStore } from "@/stores/toast";
import {
  applyHotkey,
  eventToAccelerator,
  formatAccelerator,
  normalizeAccelerator,
  useHotkeyStore,
} from "./hotkey";

// 插件 JS API 测试桩（applyHotkey 的依赖面：注册/查询/注销）
const register = vi.fn<(shortcut: string, handler?: unknown) => Promise<void>>();
const unregisterAll = vi.fn<() => Promise<void>>();
const isRegistered = vi.fn<(shortcut: string) => Promise<boolean>>();

vi.mock("@tauri-apps/plugin-global-shortcut", () => ({
  register: (shortcut: string, handler?: unknown) => register(shortcut, handler),
  unregisterAll: () => unregisterAll(),
  isRegistered: (shortcut: string) => isRegistered(shortcut),
}));

beforeEach(() => {
  vi.resetAllMocks();
  register.mockResolvedValue(undefined);
  unregisterAll.mockResolvedValue(undefined);
  isRegistered.mockResolvedValue(true);
  useHotkeyStore.setState({ error: null });
  useToastStore.setState({ toasts: [] });
});

/** 造键盘事件（happy-dom 的 KeyboardEvent 初始化参数）。 */
const key = (init: {
  key: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
}): KeyboardEvent =>
  new KeyboardEvent("keydown", {
    key: init.key,
    ctrlKey: init.ctrlKey ?? false,
    altKey: init.altKey ?? false,
    shiftKey: init.shiftKey ?? false,
    metaKey: init.metaKey ?? false,
    bubbles: true,
  });

describe("hotkey 纯函数与注册流（29 号票 FR-13-11）", () => {
  it("normalizeAccelerator 归一化表驱动（与后端 normalize_accelerator 同规则）+ formatAccelerator 显示文案", () => {
    const cases: [string, string | null][] = [
      ["Alt+Space", "alt+space"],
      ["alt+space", "alt+space"],
      ["SHIFT+CTRL+P", "ctrl+shift+p"],
      ["p+shift+ctrl", "ctrl+shift+p"],
      ["Cmd+Space", "super+space"],
      ["Control+Alt+T", "ctrl+alt+t"],
      ["alt+alt+space", "alt+space"],
      ["alt + space", "alt+space"],
      ["F5", "f5"],
      ["", null],
      ["ctrl+shift", null],
      ["alt++space", null],
      ["ctrl+alt+del+esc", null],
    ];
    for (const [raw, expected] of cases) {
      expect(normalizeAccelerator(raw)).toBe(expected);
    }
    // 存储形态 → 人类可读（修饰符与键名首字母大写，特殊键映射）
    expect(formatAccelerator("alt+space")).toBe("Alt+Space");
    expect(formatAccelerator("ctrl+shift+p")).toBe("Ctrl+Shift+P");
    expect(formatAccelerator("super+k")).toBe("Super+K");
    expect(formatAccelerator("ctrl+f1")).toBe("Ctrl+F1");
  });

  it("eventToAccelerator 录制捕获表驱动：组合键收存储形态、纯修饰键/Esc/未知键返回 null 继续等待或取消", () => {
    const cases: [KeyboardEvent, string | null][] = [
      [key({ key: " ", altKey: true }), "alt+space"],
      [key({ key: "p", ctrlKey: true, shiftKey: true }), "ctrl+shift+p"],
      [key({ key: "F5" }), "f5"],
      [key({ key: "Alt", altKey: true }), null], // 纯修饰键：继续等待
      [key({ key: "Shift", shiftKey: true }), null],
      [key({ key: "Escape" }), null], // Esc 交给录制取消语义，不当成组合键
      [key({ key: "AudioVolumeUp" }), null], // 不认识的键（多媒体键等）不入配置
    ];
    for (const [event, expected] of cases) {
      expect(eventToAccelerator(event)).toBe(expected);
    }
  });

  it("applyHotkey 注册流剧本：开→注册清错、换键先注销再注册、关→注销幂等、冲突→错误态+Toast 只试一次、非法键→静默", async () => {
    // enabled=开 → 注册归一化后的组合键（带空 handler，动作在 Rust 侧）；成功清错误态
    await applyHotkey({ enabled: true, accelerator: "Alt+Space" });
    expect(register).toHaveBeenCalledWith("alt+space", expect.any(Function));
    expect(useHotkeyStore.getState().error).toBeNull();

    // 换键 → 先注销旧注册再注册新键（unregister-all 幂等）
    let unregistersBefore = unregisterAll.mock.calls.length;
    await applyHotkey({ enabled: true, accelerator: "ctrl+shift+p" });
    expect(unregisterAll.mock.calls.length - unregistersBefore).toBe(1);
    expect(register).toHaveBeenLastCalledWith("ctrl+shift+p", expect.any(Function));

    // enabled=关 → 注销不注册；重复调用幂等（registered 已清，短路）
    unregistersBefore = unregisterAll.mock.calls.length;
    await applyHotkey({ enabled: false, accelerator: "alt+space" });
    expect(unregisterAll.mock.calls.length - unregistersBefore).toBe(1);
    await applyHotkey({ enabled: false, accelerator: "alt+space" });
    expect(unregisterAll.mock.calls.length - unregistersBefore).toBe(1);
    expect(useHotkeyStore.getState().error).toBeNull();

    // 注册冲突 → 不抛出；错误态 + 错误 Toast（应用不崩、只尝试一次不重试）
    const registersBefore = register.mock.calls.length;
    register.mockRejectedValue(new Error("shortcut already registered"));
    await applyHotkey({ enabled: true, accelerator: "alt+space" });
    expect(useHotkeyStore.getState().error).toBe("shortcut already registered");
    const toast = useToastStore.getState().toasts[0];
    expect(toast?.kind).toBe("error");
    expect(toast?.title).toContain("被其他应用占用");
    expect(register).toHaveBeenCalledTimes(registersBefore + 1);

    // 非法组合键 → 防御性静默（保存边界已拒绝，这里不注册不报错）
    useHotkeyStore.setState({ error: null });
    useToastStore.setState({ toasts: [] });
    register.mockResolvedValue(undefined);
    const registersBeforeInvalid = register.mock.calls.length;
    await applyHotkey({ enabled: true, accelerator: "ctrl+shift" });
    expect(register).toHaveBeenCalledTimes(registersBeforeInvalid);
    expect(useToastStore.getState().toasts).toEqual([]);
  });
});
