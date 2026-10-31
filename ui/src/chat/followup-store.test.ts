// @vitest-environment happy-dom
/**
 * followUp 队列前端面测试（06 号票，FR-01-07）：
 * - routeEnterKey 三态路由（composer-keys 纯函数）；
 * - queueFollowUp / sendSteer / removeQueuedFollowUp / resume / pause 的
 *   store 编排（IPC 注入，失败暂停/失败保留红线）；
 * - sessions store 队列桶持久化往返（queues/queuePaused 经 ui-state）、
 *   returnQueueToDraft 返还面与 steer 节点登记。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_RUN_CONFIG } from "@/chat/runtime";
import { useSessionsStore } from "@/stores/sessions";
import { routeEnterKey } from "./composer-keys";
import {
  type defaultFollowUpDeps,
  pauseFollowUpQueue,
  queueFollowUp,
  removeQueuedFollowUp,
  resumeFollowUpQueue,
  sendSteer,
  useFollowUpStore,
} from "./followup-store";

const injectRunMessage = vi.fn();
const removeFollowup = vi.fn();
const resumeFollowupQueue = vi.fn();
const pauseFollowupQueue = vi.fn();

const deps = {
  injectRunMessage: (...args: Parameters<typeof injectRunMessage>) => injectRunMessage(...args),
  removeFollowup: (...args: Parameters<typeof removeFollowup>) => removeFollowup(...args),
  resumeFollowupQueue: (...args: Parameters<typeof resumeFollowupQueue>) =>
    resumeFollowupQueue(...args),
  pauseFollowupQueue: (...args: Parameters<typeof pauseFollowupQueue>) =>
    pauseFollowupQueue(...args),
} satisfies typeof defaultFollowUpDeps;

beforeEach(() => {
  vi.resetAllMocks();
  useSessionsStore.setState({ queues: {}, queuePaused: {}, drafts: {} });
  useFollowUpStore.setState({ steerNodes: {} });
});

describe("followup-store（06 号票）", () => {
  it("routeEnterKey（Enter 三态路由）：空闲 none、运行中 steer、Alt+Enter followUp、IME 组合期一律 none", () => {
    // 空闲 Enter=普通发送（none，交给原生）；运行中 Enter=steer 插话
    expect(routeEnterKey({ altKey: false, isRunning: false, isComposing: false })).toBe("none");
    expect(routeEnterKey({ altKey: false, isRunning: true, isComposing: false })).toBe("steer");
    // Alt+Enter=followUp 排队（空闲/运行中都可排）
    expect(routeEnterKey({ altKey: true, isRunning: false, isComposing: false })).toBe("followUp");
    expect(routeEnterKey({ altKey: true, isRunning: true, isComposing: false })).toBe("followUp");
    // IME 组合期一律 none（防误发）
    expect(routeEnterKey({ altKey: true, isRunning: true, isComposing: true })).toBe("none");
    expect(routeEnterKey({ altKey: false, isRunning: true, isComposing: true })).toBe("none");
  });

  it("队列编排：queueFollowUp 入队/未接纳/失败、sendSteer 三态、remove 同步移除/失败保留、resume/pause 失败不抛", async () => {
    // ---- queueFollowUp：入队成功，本地队列条追加（entryId 取后端返回值）----
    injectRunMessage.mockResolvedValue({ queued: true, mode: "followUp", entryId: "fu-abc" });
    const entry = await queueFollowUp("s1", "排队消息", DEFAULT_RUN_CONFIG, [], deps);
    expect(entry).toEqual({ id: "fu-abc", text: "排队消息" });
    expect(injectRunMessage).toHaveBeenCalledWith({
      sessionId: "s1",
      text: "排队消息",
      mode: "followUp",
      runConfig: DEFAULT_RUN_CONFIG,
    });
    expect(useSessionsStore.getState().queues.s1).toEqual([{ id: "fu-abc", text: "排队消息" }]);

    // 未接纳（queued=false）：本地不入队，返回 null
    injectRunMessage.mockResolvedValue({ queued: false, mode: undefined, entryId: undefined });
    expect(await queueFollowUp("s1", "x", DEFAULT_RUN_CONFIG, [], deps)).toBeNull();
    // IPC 失败：返回 null 保留现场（调用方保留输入框文本）
    injectRunMessage.mockRejectedValue({ code: "E_INTERNAL", message: "x" });
    expect(await queueFollowUp("s1", "x", DEFAULT_RUN_CONFIG, [], deps)).toBeNull();
    expect(useSessionsStore.getState().queues.s1).toEqual([{ id: "fu-abc", text: "排队消息" }]);
    useSessionsStore.setState({ queues: {} });

    // ---- sendSteer：被接纳返回 true；未接纳/失败返回 false ----
    injectRunMessage.mockResolvedValue({ queued: true, mode: "steer" });
    expect(await sendSteer("s1", "插话", deps)).toBe(true);
    expect(injectRunMessage).toHaveBeenLastCalledWith({ sessionId: "s1", text: "插话" });
    injectRunMessage.mockResolvedValue({ queued: true, mode: "followUp" });
    expect(await sendSteer("s1", "插话", deps)).toBe(false);
    injectRunMessage.mockRejectedValue(new Error("x"));
    expect(await sendSteer("s1", "插话", deps)).toBe(false);

    // ---- removeQueuedFollowUp：后端删除 + 本地同步移除 ----
    useSessionsStore.setState({
      queues: {
        s1: [
          { id: "fu-a", text: "一" },
          { id: "fu-b", text: "二" },
        ],
      },
    });
    removeFollowup.mockResolvedValue({ size: 1 });
    expect(await removeQueuedFollowUp("s1", "fu-a", deps)).toBe(true);
    expect(removeFollowup).toHaveBeenCalledWith("s1", "fu-a");
    expect(useSessionsStore.getState().queues.s1).toEqual([{ id: "fu-b", text: "二" }]);
    // 后端失败：本地保留条目（返回 false）
    removeFollowup.mockRejectedValue({ code: "E_NOT_FOUND", message: "x" });
    expect(await removeQueuedFollowUp("s1", "fu-b", deps)).toBe(false);
    expect(useSessionsStore.getState().queues.s1).toEqual([{ id: "fu-b", text: "二" }]);

    // ---- resume（失败暂停的明确继续）/ pause（33 号票主动暂停）：失败只记日志不抛 ----
    resumeFollowupQueue.mockResolvedValue({ runId: "r1", delivered: true });
    await resumeFollowUpQueue("s1", deps);
    expect(resumeFollowupQueue).toHaveBeenCalledWith("s1");
    resumeFollowupQueue.mockRejectedValue(new Error("x"));
    await expect(resumeFollowUpQueue("s1", deps)).resolves.toBeUndefined();
    pauseFollowupQueue.mockResolvedValue({ size: 1, paused: true });
    await pauseFollowUpQueue("s1", deps);
    expect(pauseFollowupQueue).toHaveBeenCalledWith("s1");
    pauseFollowupQueue.mockRejectedValue(new Error("x"));
    await expect(pauseFollowUpQueue("s1", deps)).resolves.toBeUndefined();
  });

  it("sessions store 队列桶：写入与清键、returnQueueToDraft 返还草稿、steer 节点登记幂等与收养剔除", () => {
    // setQueue/setQueuePaused 写入；空数组/解除暂停清键
    const store = useSessionsStore.getState();
    store.setQueue("s1", [{ id: "fu-a", text: "一" }]);
    store.setQueuePaused("s1", true);
    expect(useSessionsStore.getState().queues.s1).toHaveLength(1);
    expect(useSessionsStore.getState().queuePaused.s1).toBe(true);
    useSessionsStore.getState().setQueue("s1", []);
    useSessionsStore.getState().setQueuePaused("s1", false);
    expect(useSessionsStore.getState().queues.s1).toBeUndefined();
    expect(useSessionsStore.getState().queuePaused.s1).toBeUndefined();

    // returnQueueToDraft：清队列 + returned 追加回草稿（原草稿保留）
    useSessionsStore.getState().setDraft("s1", "正在输入的草稿");
    useSessionsStore.getState().setQueue("s1", [
      { id: "fu-a", text: "甲" },
      { id: "fu-b", text: "乙" },
    ]);
    useSessionsStore.getState().setQueuePaused("s1", true);
    useSessionsStore.getState().returnQueueToDraft("s1", ["甲", "乙"]);
    const state = useSessionsStore.getState();
    expect(state.queues.s1).toBeUndefined();
    expect(state.queuePaused.s1).toBeUndefined();
    expect(state.drafts.s1).toBe("正在输入的草稿\n\n甲\n\n乙");

    // steer 节点：回执登记幂等 + dropSteerNodes 按树收养剔除
    const follow = useFollowUpStore.getState();
    follow.addSteerNode("s1", { entryId: "r:steer:1", text: "插话", acceptedAtTurn: 2 });
    follow.addSteerNode("s1", { entryId: "r:steer:1", text: "插话", acceptedAtTurn: 2 });
    expect(useFollowUpStore.getState().steerNodes.s1).toHaveLength(1);
    follow.dropSteerNodes("s1", new Set(["r:steer:1"]));
    expect(useFollowUpStore.getState().steerNodes.s1).toBeUndefined();
  });
});
