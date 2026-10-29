/**
 * Composer 按键体系（§10.6 / FR-13-05，17 号票；06 号票起 Enter 三态路由）。
 *
 * 覆盖：`@` 路径补全菜单（search_workspace_paths 驱动，光标锚定，Tab 优先级
 * 菜单 > ghost hint）、↑↓ 空输入翻阅本人历史消息、Esc 停止运行/关闭浮层、
 * Shift+Tab 循环权限档（内存态）、Tab 接受 ghost hint。
 * Enter 路由（06 号票，FR-01-07 两种语义界面明确）：空闲 Enter=普通发送、
 * 运行中 Enter=steer 插话（inject_run_message，turn 边界消费不终止 run）、
 * Alt+Enter=followUp 排队（任何时刻可排，run 正常完成后自动投递）；
 * Shift+Enter 换行 / IME 组合期防误发由 ComposerPrimitive.Input 原生承担
 * （0.15.31 内建 isComposing 检查，17 号票实证）。
 *
 * 同时承担草稿记账：composer 文本变化即写 sessions store（落盘防抖在
 * store 的 persistUiState）——§10.6「草稿按 Tab 存 ui-state」。
 */

import { useAui, useAuiState } from "@assistant-ui/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { queueFollowUp, sendSteer } from "@/chat/followup-store";
import { tierOf, usePermissionTierStore } from "@/chat/permission-tier";
import { DEFAULT_RUN_CONFIG } from "@/chat/runtime";
import { modelOf } from "@/chat/session-model";
import { useSessionScope } from "@/chat/session-scope";
import { cancelRun, searchWorkspacePaths } from "@/ipc/client";
import type { WorkspacePathHit } from "@/ipc/types";
import { matchGhost, useAskStore } from "@/stores/ask";
import { useSessionsStore } from "@/stores/sessions";

/** @ 菜单快照（浮层渲染面）。 */
export interface AtMenuState {
  query: string;
  items: WorkspacePathHit[];
  activeIndex: number;
}

/**
 * Enter 键的三态路由判定（纯函数，FR-01-07 两种语义界面明确）：
 * - Alt+Enter → followUp 排队（任何时刻可排，含运行中）；
 * - 运行中 Enter → steer 插话（注入当前 run，不终止）；
 * - 其余 → none（空闲 Enter 的普通发送 / Shift+Enter 换行交给
 *   ComposerPrimitive.Input 原生处理）。
 * IME 组合期（isComposing）一律 none——防误发与原生口径一致。
 */
export function routeEnterKey(opts: {
  altKey: boolean;
  isRunning: boolean;
  isComposing: boolean;
}): "steer" | "followUp" | "none" {
  if (opts.isComposing) return "none";
  if (opts.altKey) return "followUp";
  if (opts.isRunning) return "steer";
  return "none";
}

/** 从 (text, cursor) 检测未闭合的 `@` 触发（§10.6 光标锚定）：
 *  光标往回到最近 '@' 之间无空白即视为路径查询中，返回插入位与查询词。 */
export function detectAtQuery(text: string, cursor: number): { at: number; query: string } | null {
  let i = cursor - 1;
  while (i >= 0) {
    const ch = text[i];
    if (ch === "@") break;
    if (/\s/.test(ch)) return null; // @ 与光标间隔了空白：非触发态
    i -= 1;
  }
  if (i < 0) return null;
  return { at: i, query: text.slice(i + 1, cursor) };
}

/** 补全项的插入文本（目录补全保留 @ 语义，文件同样直接插入相对路径）。 */
export function insertAtCompletion(text: string, at: number, cursor: number, path: string): string {
  return `${text.slice(0, at + 1)}${path} ${text.slice(cursor)}`;
}

/**
 * Composer 键盘体系主 hook：返回输入框要挂的 onKeyDown/onCursor 回调、
 * @ 补全菜单状态（atMenu）与菜单交互方法（pickActive 选中/hoverIndex 悬停）。
 * 同时内部承担两件记账：@ 菜单的触发检测与防抖搜索、草稿实时写入会话 store。
 */
export function useComposerKeys(): {
  onKeyDown: (event: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  onCursor: (cursor: number) => void;
  atMenu: AtMenuState | null;
  pickActive: () => void;
  hoverIndex: (index: number) => void;
} {
  const sessionId = useSessionScope();
  const aui = useAui();
  const text = useAuiState((s) => s.composer.text);
  const isRunning = useAuiState((s) => s.thread.isRunning);
  const threadMessages = useAuiState((s) => s.thread.messages);
  // 本人历史输入（时间序，非空文本）——翻阅用；useMemo 派生避免 selector 引用抖动
  const userMessages = useMemo(
    () =>
      threadMessages
        .filter((m) => m.role === "user")
        .map((m) => m.content.map((p) => (p.type === "text" ? p.text : "")).join(""))
        .filter((t) => t.length > 0),
    [threadMessages],
  );
  const hint = useAskStore((s) => s.hints[sessionId]);
  const clearHint = useAskStore((s) => s.clearHint);
  const cycleTier = usePermissionTierStore((s) => s.cycle);
  const setDraft = useSessionsStore((s) => s.setDraft);

  /** 报告当前光标位置（onKeyDown/onKeyUp/onSelect 都会回调；存 ref 快照，
   *  供 @ 触发检测与补全插入使用，不参与渲染）。 */
  const cursorRef = useRef(0);
  const onCursor = useCallback((cursor: number) => {
    cursorRef.current = cursor;
  }, []);

  // ---- @ 菜单状态机 ----
  const [atMenu, setAtMenu] = useState<AtMenuState | null>(null);
  // 文本变化 → 重检测触发态（光标取 ref 快照）；查询词变化 → 防抖拉补全。
  // detected 在 effect 内现算，避免对象引用进依赖数组。
  useEffect(() => {
    const detected = detectAtQuery(text, cursorRef.current);
    if (detected === null) {
      setAtMenu(null);
      return;
    }
    setAtMenu((prev) =>
      prev !== null && prev.query === detected.query
        ? prev
        : { query: detected.query, items: [], activeIndex: 0 },
    );
    const timer = setTimeout(() => {
      void searchWorkspacePaths(sessionId, detected.query, 8)
        .then((items) => {
          setAtMenu((prev) => (prev !== null ? { ...prev, items, activeIndex: 0 } : prev));
        })
        .catch(() => {
          // IPC 异常（纯浏览器等）：菜单留空，不打断输入
        });
    }, 150);
    return () => clearTimeout(timer);
  }, [text, sessionId]);

  /** 插入当前选中项（键盘 Tab/Enter 与浮层点击共用）。 */
  const pickActive = useCallback(() => {
    const snapshot = atMenu;
    if (snapshot === null) return;
    const hit = snapshot.items[snapshot.activeIndex];
    if (hit !== undefined) {
      const at = cursorRef.current;
      aui.composer.setText(
        insertAtCompletion(text, detectAtQuery(text, at)?.at ?? at - 1, at, hit.path),
        {
          origin: "user",
        },
      );
    }
    setAtMenu(null);
  }, [aui, atMenu, text]);

  // ---- 历史翻阅（↑↓ 空输入，§10.6）----
  const historyRef = useRef<{ index: number; draft: string }>({ index: -1, draft: "" });

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    onCursor(event.currentTarget.selectionStart ?? 0);
    // 1) @ 菜单导航（Tab 优先级：菜单 > ghost hint，FR-13-05）
    if (atMenu !== null) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const delta = event.key === "ArrowDown" ? 1 : -1;
        setAtMenu((prev) =>
          prev === null || prev.items.length === 0
            ? prev
            : {
                ...prev,
                activeIndex: (prev.activeIndex + delta + prev.items.length) % prev.items.length,
              },
        );
        return;
      }
      if (event.key === "Tab" || event.key === "Enter") {
        event.preventDefault();
        pickActive();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setAtMenu(null);
        return;
      }
    }
    // 2) ↑↓ 空输入：翻阅本人历史（Esc/清空退出翻阅恢复原稿）
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && text.length === 0) {
      const stack = userMessages.slice().reverse();
      if (stack.length === 0) return;
      event.preventDefault();
      const h = historyRef.current;
      if (h.index === -1) h.draft = text;
      const next = event.key === "ArrowUp" ? h.index + 1 : h.index - 1;
      if (next < 0) {
        historyRef.current = { index: -1, draft: "" };
        aui.composer.setText(h.draft, { origin: "user" });
        return;
      }
      const clamped = Math.min(next, stack.length - 1);
      historyRef.current = { ...h, index: clamped };
      aui.composer.setText(stack[clamped], { origin: "user" });
      return;
    }
    // 3) Tab 接受 ghost hint（§10.4）
    if (event.key === "Tab" && !event.shiftKey && matchGhost(hint ?? null, text) !== null) {
      event.preventDefault();
      aui.composer.setText(hint ?? "", { origin: "user" });
      clearHint(sessionId);
      return;
    }
    // 4) Shift+Tab 循环权限档（内存态，§10.6）
    if (event.key === "Tab" && event.shiftKey) {
      event.preventDefault();
      cycleTier(sessionId);
      return;
    }
    // 5) Enter 三态路由（06 号票，FR-01-07）：Alt+Enter=followUp 排队、
    //    运行中 Enter=steer 插话；空闲 Enter（普通发送）与 Shift+Enter（换行）
    //    不拦截，交给 ComposerPrimitive.Input 原生。两种排队语义排队成功后
    //    清空输入框（steer 回执后时间线插节点，followUp 进队列条）。
    if (event.key === "Enter" && !event.shiftKey) {
      const route = routeEnterKey({
        altKey: event.altKey,
        isRunning,
        isComposing: event.nativeEvent.isComposing,
      });
      const body = text.trim();
      if (route === "none" || body.length === 0) return;
      event.preventDefault();
      if (route === "steer") {
        void sendSteer(sessionId, body).then((accepted) => {
          if (accepted) aui.composer.setText("", { origin: "user" });
          // 未接纳（防御态）：保留输入框文本，不打断输入
        });
        return;
      }
      const runConfig = {
        ...DEFAULT_RUN_CONFIG,
        tier: tierOf(sessionId),
        ...(modelOf(sessionId) !== undefined ? { modelId: modelOf(sessionId) } : {}),
      };
      void queueFollowUp(sessionId, body, runConfig).then((queued) => {
        if (queued !== null) aui.composer.setText("", { origin: "user" });
      });
      return;
    }
    // 6) Esc：停止运行（终态以 runEnded 帧为权威，S5 语义）
    if (event.key === "Escape") {
      if (isRunning) {
        event.preventDefault();
        void cancelRun(sessionId).catch(() => {});
      } else if (hint != null) {
        clearHint(sessionId);
      }
    }
  };

  // ---- 草稿记账（§10.6 按 Tab 存 ui-state；空串自动清除该键） ----
  useEffect(() => {
    setDraft(sessionId, text);
  }, [sessionId, text, setDraft]);

  /** 浮层 hover 高亮（键盘 ↑↓ 导航共享 activeIndex）。 */
  const hoverIndex = useCallback((index: number) => {
    setAtMenu((prev) => (prev === null ? prev : { ...prev, activeIndex: index }));
  }, []);

  return { onKeyDown, onCursor, atMenu, pickActive, hoverIndex };
}
