/**
 * 会话域 store（16 号票，FR-01-01/02）：多 Tab 状态 + 左栏数据面。
 *
 * - tabs：打开的 Tab（SessionMeta 快照，标题/计数条显示用）；activeId 当前 Tab；
 * - projects/sessions：左栏列表（list_projects / list_sessions 全量，操作后刷新）；
 * - running：Tab 运行指示（SessionTab 内 useThread 桥进 store）；
 * - ui-state：Tab 布局持久化（{ tabIds, activeId }，§3.3 #30/#31；草稿不落盘）。
 */

import { create } from "zustand";
import * as client from "@/ipc/client";
import type { ProjectMeta, SessionMeta } from "@/ipc/types";

export interface UiStatePayload {
  tabIds: string[];
  activeId: string | null;
  /** run 结束自动折叠开关（§10.4/FR-01-10 可配置；缺省 true）。 */
  collapseRuns?: boolean;
  /** 右栏 Inspector 开合（FR-13-01；缺省 false = 收起）。 */
  inspectorOpen?: boolean;
  /** 会话草稿（§10.6 按 Tab 存；sessionId → 输入框文本，缺省无）。 */
  drafts?: Record<string, string>;
}

export interface SessionsStore {
  /** 打开的 Tab（按打开序）。 */
  tabs: SessionMeta[];
  /** 当前活跃 Tab（null = 无）。 */
  activeId: string | null;
  /** 左栏：项目注册表。 */
  projects: ProjectMeta[];
  /** 左栏：会话全量（updatedAt 倒序，Rust 侧排序）。 */
  sessions: SessionMeta[];
  /** Tab 运行指示（SessionTab 桥入）。 */
  running: Record<string, boolean>;
  /** 启动恢复已就绪（首帧渲染前避免闪建 Tab）。 */
  ready: boolean;
  /** run 结束自动折叠开关（§10.4，ui-state 持久化）。 */
  collapseRuns: boolean;
  /** 右栏 Inspector 开合（FR-13-01，ui-state 持久化）。 */
  inspectorOpen: boolean;
  /** 会话草稿（§10.6，ui-state 持久化；写侧防抖由 persistUiState 承担）。 */
  drafts: Record<string, string>;

  /** 启动恢复：并行拉项目/会话/ui-state，重建 Tab 布局（空则兜底最新会话或新建）。 */
  bootstrap: () => Promise<void>;
  /** 左栏数据刷新（项目 + 会话）。 */
  refresh: () => Promise<void>;
  /** 打开 Tab（已在则仅激活）；ui-state 落盘。 */
  openTab: (meta: SessionMeta) => void;
  /** 激活 Tab；ui-state 落盘。 */
  setActive: (sessionId: string) => void;
  /** 关闭 Tab（活跃 Tab 关闭时自动右移左补）；ui-state 落盘。 */
  closeTab: (sessionId: string) => void;
  /** 会话标题/计数的就地更新（run 期间 index 推进后由调用方刷新）。 */
  patchTab: (meta: SessionMeta) => void;
  /** 运行指示桥（SessionTab 内写）。 */
  setRunning: (sessionId: string, running: boolean) => void;
  /** 自动折叠开关切换（FR-01-10）。 */
  setCollapseRuns: (on: boolean) => void;
  /** 右栏开合切换（FR-13-01）。 */
  setInspectorOpen: (open: boolean) => void;
  /** 草稿写入（sessionId 维度替换；空串清除该键）。 */
  setDraft: (sessionId: string, text: string) => void;
  /** Tab 布局落盘（setUiState；失败静默——布局可丢，数据不可）。 */
  persistUiState: () => void;
}

/** 落盘防抖句柄（模块级：整份替换，高频切换时只保留最后一次）。 */
let persistTimer: ReturnType<typeof setTimeout> | null = null;

export const useSessionsStore = create<SessionsStore>((set, get) => ({
  tabs: [],
  activeId: null,
  projects: [],
  sessions: [],
  running: {},
  ready: false,
  collapseRuns: true,
  inspectorOpen: false,
  drafts: {},

  bootstrap: async () => {
    const emptyUiState: UiStatePayload = { tabIds: [], activeId: null };
    const [projects, sessions, uiState] = await Promise.all([
      client.listProjects(),
      client.listSessions(),
      client
        .getUiState()
        .then((raw) => (raw as UiStatePayload | null) ?? emptyUiState)
        .catch(() => emptyUiState),
    ]);
    // ui-state 里已删除的会话自动剔除
    const byId = new Map(sessions.map((m) => [m.id, m]));
    const tabs = uiState.tabIds
      .map((id) => byId.get(id))
      .filter((m): m is SessionMeta => m !== undefined);
    set({
      projects,
      sessions,
      tabs,
      ready: true,
      collapseRuns: uiState.collapseRuns ?? true,
      inspectorOpen: uiState.inspectorOpen ?? false,
      drafts: uiState.drafts ?? {},
    });
    if (tabs.length > 0) {
      const active = tabs.find((m) => m.id === uiState.activeId) ?? tabs[0];
      set({ activeId: active.id });
      get().persistUiState();
      return;
    }
    // 兜底：恢复最新会话，全新安装则建一个临时会话
    if (sessions.length > 0) {
      set({ tabs: [sessions[0]], activeId: sessions[0].id });
      get().persistUiState();
      return;
    }
    const created = await client.createSession();
    set({ tabs: [created], activeId: created.id });
    await get().refresh();
    get().persistUiState();
  },

  refresh: async () => {
    const [projects, sessions] = await Promise.all([client.listProjects(), client.listSessions()]);
    set((s) => ({
      projects,
      sessions,
      // Tab 快照同步刷新（左栏改名/删除后标题保持一致）
      tabs: s.tabs.map((tab) => sessions.find((m) => m.id === tab.id) ?? tab),
    }));
  },

  openTab: (meta) => {
    set((s) =>
      s.tabs.some((t) => t.id === meta.id)
        ? { activeId: meta.id, tabs: s.tabs.map((t) => (t.id === meta.id ? meta : t)) }
        : { tabs: [...s.tabs, meta], activeId: meta.id },
    );
    get().persistUiState();
  },

  setActive: (sessionId) => {
    set({ activeId: sessionId });
    get().persistUiState();
  },

  closeTab: (sessionId) => {
    set((s) => {
      const idx = s.tabs.findIndex((t) => t.id === sessionId);
      if (idx < 0) return s;
      const tabs = s.tabs.filter((t) => t.id !== sessionId);
      const activeId =
        s.activeId !== sessionId ? s.activeId : (tabs[Math.min(idx, tabs.length - 1)]?.id ?? null);
      return { tabs, activeId };
    });
    get().persistUiState();
  },

  patchTab: (meta) => {
    set((s) => ({ tabs: s.tabs.map((t) => (t.id === meta.id ? meta : t)) }));
  },

  setRunning: (sessionId, running) => {
    set((s) => ({ running: { ...s.running, [sessionId]: running } }));
  },

  setCollapseRuns: (on) => {
    set({ collapseRuns: on });
    get().persistUiState();
  },

  setInspectorOpen: (open) => {
    set({ inspectorOpen: open });
    get().persistUiState();
  },

  setDraft: (sessionId, text) => {
    set((s) => {
      const drafts = { ...s.drafts };
      if (text.length === 0) delete drafts[sessionId];
      else drafts[sessionId] = text;
      return { drafts };
    });
    get().persistUiState();
  },

  persistUiState: () => {
    if (persistTimer !== null) clearTimeout(persistTimer);
    persistTimer = setTimeout(() => {
      persistTimer = null;
      const { tabs, activeId, collapseRuns, inspectorOpen, drafts } = get();
      void client
        .setUiState({
          tabIds: tabs.map((t) => t.id),
          activeId,
          collapseRuns,
          inspectorOpen,
          drafts,
        } satisfies UiStatePayload)
        .catch((err) => console.warn("[ui-state] 布局落盘失败（忽略）", err));
    }, 300);
  },
}));
