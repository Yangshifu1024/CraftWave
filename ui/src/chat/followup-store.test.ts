// @vitest-environment happy-dom
/**
 * followUp 队列前端面测试（06 号票，FR-01-07）：
 * - routeEnterKey 三态路由（composer-keys 纯函数）；
 * - queueFollowUp / sendSteer / removeQueuedFollowUp 的 store 编排（IPC 注入）；
 * - sessions store 队列桶持久化往返（queues/queuePaused 经 ui-state）；
 * - run:queue 事件驱动的返还面（returnQueueToDraft）与 size 校正。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_RUN_CONFIG } from "@/chat/runtime";
import { useSessionsStore } from "@/stores/sessions";
import { routeEnterKey } from "./composer-keys";
import {
  type defaultFollowUpDeps,
  queueFollowUp,
  removeQueuedFollowUp,
  resumeFollowUpQueue,
  sendSteer,
  useFollowUpStore,
} from "./followup-store";

const injectRunMessage = vi.fn();
const removeFollowup = vi.fn();
const resumeFollowupQueue = vi.fn();

const deps = {
  injectRunMessage: (...args: Parameters<typeof injectRunMessage>) => injectRunMessage(...args),
  removeFollowup: (...args: Parameters<typeof removeFollowup>) => removeFollowup(...args),
  resumeFollowupQueue: (...args: Parameters<typeof resumeFollowupQueue>) =>
    resumeFollowupQueue(...args),
} satisfies typeof defaultFollowUpDeps;

beforeEach(() => {
  vi.resetAllMocks();
  useSessionsStore.setState({ queues: {}, queuePaused: {}, drafts: {} });
  useFollowUpStore.setState({ steerNodes: {} });
});

describe("routeEnterKey（Enter 三态路由，FR-01-07 两种语义）", () => {
  it("空闲 Enter=普通发送（none，交给原生）", () => {
    expect(routeEnterKey({ altKey: false, isRunning: false, isComposing: false })).toBe("none");
  });

  it("运行中 Enter=steer 插话", () => {
    expect(routeEnterKey({ altKey: false, isRunning: true, isComposing: false })).toBe("steer");
  });

  it("Alt+Enter=followUp 排队（空闲/运行中都可排）", () => {
    expect(routeEnterKey({ altKey: true, isRunning: false, isComposing: false })).toBe("followUp");
    expect(routeEnterKey({ altKey: true, isRunning: true, isComposing: false })).toBe("followUp");
  });

  it("IME 组合期一律 none（防误发）", () => {
    expect(routeEnterKey({ altKey: true, isRunning: true, isComposing: true })).toBe("none");
    expect(routeEnterKey({ altKey: false, isRunning: true, isComposing: true })).toBe("none");
  });
});

describe("queueFollowUp（Alt+Enter 排队编排）", () => {
  it("入队成功：本地队列条追加（entryId 取后端返回值）", async () => {
    injectRunMessage.mockResolvedValue({
      queued: true,
      mode: "followUp",
      entryId: "fu-abc",
    });
    const entry = await queueFollowUp("s1", "排队消息", DEFAULT_RUN_CONFIG, [], deps);
    expect(entry).toEqual({ id: "fu-abc", text: "排队消息" });
    expect(injectRunMessage).toHaveBeenCalledWith({
      sessionId: "s1",
      text: "排队消息",
      mode: "followUp",
      runConfig: DEFAULT_RUN_CONFIG,
    });
    expect(useSessionsStore.getState().queues.s1).toEqual([{ id: "fu-abc", text: "排队消息" }]);
  });

  it("未接纳（queued=false）：本地不入队，返回 null", async () => {
    injectRunMessage.mockResolvedValue({ queued: false, mode: undefined, entryId: undefined });
    const entry = await queueFollowUp("s1", "x", DEFAULT_RUN_CONFIG, [], deps);
    expect(entry).toBeNull();
    expect(useSessionsStore.getState().queues.s1).toBeUndefined();
  });

  it("IPC 失败：返回 null 保留现场（调用方保留输入框文本）", async () => {
    injectRunMessage.mockRejectedValue({ code: "E_INTERNAL", message: "x" });
    const entry = await queueFollowUp("s1", "x", DEFAULT_RUN_CONFIG, [], deps);
    expect(entry).toBeNull();
    expect(useSessionsStore.getState().queues.s1).toBeUndefined();
  });
});

describe("sendSteer（运行中 Enter 插话编排）", () => {
  it("被接纳返回 true；未接纳/失败返回 false", async () => {
    injectRunMessage.mockResolvedValue({ queued: true, mode: "steer" });
    expect(await sendSteer("s1", "插话", deps)).toBe(true);
    expect(injectRunMessage).toHaveBeenCalledWith({ sessionId: "s1", text: "插话" });
    injectRunMessage.mockResolvedValue({ queued: true, mode: "followUp" });
    expect(await sendSteer("s1", "插话", deps)).toBe(false);
    injectRunMessage.mockRejectedValue(new Error("x"));
    expect(await sendSteer("s1", "插话", deps)).toBe(false);
  });
});

describe("removeQueuedFollowUp（队列条删除/编辑取出共用）", () => {
  it("后端删除 + 本地同步移除", async () => {
    useSessionsStore.setState({
      queues: {
        s1: [
          { id: "fu-a", text: "一" },
          { id: "fu-b", text: "二" },
        ],
      },
    });
    removeFollowup.mockResolvedValue({ size: 1 });
    const ok = await removeQueuedFollowUp("s1", "fu-a", deps);
    expect(ok).toBe(true);
    expect(removeFollowup).toHaveBeenCalledWith("s1", "fu-a");
    expect(useSessionsStore.getState().queues.s1).toEqual([{ id: "fu-b", text: "二" }]);
  });

  it("后端失败：本地保留条目（返回 false）", async () => {
    useSessionsStore.setState({ queues: { s1: [{ id: "fu-a", text: "一" }] } });
    removeFollowup.mockRejectedValue({ code: "E_NOT_FOUND", message: "x" });
    expect(await removeQueuedFollowUp("s1", "fu-a", deps)).toBe(false);
    expect(useSessionsStore.getState().queues.s1).toEqual([{ id: "fu-a", text: "一" }]);
  });
});

describe("resumeFollowUpQueue（失败暂停的明确继续）", () => {
  it("调用后端继续命令；失败只记日志不抛", async () => {
    resumeFollowupQueue.mockResolvedValue({ runId: "r1", delivered: true });
    await resumeFollowUpQueue("s1", deps);
    expect(resumeFollowupQueue).toHaveBeenCalledWith("s1");
    resumeFollowupQueue.mockRejectedValue(new Error("x"));
    await expect(resumeFollowUpQueue("s1", deps)).resolves.toBeUndefined();
  });
});

describe("sessions store 队列桶（持久化往返 + 返还面）", () => {
  it("setQueue/setQueuePaused 写入；空数组/解除暂停清键", () => {
    const store = useSessionsStore.getState();
    store.setQueue("s1", [{ id: "fu-a", text: "一" }]);
    store.setQueuePaused("s1", true);
    expect(useSessionsStore.getState().queues.s1).toHaveLength(1);
    expect(useSessionsStore.getState().queuePaused.s1).toBe(true);
    useSessionsStore.getState().setQueue("s1", []);
    useSessionsStore.getState().setQueuePaused("s1", false);
    expect(useSessionsStore.getState().queues.s1).toBeUndefined();
    expect(useSessionsStore.getState().queuePaused.s1).toBeUndefined();
  });

  it("returnQueueToDraft：清队列 + returned 追加回草稿（原草稿保留）", () => {
    const store = useSessionsStore.getState();
    store.setDraft("s1", "正在输入的草稿");
    store.setQueue("s1", [
      { id: "fu-a", text: "甲" },
      { id: "fu-b", text: "乙" },
    ]);
    store.setQueuePaused("s1", true);
    store.returnQueueToDraft("s1", ["甲", "乙"]);
    const state = useSessionsStore.getState();
    expect(state.queues.s1).toBeUndefined();
    expect(state.queuePaused.s1).toBeUndefined();
    expect(state.drafts.s1).toBe("正在输入的草稿\n\n甲\n\n乙");
  });

  it("steer 节点：回执登记幂等 + dropSteerNodes 按树收养剔除", () => {
    const follow = useFollowUpStore.getState();
    follow.addSteerNode("s1", { entryId: "r:steer:1", text: "插话", acceptedAtTurn: 2 });
    follow.addSteerNode("s1", { entryId: "r:steer:1", text: "插话", acceptedAtTurn: 2 });
    expect(useFollowUpStore.getState().steerNodes.s1).toHaveLength(1);
    follow.dropSteerNodes("s1", new Set(["r:steer:1"]));
    expect(useFollowUpStore.getState().steerNodes.s1).toBeUndefined();
  });
});
