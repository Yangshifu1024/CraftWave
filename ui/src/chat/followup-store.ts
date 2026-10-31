/**
 * steer / followUp 的前端状态面（§5.5，06 号票）。
 *
 * - steerNodes：steer 插话的时间线承载（§5.5 回写定稿——§3.4 无 user 消息帧，
 *   前端以 run:inject 回执本地插入带「插话」徽标的时间线节点；canonical 由
 *   Rust 侧同 id 落盘，树重载收养后本节点与之重合，按 entryId 去重防双显）；
 * - 队列条数据在 sessions store（queues/queuePaused，ui-state 持久化），
 *   本文件只放内存态的插话节点与跨 store 的动作编排（入队/删除/编辑取出/
 *   继续），供 composer-keys 与 Thread 共用；
 * - 动作函数的 IPC 依赖可注入（测试换假实现）。
 */

import { create } from "zustand";
import * as client from "@/ipc/client";
import type { RunConfig } from "@/ipc/types";
import { type QueuedFollowUp, useSessionsStore } from "@/stores/sessions";

/** 一条 steer 插话节点（时间线渲染面）。 */
export interface SteerNode {
  /** canonical 条目 id（{runId}:steer:{n}，树收养后与树节点同 id）。 */
  entryId: string;
  text: string;
  /** 消费时的轮次（徽标悬浮提示用）。 */
  acceptedAtTurn: number;
}

/** steer 插话节点仓库的形状。 */
export interface FollowUpState {
  /** sessionId → 插话节点（按消费序追加）。 */
  steerNodes: Record<string, SteerNode[]>;
  /** run:inject 回执登记（重复回执按 entryId 幂等）。 */
  addSteerNode: (sessionId: string, node: SteerNode) => void;
  /** 树收养后剔除已被 canonical 收养的节点（按 entryId 集合）。 */
  dropSteerNodes: (sessionId: string, entryIds: ReadonlySet<string>) => void;
  /** 会话关闭/删除时丢弃其分桶。 */
  forgetSession: (sessionId: string) => void;
}

/** steer 插话节点仓库实例。 */
export const useFollowUpStore = create<FollowUpState>((set) => ({
  steerNodes: {},

  addSteerNode: (sessionId, node) =>
    set((s) => {
      const list = s.steerNodes[sessionId] ?? [];
      if (list.some((n) => n.entryId === node.entryId)) return s;
      return { steerNodes: { ...s.steerNodes, [sessionId]: [...list, node] } };
    }),

  dropSteerNodes: (sessionId, entryIds) =>
    set((s) => {
      const list = s.steerNodes[sessionId];
      if (list === undefined) return s;
      const rest = list.filter((n) => !entryIds.has(n.entryId));
      if (rest.length === list.length) return s;
      const steerNodes = { ...s.steerNodes };
      if (rest.length === 0) delete steerNodes[sessionId];
      else steerNodes[sessionId] = rest;
      return { steerNodes };
    }),

  forgetSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.steerNodes)) return s;
      const steerNodes = { ...s.steerNodes };
      delete steerNodes[sessionId];
      return { steerNodes };
    }),
}));

/** followUp 动作的依赖面（全部可注入——测试换假实现）。 */
export interface FollowUpDeps {
  injectRunMessage: typeof client.injectRunMessage;
  removeFollowup: typeof client.removeFollowup;
  resumeFollowupQueue: typeof client.resumeFollowupQueue;
  pauseFollowupQueue: typeof client.pauseFollowupQueue;
}

/** 生产依赖：ipc/client 真实出口。 */
export const defaultFollowUpDeps: FollowUpDeps = {
  injectRunMessage: client.injectRunMessage,
  removeFollowup: client.removeFollowup,
  resumeFollowupQueue: client.resumeFollowupQueue,
  pauseFollowupQueue: client.pauseFollowupQueue,
};

/**
 * followUp 排队（Alt+Enter / 运行中带图入队）：调后端入队（queue.json 即刻
 * 持久化），本地队列条同步追加（run:queue 事件到达时以 size 校正）。返回
 * 队列条目（失败返回 null，输入由调用方保留处理）。attachments 为图片附件
 * 引用（17 号票，§5.5 带图不入 steer、经此通路进 followUp；投递时展开为
 * image part）。
 */
export async function queueFollowUp(
  sessionId: string,
  text: string,
  runConfig: RunConfig,
  attachments: { blobHash: string; mimeType: string }[] = [],
  deps: FollowUpDeps = defaultFollowUpDeps,
): Promise<QueuedFollowUp | null> {
  try {
    const result = await deps.injectRunMessage({
      sessionId,
      text,
      mode: "followUp",
      runConfig,
      ...(attachments.length > 0 ? { attachments } : {}),
    });
    if (!result.queued) return null;
    // entryId 由后端返回（队列条删除/编辑取出的回传键，§3.3 #21）
    const entry: QueuedFollowUp = { id: result.entryId ?? `local-${Date.now()}`, text };
    const store = useSessionsStore.getState();
    store.setQueue(sessionId, [...(store.queues[sessionId] ?? []), entry]);
    return entry;
  } catch (err) {
    console.error("[followUp] 排队失败", err);
    return null;
  }
}

/**
 * steer 插话（运行中 Enter）：调后端入插话队列（turn 边界消费，run:inject
 * 回执后时间线插入节点）。返回是否被接纳（未接纳保留输入框文本）。
 */
export async function sendSteer(
  sessionId: string,
  text: string,
  deps: FollowUpDeps = defaultFollowUpDeps,
): Promise<boolean> {
  try {
    const result = await deps.injectRunMessage({ sessionId, text });
    return result.queued && result.mode === "steer";
  } catch (err) {
    console.error("[steer] 插话失败", err);
    return false;
  }
}

/**
 * 移除一条待发（队列条 ×）：后端删除 + 本地队列条同步。返回是否成功
 * （失败保留条目）。编辑取出共用本函数（取出 = 删除 + 文本回填输入框）。
 */
export async function removeQueuedFollowUp(
  sessionId: string,
  entryId: string,
  deps: FollowUpDeps = defaultFollowUpDeps,
): Promise<boolean> {
  try {
    await deps.removeFollowup(sessionId, entryId);
  } catch (err) {
    console.error("[followUp] 移除失败", err);
    return false;
  }
  const store = useSessionsStore.getState();
  store.setQueue(
    sessionId,
    (store.queues[sessionId] ?? []).filter((q) => q.id !== entryId),
  );
  return true;
}

/**
 * 继续队列（失败暂停的明确继续入口）：后端解除暂停；delivered=true 时本地
 * 队列条清空由 run:queue 事件驱动（size=0），此处只负责调用。
 */
export async function resumeFollowUpQueue(
  sessionId: string,
  deps: FollowUpDeps = defaultFollowUpDeps,
): Promise<void> {
  try {
    await deps.resumeFollowupQueue(sessionId);
  } catch (err) {
    console.error("[followUp] 继续队列失败", err);
  }
}

/**
 * 主动暂停队列（33 号票 #59o，更新门配套）：用户在更新前须「明确暂停并
 * 持久保留队列」——后端置 queue.json paused=true 持久化；本地暂停横幅由
 * run:queue 事件回推驱动（与失败暂停同一展示接缝）。
 */
export async function pauseFollowUpQueue(
  sessionId: string,
  deps: FollowUpDeps = defaultFollowUpDeps,
): Promise<void> {
  try {
    await deps.pauseFollowupQueue(sessionId);
  } catch (err) {
    console.error("[followUp] 暂停队列失败", err);
  }
}
