// @vitest-environment happy-dom
/**
 * 会话偏好前端面测试：
 * - thinking store：五档循环顺序锚定、setLevel 内存态切换；
 * - setModelPersisted / setLevelPersisted：防抖合并落盘（set_session_prefs
 *   只 invoke 一次、同字段后写覆盖先写、null 清除语义）、flushSessionPrefs
 *   立即冲刷、各会话独立窗口；
 * - restoreSessionPrefs：进入会话回填两个 store、未设置字段不动、IPC 失败
 *   静默、不回写盘；
 * - Ctrl+L 打开信号 store：requestOpen/consumeOpen 的指向与消费。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setModelPersisted, useSessionModelStore } from "./session-model";
import { flushSessionPrefs, resetPendingPrefs, restoreSessionPrefs } from "./session-prefs";
import {
  setLevelPersisted,
  THINKING_LEVELS,
  thinkingLabel,
  thinkingOf,
  useThinkingStore,
} from "./thinking-level";

const setSessionPrefs = vi.fn();
const getSessionPrefs = vi.fn();

vi.mock("@/ipc/client", () => ({
  setSessionPrefs: (...args: unknown[]) => setSessionPrefs(...(args as [])),
  getSessionPrefs: (...args: unknown[]) => getSessionPrefs(...(args as [])),
}));

beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  // resetAllMocks 会清掉实现：补默认成功返回（flush 路径走 .catch 分支用）
  setSessionPrefs.mockResolvedValue(undefined);
  resetPendingPrefs();
  useSessionModelStore.setState({ bySession: {}, openRequest: null });
  useThinkingStore.setState({ bySession: {} });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("session-prefs", () => {
  it("thinking store（五档）：循环顺序锚定、五档均有用户可读名、setLevel 切换与清除", () => {
    // 循环顺序锚定
    expect(THINKING_LEVELS).toEqual(["off", "minimal", "low", "medium", "high"]);
    // 五档均有用户可读名（Composer 钮显示面不缺行）
    for (const level of THINKING_LEVELS) {
      expect(thinkingLabel(level).length).toBeGreaterThan(0);
    }
    // setLevel 切换内存态；undefined 清除（回 per-model 回落）
    useThinkingStore.getState().setLevel("sess-1", "high");
    expect(thinkingOf("sess-1")).toBe("high");
    useThinkingStore.getState().setLevel("sess-1", undefined);
    expect(thinkingOf("sess-1")).toBeUndefined();
  });

  it("防抖落盘：窗口合并一次取最后值、跨字段合并载荷、null 清除、flush 冲刷、各会话独立窗口", () => {
    // 窗口内多次变更只 invoke 一次，字段取最后值
    setLevelPersisted("sess-1", "low");
    setLevelPersisted("sess-1", "high");
    expect(setSessionPrefs).not.toHaveBeenCalled();
    vi.advanceTimersByTime(400);
    expect(setSessionPrefs).toHaveBeenCalledTimes(1);
    expect(setSessionPrefs).toHaveBeenCalledWith("sess-1", { thinking: "high" });

    // 不同字段合并进同一载荷（模型 + thinking 一次写）
    setModelPersisted("sess-2", "model-9");
    setLevelPersisted("sess-2", "minimal");
    vi.advanceTimersByTime(400);
    expect(setSessionPrefs).toHaveBeenCalledTimes(2);
    expect(setSessionPrefs).toHaveBeenLastCalledWith("sess-2", {
      modelId: "model-9",
      thinking: "minimal",
    });

    // 清除语义：undefined → null 载荷（merge 协议的清除面）
    setModelPersisted("sess-3", undefined);
    vi.advanceTimersByTime(400);
    expect(setSessionPrefs).toHaveBeenLastCalledWith("sess-3", { modelId: null });

    // flushSessionPrefs 立即冲刷待写（不等窗口）；冲刷后窗口期满不再重复写
    setLevelPersisted("sess-4", "medium");
    flushSessionPrefs("sess-4");
    expect(setSessionPrefs).toHaveBeenLastCalledWith("sess-4", { thinking: "medium" });
    vi.advanceTimersByTime(400);
    expect(setSessionPrefs).toHaveBeenCalledTimes(4);

    // 各会话独立防抖（互不合并、互不提前）
    setLevelPersisted("sess-a", "off");
    vi.advanceTimersByTime(100);
    setLevelPersisted("sess-b", "high");
    vi.advanceTimersByTime(250); // sess-a 已满 350ms、sess-b 才 250ms
    expect(setSessionPrefs).toHaveBeenCalledTimes(5);
    expect(setSessionPrefs).toHaveBeenLastCalledWith("sess-a", { thinking: "off" });
    vi.advanceTimersByTime(100);
    expect(setSessionPrefs).toHaveBeenCalledTimes(6);
    expect(setSessionPrefs).toHaveBeenLastCalledWith("sess-b", { thinking: "high" });
  });

  it("restoreSessionPrefs 回填与容错 + Ctrl+L 打开信号", async () => {
    // 持久化偏好回填模型与 thinking 两个 store
    getSessionPrefs.mockResolvedValue({ modelId: "model-1", thinking: "medium" });
    await restoreSessionPrefs("sess-r1");
    expect(getSessionPrefs).toHaveBeenCalledWith("sess-r1");
    expect(useSessionModelStore.getState().bySession["sess-r1"]).toBe("model-1");
    expect(thinkingOf("sess-r1")).toBe("medium");

    // 未设置的字段不动（保持「跟随默认」缺省语义）
    useSessionModelStore.getState().setModel("sess-r2", "keep-1");
    getSessionPrefs.mockResolvedValue({ thinking: "low" });
    await restoreSessionPrefs("sess-r2");
    expect(useSessionModelStore.getState().bySession["sess-r2"]).toBe("keep-1");
    expect(thinkingOf("sess-r2")).toBe("low");

    // 恢复不回写盘（防「读取→原样写回」回环）
    getSessionPrefs.mockResolvedValue({ modelId: "m", thinking: "off" });
    await restoreSessionPrefs("sess-r3");
    vi.advanceTimersByTime(1000);
    expect(setSessionPrefs).not.toHaveBeenCalled();

    // IPC 失败静默忽略（偏好缺失不阻塞会话加载，容错口径）
    getSessionPrefs.mockRejectedValue({ code: "E_NOT_FOUND", message: "会话不存在" });
    await expect(restoreSessionPrefs("sess-r4")).resolves.toBeUndefined();

    // Ctrl+L：requestOpen 记录目标会话；新请求覆盖旧请求（单打开信号，最后按键生效）
    useSessionModelStore.getState().requestOpen("sess-a");
    useSessionModelStore.getState().requestOpen("sess-b");
    expect(useSessionModelStore.getState().openRequest).toBe("sess-b");
    // 非目标会话消费不掉（openRequest 不指向它时保持）；目标会话消费置空
    useSessionModelStore.getState().consumeOpen("sess-other");
    expect(useSessionModelStore.getState().openRequest).toBe("sess-b");
    useSessionModelStore.getState().consumeOpen("sess-b");
    expect(useSessionModelStore.getState().openRequest).toBeNull();
  });
});
