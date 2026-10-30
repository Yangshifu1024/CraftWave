/**
 * 会话偏好持久化（FR-14-05 / FR-02-07，18 号票）：前端对 set_session_prefs
 * 的统一写入口——防抖合并 + 会话进入时的恢复链路。
 *
 * 为什么要防抖：Composer thinking 钮与 Ctrl+L 模型选择都是高频交互面
 * （连续点按/滚动选择），每次变更立即 invoke 会刷出大量碎片写盘；本模块
 * 把同一会话短时间内的多字段变更合并进一次 set_session_prefs（票面定案
 * 「变更时 set_session_prefs 落盘（防抖）」）。
 *
 * merge 语义（§3.3 #25）：载荷字段缺省 = 不动、null = 清除、设值 = 覆盖。
 * 防抖窗口内同字段多次变更只保留最后值（后写覆盖先写，与用户感知一致）。
 *
 * 恢复链路（票面「进入会话 get_session_prefs → 回填」）：restoreSessionPrefs
 * 由 SessionTab 挂载时调用，回填模型与 thinking 两个内存态 store——恢复走
 * store 纯方法（不经防抖写回），避免「读取 → 原样写回」的冗余落盘。
 */

import { getSessionPrefs, setSessionPrefs } from "@/ipc/client";
import type { SetSessionPrefsPayload } from "@/ipc/types";
import { useSessionModelStore } from "./session-model";
import { useThinkingStore } from "./thinking-level";

/** 防抖窗口（毫秒）：与 UI 连续交互的节奏匹配——快速连续切换只落最后一次。 */
export const PREFS_DEBOUNCE_MS = 300;

/** 每会话的待写缓冲与定时器（模块级：防抖状态不随组件卸载丢失）。 */
interface PendingPrefs {
  patch: SetSessionPrefsPayload;
  timer: ReturnType<typeof setTimeout>;
}

const pending = new Map<string, PendingPrefs>();

/**
 * 提交一次会话偏好变更（防抖）：patch 合并进该会话的待写缓冲（同字段
 * 覆盖、不同字段共存），窗口内无新变更才真正 invoke。进程退出等极端
 * 场景最多丢最后一窗（300ms）的偏好——便利性数据可接受。
 */
export function persistSessionPrefs(sessionId: string, patch: SetSessionPrefsPayload): void {
  const existing = pending.get(sessionId);
  if (existing !== undefined) {
    clearTimeout(existing.timer);
  }
  const merged: SetSessionPrefsPayload = {
    ...existing?.patch,
    ...patch,
  };
  const timer = setTimeout(() => {
    pending.delete(sessionId);
    void setSessionPrefs(sessionId, merged).catch(() => {
      // IPC 失败（会话刚删除等）静默放弃：偏好是便利性数据，内存态仍有效
    });
  }, PREFS_DEBOUNCE_MS);
  pending.set(sessionId, { patch: merged, timer });
}

/** 立即冲刷某会话的待写偏好（测试与卸载路径用；无待写时为空操作）。 */
export function flushSessionPrefs(sessionId: string): void {
  const existing = pending.get(sessionId);
  if (existing === undefined) return;
  clearTimeout(existing.timer);
  pending.delete(sessionId);
  void setSessionPrefs(sessionId, existing.patch).catch(() => {});
}

/** 测试隔离：清空全部防抖状态（不触发写入）。 */
export function resetPendingPrefs(): void {
  for (const { timer } of pending.values()) clearTimeout(timer);
  pending.clear();
}

/**
 * 进入会话的恢复链路（票面「恢复链路」条目）：get_session_prefs 读取
 * 持久化偏好 → 回填模型 store 与 thinking store（会话未显式设置的字段
 * 不动——保持「跟随全局默认 / per-model 回落」的缺省语义）。
 *
 * 恢复走 store 纯方法（不落盘）：数据来源就是盘，写回是无意义回环。
 * 会话不存在（E_NOT_FOUND，刚删除的竞态）等 IPC 失败静默忽略——偏好
 * 缺失不阻塞会话加载（FR-14-05 容错口径）。
 */
export async function restoreSessionPrefs(sessionId: string): Promise<void> {
  let prefs: Awaited<ReturnType<typeof getSessionPrefs>>;
  try {
    prefs = await getSessionPrefs(sessionId);
  } catch {
    return; // 读不到按空偏好处理（与后端容错口径一致）
  }
  if (prefs.modelId !== undefined) {
    useSessionModelStore.getState().setModel(sessionId, prefs.modelId);
  }
  if (prefs.thinking !== undefined) {
    useThinkingStore.getState().setLevel(sessionId, prefs.thinking);
  }
}
