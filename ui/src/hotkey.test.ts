// @vitest-environment happy-dom
/**
 * 全局热键编排测试（29 号票，FR-13-11）：归一化/显示格式化/录制捕获纯
 * 函数表驱动 + applyHotkey 的注册流（@tauri-apps/plugin-global-shortcut
 * 经 vi.mock 注入桩——不触真实系统热键）。
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

describe("normalizeAccelerator（归一化，与后端 normalize_accelerator 同规则）", () => {
  it("表驱动：大小写/修饰符顺序/别名收编/重复去重", () => {
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
      expect(normalizeAccelerator(raw), `组合键「${raw}」`).toBe(expected);
    }
  });
});

describe("formatAccelerator（显示文案）", () => {
  it("存储形态 → 人类可读（修饰符与键名首字母大写，特殊键映射）", () => {
    expect(formatAccelerator("alt+space")).toBe("Alt+Space");
    expect(formatAccelerator("ctrl+shift+p")).toBe("Ctrl+Shift+P");
    expect(formatAccelerator("super+k")).toBe("Super+K");
    expect(formatAccelerator("ctrl+f1")).toBe("Ctrl+F1");
  });
});

describe("eventToAccelerator（录制捕获）", () => {
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

  it("组合键捕获为存储形态；纯修饰键返回 null 继续等待", () => {
    expect(eventToAccelerator(key({ key: " ", altKey: true }))).toBe("alt+space");
    expect(eventToAccelerator(key({ key: "p", ctrlKey: true, shiftKey: true }))).toBe(
      "ctrl+shift+p",
    );
    expect(eventToAccelerator(key({ key: "F5" }))).toBe("f5");
    expect(eventToAccelerator(key({ key: "Alt", altKey: true }))).toBeNull();
    expect(eventToAccelerator(key({ key: "Shift", shiftKey: true }))).toBeNull();
  });

  it("Esc 交给录制取消语义（返回 null，不当成组合键）", () => {
    expect(eventToAccelerator(key({ key: "Escape" }))).toBeNull();
  });

  it("不认识的键（多媒体键等）不入配置", () => {
    expect(eventToAccelerator(key({ key: "AudioVolumeUp" }))).toBeNull();
  });
});

describe("applyHotkey（注册流，插件 API 桩注入）", () => {
  it("enabled=开 → 注册归一化后的组合键（带空 handler，动作在 Rust 侧）；成功清错误态", async () => {
    await applyHotkey({ enabled: true, accelerator: "Alt+Space" });
    expect(register).toHaveBeenCalledWith("alt+space", expect.any(Function));
    expect(useHotkeyStore.getState().error).toBeNull();
  });

  it("换键 → 先注销旧注册再注册新键（unregister-all 幂等）", async () => {
    await applyHotkey({ enabled: true, accelerator: "alt+space" });
    const before = unregisterAll.mock.calls.length;
    await applyHotkey({ enabled: true, accelerator: "ctrl+shift+p" });
    // 本次换键恰好触发一次注销 + 一次注册（模块级注册记忆跨用例残留不影响差值断言）
    expect(unregisterAll.mock.calls.length - before).toBe(1);
    expect(register).toHaveBeenLastCalledWith("ctrl+shift+p", expect.any(Function));
  });

  it("enabled=关 → 注销不注册；重复调用幂等", async () => {
    await applyHotkey({ enabled: true, accelerator: "alt+space" });
    const before = unregisterAll.mock.calls.length;
    await applyHotkey({ enabled: false, accelerator: "alt+space" });
    expect(unregisterAll.mock.calls.length - before).toBe(1);
    // 再关一次：无新注销（registered 已清，短路）
    await applyHotkey({ enabled: false, accelerator: "alt+space" });
    expect(unregisterAll.mock.calls.length - before).toBe(1);
    expect(useHotkeyStore.getState().error).toBeNull();
  });

  it("注册冲突 → 不抛出；错误态 + 错误 Toast（应用不崩、不重试）", async () => {
    register.mockRejectedValue(new Error("shortcut already registered"));
    await applyHotkey({ enabled: true, accelerator: "alt+space" });
    expect(useHotkeyStore.getState().error).toBe("shortcut already registered");
    const toast = useToastStore.getState().toasts[0];
    expect(toast?.kind).toBe("error");
    expect(toast?.title).toContain("被其他应用占用");
    // 只尝试一次（无重试循环）
    expect(register).toHaveBeenCalledTimes(1);
  });

  it("非法组合键 → 防御性静默（保存边界已拒绝，这里不注册不报错）", async () => {
    await applyHotkey({ enabled: true, accelerator: "ctrl+shift" });
    expect(register).not.toHaveBeenCalled();
    expect(useToastStore.getState().toasts).toEqual([]);
  });
});
