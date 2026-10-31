/**
 * 灰区命令审计 store（§10.1 zustand，36 号票）：工具卡审计行的共享数据面。
 *
 * 数据源单通道：toolAudit 帧（§3.4 五态，§8.7）——实时（runtime onFrame）
 * 与重连重放（run-reconnect）两条路径都经过，经 [`applyAuditFrame`] 分发；
 * WebView 重载后审计状态由 attach 重放恢复。按 (sessionId, callId) 存取
 * （帧面 callId 在单 run 内唯一，跨 run 以会话分桶隔离）。
 *
 * 审计状态与人工审批相互独立（ADR-0002）：allow/block/ask/manual 各态的
 * 展示文案见 CommandCard 与 AskPanel（ask/manual 同时随审批载荷 audit
 * 字段展示——帧与载荷双通道携带同一结论）。
 */

import { create } from "zustand";
import type { AuditState } from "@/ipc/generated/AuditState";
import type { ChatFrame } from "@/ipc/generated/ChatFrame";

/** 一次灰区命令审计的展示记录（工具卡审计行）。 */
export interface ToolAuditInfo {
  /** 审计五态（running/allow/block/ask/manual）。 */
  state: AuditState;
  /** 简短依据（allow/ask/block）或失败原因（manual）；running 态缺省。 */
  reason?: string;
}

/** 按会话分桶的审计记录表（键 = 帧面 callId）。 */
interface AuditsState {
  bySession: Record<string, Record<string, ToolAuditInfo>>;
}

/** 审计状态仓库实例。 */
export const useAuditsStore = create<AuditsState>(() => ({
  bySession: {},
}));

/** toolAudit 帧分发（runtime 实时路径与 run-reconnect 重放路径共用）：
 * 写入 (sessionId, callId) 桶；非 toolAudit 帧忽略。 */
export function applyAuditFrame(sessionId: string, frame: ChatFrame): void {
  if (frame.type !== "toolAudit") return;
  const state = useAuditsStore.getState();
  const bucket = state.bySession[sessionId] ?? {};
  useAuditsStore.setState({
    bySession: {
      ...state.bySession,
      [sessionId]: {
        ...bucket,
        [frame.callId]: { state: frame.state, reason: frame.reason },
      },
    },
  });
}

/** 读取某会话某调用的审计记录（无记录返回 undefined）。 */
export function auditOf(
  bySession: Record<string, Record<string, ToolAuditInfo>>,
  sessionId: string,
  callId: string,
): ToolAuditInfo | undefined {
  return bySession[sessionId]?.[callId];
}
