/**
 * 会话工作区（16 号票 FR-01-01/02 + 17 号票三栏 FR-13-01）：
 *
 * - 背景一体化：窗口/左栏/右栏同底色；会话区承载在四周留缝的四角圆角
 *   卡上，卡头显示项目/标题并放右栏折叠钮（Tab 条已移除——会话切换由
 *   左栏承担；左栏常开，顶条只为交通灯让位）；
 * - 左栏：临时会话区 + 项目分组会话树、新建（临时会话/项目/项目下会话）、
 *   标题本地过滤、重命名/删除、项目删除（名下会话转临时）、项目改名
 *   （16 号票 shortcut，17 号票收敛）；宽度可拖（min=初始宽，max=半屏）；
 * - 会话面：每会话一个 keep-mounted SessionTab（独立 runtime/草稿/运行态/
 *   滚动锚点；非活跃仅隐藏，run 帧照收）；
 * - 右栏 Inspector（可折叠，开合入 ui-state，FR-13-01；宽度可拖）；
 * - 宿主事件接线（17 号票）：app:exit_requested 退出确认（resolve_exit_request
 *   回执）、notify:activate 通知点击路由到对应会话。
 */

import {
  AssistantRuntimeProvider,
  AuiConfig,
  Tools,
  useAui,
  useAuiState,
  useLocalRuntime,
} from "@assistant-ui/react";
import {
  CirclePlus,
  Folder,
  FolderOpen,
  FolderPlus,
  LoaderCircle,
  PanelRight,
  Plus,
  Settings,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AskPanel } from "@/chat/AskPanel";
import { useCompactionStore } from "@/chat/compaction-store";
import { useBackendEvents } from "@/chat/event-bridge";
import { useFollowUpStore } from "@/chat/followup-store";
import { Inspector } from "@/chat/Inspector";
import { ReconnectOverlay } from "@/chat/ReconnectOverlay";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { reconnectIfNeeded, useRunReconnectStore } from "@/chat/run-reconnect";
import { sessionAdapter } from "@/chat/runtime";
import { useSessionModelStore } from "@/chat/session-model";
import { restoreSessionPrefs } from "@/chat/session-prefs";
import { RunSummariesContext, SessionScopeContext } from "@/chat/session-scope";
import { ChatThread } from "@/chat/Thread";
import { cwToolkit } from "@/chat/toolkit";
import type { RunSummary } from "@/chat/tree-persistence";
import { deriveRunSummaries, loadedTreeToExported } from "@/chat/tree-persistence";
import { useUsageStatsStore } from "@/chat/usage-stats";
import { ConfirmDialog } from "@/components/ui/confirm";
import { ContextMenu, type ContextMenuEntry } from "@/components/ui/context-menu";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  createSession,
  deleteProject,
  deleteSession,
  listenEvent,
  renameSession,
  resolveExitRequest,
  saveProject,
  selectDir,
} from "@/ipc/client";
import type { ProjectMeta, SessionMeta } from "@/ipc/types";
import type { PageKey } from "@/settings/registry";
import { SettingsPage } from "@/settings/SettingsPage";
import { useAskStore } from "@/stores/ask";
import { loadHistory, useSessionPersistStore } from "@/stores/session";
import { findReusableEmptyTab, useSessionsStore } from "@/stores/sessions";

// 正式工具卡装配（S1 实测钉死：Tools 是 resource 工厂，经 AuiConfig 注入；
// config 为纯数据可提模块级，各 SessionTab 共用同一份）。
const cwAuiConfig = AuiConfig({ tools: Tools({ toolkit: cwToolkit }) });

/** 侧栏初始宽（= 最小宽）：原 w-60 / w-72 对应像素；可拖宽，上限半屏。 */
export const SIDEBAR_MIN_WIDTH = 240;
export const INSPECTOR_MIN_WIDTH = 288;

/** 宽度夹取：min=初始宽，max=半屏（拖动与键盘方向键共用同一条规则）。 */
export function clampWidth(w: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, w));
}

/** 会话工作区根组件：左栏（含交通灯让位顶条，常开）| 会话大圆角卡 | 右栏。
 *  背景一体化：左/右栏与窗口同底色，会话区承载在四角圆角卡上（四周留缝），
 *  卡头显示项目/标题。挂载时恢复会话列表，未就绪前显示加载占位。 */
export function SessionWorkspace() {
  useBackendEvents();
  const { exitDialog } = useHostEvents();
  const ready = useSessionsStore((s) => s.ready);
  const tabs = useSessionsStore((s) => s.tabs);
  const projects = useSessionsStore((s) => s.projects);
  const activeId = useSessionsStore((s) => s.activeId);
  const inspectorOpen = useSessionsStore((s) => s.inspectorOpen);
  const setInspectorOpen = useSessionsStore((s) => s.setInspectorOpen);
  // 设置页浮层（17 号后从 App 顶层挪入工作区；入口在左栏底部）
  const [settingsPage, setSettingsPage] = useState<PageKey | null>(null);
  // 侧栏宽度为局部态不落盘（重启回初始宽；持久化的是开合状态）
  const [leftWidth, setLeftWidth] = useState(SIDEBAR_MIN_WIDTH);
  const [rightWidth, setRightWidth] = useState(INSPECTOR_MIN_WIDTH);
  // 拖宽进行中：关掉宽度过渡，保证手柄 1:1 跟手（松手后恢复动画）
  const [resizing, setResizing] = useState(false);

  useEffect(() => {
    void useSessionsStore
      .getState()
      .bootstrap()
      .catch((err) => console.error("[sessions] 启动恢复失败", err));
  }, []);

  if (!ready) {
    return (
      <div className="flex h-dvh items-center justify-center bg-[var(--background)] text-ui-base text-[var(--muted-foreground)]">
        正在恢复会话…
      </div>
    );
  }
  const activeTab = tabs.find((t) => t.id === activeId);
  const project =
    activeTab?.projectId !== undefined
      ? projects.find((p) => p.id === activeTab.projectId)
      : undefined;
  return (
    <div className="flex h-dvh bg-[var(--background)] pr-2 text-[var(--foreground)]">
      {/* pr-2：右栏收起（右手柄不渲染）时卡片右侧仍留与底部一致的缝 */}
      <Sidebar
        width={leftWidth}
        animate={!resizing}
        onOpenSettings={() => setSettingsPage("appearance")}
      />
      {/* 手柄贴卡片边框，留缝留在手柄外侧（ml-2/mr-2） */}
      <ResizeHandle
        label="左侧栏宽度"
        value={leftWidth}
        min={SIDEBAR_MIN_WIDTH}
        className="ml-2"
        onResizing={setResizing}
        onChange={setLeftWidth}
      />
      {/* 会话区大圆角卡：上下与窗口边缘留缝（py-2），左右紧贴拖宽手柄 */}
      <div className="flex min-w-0 flex-1 flex-col py-2">
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-[var(--border)] bg-[var(--cw-bg-main)]">
          {/* 卡头：项目/标题（FR 拖拽区），右侧为右栏折叠钮 */}
          <div
            data-tauri-drag-region
            className="flex h-9 shrink-0 cursor-default select-none items-center gap-2 border-b border-[var(--border)] px-4 text-ui-sm text-[var(--muted-foreground)]"
          >
            {activeTab && (
              <div className="flex min-w-0 items-center gap-1.5">
                <Folder size={16} className="shrink-0" />
                {project && (
                  <>
                    <span className="shrink-0">{project.name}</span>
                    <span className="text-[var(--cw-dim)]">/</span>
                  </>
                )}
                <span className="truncate font-medium text-[var(--foreground)]">
                  {activeTab.title}
                </span>
              </div>
            )}
            <SidebarToggle open={inspectorOpen} onToggle={() => setInspectorOpen(!inspectorOpen)} />
          </div>
          <div className="relative min-h-0 flex-1">
            {tabs.map((tab) => (
              <SessionTab
                key={tab.id}
                sessionId={tab.id}
                active={tab.id === activeId}
                onOpenSettings={() => setSettingsPage("providers")}
              />
            ))}
          </div>
        </div>
      </div>
      {activeId !== null && (
        <>
          {inspectorOpen && (
            <ResizeHandle
              label="右侧栏宽度"
              value={rightWidth}
              min={INSPECTOR_MIN_WIDTH}
              inverse
              className="mr-2"
              onResizing={setResizing}
              onChange={setRightWidth}
            />
          )}
          {/* Inspector 只依赖会话域（scope/metrics/plan store），提升到
              工作区挂载以便拖宽手柄同层排布；随 activeId 取数即随 Tab 切换 */}
          <SessionScopeContext.Provider value={activeId}>
            <Inspector width={rightWidth} open={inspectorOpen} animate={!resizing} />
          </SessionScopeContext.Provider>
        </>
      )}
      {exitDialog}
      {settingsPage && (
        <SettingsPage initialPage={settingsPage} onClose={() => setSettingsPage(null)} />
      )}
    </div>
  );
}

/** 宿主事件接线（17 号票）：退出确认 + 通知点击路由（§9.6）。 */
function useHostEvents(): { exitDialog: React.ReactNode } {
  const [exitConfirm, setExitConfirm] = useState(false);
  const openTab = useSessionsStore((s) => s.openTab);
  useEffect(() => {
    // 退出拦截：托盘退出遇活跃 run → 前端确认 → resolve_exit_request 回执
    const unlistenExit = listenEvent<{ hasActiveRuns: boolean }>(
      "app:exit_requested",
      (payload) => {
        if (payload.hasActiveRuns) setExitConfirm(true);
      },
    );
    // 通知点击：路由到对应会话（列表可能未含 → 刷新后重试一次）
    const unlistenNotify = listenEvent<{ sessionId?: string }>("notify:activate", (payload) => {
      const sessionId = payload.sessionId;
      if (sessionId === undefined) return;
      void (async () => {
        const found = useSessionsStore.getState().sessions.find((m) => m.id === sessionId);
        if (found) {
          openTab(found);
          return;
        }
        await useSessionsStore.getState().refresh();
        const retry = useSessionsStore.getState().sessions.find((m) => m.id === sessionId);
        if (retry) openTab(retry);
      })();
    });
    return () => {
      void unlistenExit.then((un) => un());
      void unlistenNotify.then((un) => un());
    };
  }, [openTab]);
  return {
    exitDialog: <ExitConfirmDialog open={exitConfirm} onClose={() => setExitConfirm(false)} />,
  };
}

/** 退出确认框（§9.6：存在活跃 run 的退出请求）。 */
function ExitConfirmDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    <ConfirmDialog
      open={open}
      title="仍有运行中的任务"
      description="现在退出将终止所有运行中的任务。确定退出吗？"
      confirmLabel="退出"
      onCancel={onClose}
      onConfirm={() => {
        onClose();
        void resolveExitRequest(true).catch((err) => console.error("[shell] 退出确认失败", err));
      }}
    />
  );
}

/** 单会话 Tab 面（keep-mounted：非活跃 display:none，运行态与帧流不断）。 */
function SessionTab({
  sessionId,
  active,
  onOpenSettings,
}: {
  sessionId: string;
  active: boolean;
  onOpenSettings: () => void;
}) {
  const adapter = useMemo(() => sessionAdapter(sessionId), [sessionId]);
  // 消息队列已迁移到后端（06 号票，§5.5）：运行中 Enter=steer 插话、
  // Alt+Enter=followUp 排队（后端 queue.json 持久化 + run 正常完成后自动
  // 投递）——M1 的 unstable_enableMessageQueue 前端队列撤除，避免与后端
  // 队列双轨重复投递。
  const runtime = useLocalRuntime(adapter);
  // run 收口摘要（17 号票折叠分组数据面）：历史收养后按 runMarker 推导
  const [summaries, setSummaries] = useState<Map<string, RunSummary>>(() => new Map());
  // 树收养（历史恢复 + 观察态收口共用）：load → import → 摘要/压缩投影刷新
  const adoptTree = useCallback(
    async (tree: NonNullable<Awaited<ReturnType<typeof loadHistory>>>) => {
      runtime.thread.import(loadedTreeToExported(tree));
      setSummaries(deriveRunSummaries(tree.messages));
      // 压缩边界投影（04 号票，§6.3：compaction 条目不进树，store 承载）
      useCompactionStore.getState().setCompactions(sessionId, tree.compactions ?? []);
      // steer 插话节点收养剔除（06 号票，§5.5）：树内已有同 id 的普通 user
      // 消息，本地「插话」节点任务完成
      useFollowUpStore
        .getState()
        .dropSteerNodes(sessionId, new Set(tree.messages.map((m) => m.id)));
    },
    [runtime, sessionId],
  );
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const tree = await loadHistory(sessionId);
        if (cancelled || tree === null) return;
        await adoptTree(tree);
      } catch (err) {
        console.warn("[session] 历史收养失败（以空会话继续）", err);
      }
    })();
    // 重连观察态（05 号票，§5.8）：WebView 重载/崩溃后 Rust run 仍存活时
    // 恢复实时流——快照 running → attach 重放续流；runEnded（或无帧兜底）
    // 收口后经本回调重载 canonical 树（含观察期间落盘的全部产物）
    void reconnectIfNeeded(sessionId, {
      onEnded: async () => {
        try {
          const tree = await loadHistory(sessionId);
          if (!cancelled && tree !== null) await adoptTree(tree);
        } catch (err) {
          console.warn("[session] 观察态收口树重载失败", err);
        }
      },
    });
    return () => {
      cancelled = true;
    };
  }, [sessionId, adoptTree]);
  // 会话偏好恢复（18 号票，FR-14-05）：进入会话 get_session_prefs → 回填
  // 模型覆盖与 thinking 档两个内存态 store（选模型 + 切档跨重启还原）。
  // 恢复走 store 纯方法（session-prefs 内部已处理），读失败静默按空偏好。
  useEffect(() => {
    void restoreSessionPrefs(sessionId);
  }, [sessionId]);
  // Ctrl+L 模型快速切换（FR-13-05，18 号票）：仅活跃 Tab 响应（多 Tab
  // keep-mounted，非活跃 handler 直接放行不拦截）——经 openRequest store
  // 唤起本会话 Composer 的 ModelPicker 下拉。
  useEffect(() => {
    if (!active) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.ctrlKey && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "l") {
        event.preventDefault();
        useSessionModelStore.getState().requestOpen(sessionId);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, sessionId]);
  return (
    <div className={active ? "absolute inset-0 flex" : "hidden"}>
      <AssistantRuntimeProvider runtime={runtime} config={cwAuiConfig}>
        <SessionScopeContext.Provider value={sessionId}>
          <RunSummariesContext.Provider value={summaries}>
            <div className="relative flex min-h-0 w-full">
              <ChatThread onOpenSettings={onOpenSettings} />
              <ReconnectOverlay sessionId={sessionId} />
            </div>
            <AskPanel />
            <RunningReporter sessionId={sessionId} />
            <DraftRestorer sessionId={sessionId} />
          </RunSummariesContext.Provider>
        </SessionScopeContext.Provider>
      </AssistantRuntimeProvider>
    </div>
  );
}

/** 运行态桥：assistant-ui thread 状态 → sessions store 的 Tab 指示。 */
function RunningReporter({ sessionId }: { sessionId: string }) {
  const running = useAuiState((s) => s.thread.isRunning);
  useEffect(() => {
    useSessionsStore.getState().setRunning(sessionId, running);
  }, [sessionId, running]);
  return null;
}

/** 草稿恢复（§10.6，17 号票）：挂载时把 ui-state 里的本会话草稿写回输入框
 *  一次（此后 composer-keys 的记账接管——恢复写入同样幂等记账）。 */
function DraftRestorer({ sessionId }: { sessionId: string }) {
  const aui = useAui();
  useEffect(() => {
    const draft = useSessionsStore.getState().drafts[sessionId];
    if (draft !== undefined && draft.length > 0) {
      aui.composer.setText(draft, { origin: "user" });
    }
  }, [aui, sessionId]);
  return null;
}

/** 左栏（§10.7）：顶部条（交通灯让位，常驻）+ 搜索 + 临时会话区 +
 *  项目分组会话树；底部「设置/用量」双入口各占一半宽度。与窗口背景
 *  一体化（无底色无边框）。宽度受控（拖宽手柄，min=初始宽 max=半屏），
 *  拖动时过渡关闭保证跟手。 */
function Sidebar({
  width,
  animate,
  onOpenSettings,
}: {
  width: number;
  animate: boolean;
  onOpenSettings: () => void;
}) {
  const [confirm, setConfirm] = useState<
    { kind: "session"; session: SessionMeta } | { kind: "project"; project: ProjectMeta } | null
  >(null);
  // 新建项目弹窗（参考 ZCode 创建项目弹窗）：+ 钮改为开弹窗选名称与目录
  const [createOpen, setCreateOpen] = useState(false);
  // 右键菜单（ZCode 列表级单例）：坐标 + 目标；重命名受控 id（菜单触发
  // 行内编辑态，替代已移除的 hover 内联钮——时间戳不再被挤）
  const [renamingId, setRenamingId] = useState<string | null>(null);
  // 项目组折叠态（内存态，重启回全展开）：点击项目行切换名下会话列表显隐
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(() => new Set());
  const toggleProject = (id: string) => {
    setCollapsedProjects((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    kind: "session" | "project";
    id: string;
  } | null>(null);
  const list = useScrollFade();
  const projects = useSessionsStore((s) => s.projects);
  const sessions = useSessionsStore((s) => s.sessions);
  const activeId = useSessionsStore((s) => s.activeId);
  const openTab = useSessionsStore((s) => s.openTab);

  const temporaries = sessions.filter((m) => m.projectId === undefined);
  const byProject = new Map<string, SessionMeta[]>();
  for (const meta of sessions) {
    if (meta.projectId === undefined) continue;
    byProject.set(meta.projectId, [...(byProject.get(meta.projectId) ?? []), meta]);
  }
  const contextEntries: ContextMenuEntry[] =
    menu === null
      ? []
      : menu.kind === "session"
        ? [
            { label: "重命名", onSelect: () => setRenamingId(menu.id) },
            { separator: true },
            {
              label: "删除会话",
              danger: true,
              onSelect: () => {
                const meta = sessions.find((m) => m.id === menu.id);
                if (meta) setConfirm({ kind: "session", session: meta });
              },
            },
          ]
        : [
            { label: "新建会话", onSelect: () => void createAndOpen(menu.id) },
            { label: "重命名", onSelect: () => setRenamingId(menu.id) },
            { separator: true },
            {
              label: "删除项目",
              danger: true,
              onSelect: () => {
                const project = projects.find((pr) => pr.id === menu.id);
                if (project) setConfirm({ kind: "project", project });
              },
            },
          ];
  return (
    <aside
      data-testid="session-sidebar"
      className={`shrink-0 overflow-hidden ${
        animate ? "transition-[width] duration-200 ease-out" : ""
      }`}
      style={{ width }}
    >
      <div className="flex h-full min-h-0 flex-col" style={{ width }}>
        {/* 顶部条：交通灯让位；h-12（48px）与 Rust 侧交通灯容器
            （12px 钮 + inset 36）同心 */}
        <div
          data-tauri-drag-region
          className="h-12 shrink-0 cursor-default select-none"
          style={{ paddingLeft: "max(var(--tauri-plugin-decoration-left-clearance, 76px), 8px)" }}
        />
        <div className="flex min-h-0 flex-1 flex-col">
          {/* 主操作区：整宽大行（ZCode NewTaskButtonGroup 同款：h-8 圆角 lg，
              顶部 pt-3 对齐其导航块 py-3 的呼吸感） */}
          <div className="px-2 py-3">
            <button
              type="button"
              data-testid="new-temp-session"
              onClick={() => void createAndOpen(undefined)}
              className="flex h-8 w-full items-center gap-2 rounded-lg px-2.5 text-ui-base text-[var(--foreground)] hover:bg-[var(--selected)]"
            >
              <CirclePlus size={16} className="shrink-0" />
              新建会话
            </button>
          </div>
          <div className="mx-3 shrink-0 border-t border-[var(--border)]/50" />
          <div
            ref={list.ref}
            className={`min-h-0 flex-1 overflow-y-auto px-2 pt-2 pb-2 ${FADE_MASK[list.fade]}`}
          >
            {temporaries.length === 0 && projects.length === 0 && (
              <div className="px-8.5 py-2 text-ui-base text-[var(--cw-dim)]">暂无会话</div>
            )}
            <SectionHeader title="临时会话" />
            <SectionBody>
              {temporaries.map((meta) => (
                <SessionRow
                  key={meta.id}
                  meta={meta}
                  active={meta.id === activeId}
                  editing={renamingId === meta.id}
                  onRenameEnd={() => setRenamingId(null)}
                  onOpen={() => openTab(meta)}
                  onContextMenu={(x, y) => setMenu({ x, y, kind: "session", id: meta.id })}
                />
              ))}
            </SectionBody>
            {/* 项目组标题：ZCode purpose section 同款——同字号仅 medium + subtlest
                最淡档（styles.css --foreground-subtlest）；悬停露出「新建项目」 */}
            <div className="group mt-3 flex h-7 items-center px-2 text-ui-base font-medium text-[var(--foreground-subtlest)]">
              <span>项目</span>
              <button
                type="button"
                data-testid="new-project"
                aria-label="新建项目"
                onClick={() => setCreateOpen(true)}
                className="ml-auto flex h-6 w-6 items-center justify-center rounded opacity-0 transition-opacity hover:bg-[var(--accent)] group-hover:opacity-100"
              >
                +
              </button>
            </div>
            {projects.map((project) => {
              const list = byProject.get(project.id) ?? [];
              const collapsed = collapsedProjects.has(project.id);
              return (
                <div key={project.id} className="mb-1">
                  <ProjectRow
                    project={project}
                    editing={renamingId === project.id}
                    collapsed={collapsed}
                    onToggle={() => toggleProject(project.id)}
                    onRenameEnd={() => setRenamingId(null)}
                    onAddSession={() => void createAndOpen(project.id)}
                    onContextMenu={(x, y) => setMenu({ x, y, kind: "project", id: project.id })}
                  />
                  {/* 名下会话缩进一级（ZCode 为两层 pl-2（16px)）；收起时不渲染 */}
                  {!collapsed && (
                    <div className="ml-4 pt-1">
                      <SectionBody>
                        {list.map((meta) => (
                          <SessionRow
                            key={meta.id}
                            meta={meta}
                            active={meta.id === activeId}
                            editing={renamingId === meta.id}
                            onRenameEnd={() => setRenamingId(null)}
                            onOpen={() => openTab(meta)}
                            onContextMenu={(x, y) =>
                              setMenu({ x, y, kind: "session", id: meta.id })
                            }
                          />
                        ))}
                      </SectionBody>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
          {/* 底部：设置入口（右栏用量改由卡头右上钮开合） */}
          <div className="pr-3 pl-4 pt-2 pb-4">
            <button
              type="button"
              data-testid="open-settings"
              onClick={onOpenSettings}
              className="flex h-8 w-full items-center gap-2 rounded-lg px-2.5 text-ui-base text-[var(--foreground)] hover:bg-[var(--selected)]"
            >
              <Settings size={16} className="shrink-0" />
              设置
            </button>
          </div>
        </div>
      </div>
      <NewProjectDialog open={createOpen} onClose={() => setCreateOpen(false)} />
      <ContextMenu
        x={menu?.x ?? 0}
        y={menu?.y ?? 0}
        entries={contextEntries}
        onClose={() => setMenu(null)}
      />
      <ConfirmDialog
        open={confirm !== null}
        title={
          confirm?.kind === "session"
            ? `删除会话「${confirm.session.title}」？`
            : `删除项目「${confirm?.kind === "project" ? confirm.project.name : ""}」？`
        }
        description={
          confirm?.kind === "session"
            ? "会话历史与工具记录将一并删除，不可恢复。"
            : "仅解除注册（项目目录不动），名下会话转为临时会话保留。"
        }
        confirmLabel="删除"
        onCancel={() => setConfirm(null)}
        onConfirm={() => {
          if (confirm === null) return;
          if (confirm.kind === "session") {
            void deleteSessionAndClose(confirm.session.id);
          } else {
            void client_deleteProject(confirm.project.id);
          }
          setConfirm(null);
        }}
      />
    </aside>
  );
}

// ---- 左栏数据操作（ IPC + store 同步；异常只 console——列表仍在） ----

/** 新建会话并打开 Tab（可选挂在某项目下），随后刷新左栏列表；同项目域已有
 *  「新建但未发送」的 Tab 时复用激活（空会话不进左栏列表，见 findReusableEmptyTab）。 */
async function createAndOpen(projectId: string | undefined): Promise<void> {
  const empty = findReusableEmptyTab(
    useSessionsStore.getState().tabs,
    useSessionPersistStore.getState().scopes,
    projectId,
  );
  if (empty) {
    useSessionsStore.getState().openTab(empty);
    return;
  }
  try {
    const meta = await createSession(projectId);
    useSessionsStore.getState().openTab(meta);
    await useSessionsStore.getState().refresh();
  } catch (err) {
    console.error("[sessions] 新建会话失败", err);
  }
}

/** 新建项目：按名称+目录注册，随后刷新左栏列表（名称留空时调用方兜底；
 *  失败上抛让弹窗保持打开）。 */
async function createProject(name: string, homeDir: string): Promise<void> {
  try {
    await saveProject({ project: { name, homeDir } });
    await useSessionsStore.getState().refresh();
  } catch (err) {
    console.error("[sessions] 新建项目失败", err);
    throw err;
  }
}

/** 新建项目弹窗（参考 ZCode 创建项目弹窗）：项目名称 + 源文件夹虚线
 *  选择区；名称留空取文件夹名兜底，未选文件夹时创建禁用。 */
function NewProjectDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [name, setName] = useState("");
  const [homeDir, setHomeDir] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const pick = async () => {
    const dir = await selectDir();
    if (dir) setHomeDir(dir);
  };
  const submit = () => {
    if (homeDir === null || busy) return;
    setBusy(true);
    void (async () => {
      const fallback = homeDir.split("/").filter(Boolean).at(-1) ?? homeDir;
      try {
        await createProject(name.trim() || fallback, homeDir);
        setName("");
        setHomeDir(null);
        onClose();
      } catch {
        // 保留弹窗与已填内容，用户可直接重试
      } finally {
        setBusy(false);
      }
    })();
  };
  return (
    <Dialog
      open={open}
      title="新建项目"
      onClose={onClose}
      width={480}
      actions={[
        { label: "取消", variant: "ghost", onClick: onClose },
        {
          label: "创建项目",
          variant: "default",
          disabled: homeDir === null || busy,
          onClick: submit,
        },
      ]}
    >
      <div className="mb-5">
        <p className="mb-1.5 text-ui-base font-medium text-[var(--foreground)]">项目名称</p>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="新项目"
          autoFocus
        />
      </div>
      <div>
        <p className="mb-1.5 text-ui-base font-medium text-[var(--foreground)]">源文件夹</p>
        <button
          type="button"
          data-testid="pick-project-dir"
          onClick={() => void pick()}
          className="flex w-full flex-col items-center gap-2 rounded-lg border border-dashed border-[var(--input)] px-4 py-6 transition-colors hover:border-[var(--cw-text-2)]"
        >
          <span className="flex size-12 items-center justify-center rounded-lg bg-[var(--muted)] text-[var(--muted-foreground)]">
            <FolderPlus size={20} />
          </span>
          {homeDir === null ? (
            <span className="text-ui-base font-medium text-[var(--foreground)]">选择文件夹</span>
          ) : (
            <span className="max-w-full truncate font-mono text-ui-sm text-[var(--foreground)]">
              {homeDir}
            </span>
          )}
          <span className="text-ui-sm text-[var(--muted-foreground)]">
            {homeDir === null ? "CraftWave 可读取和编辑此文件夹" : "点击重新选择"}
          </span>
        </button>
      </div>
    </Dialog>
  );
}

/** 删除会话并收尾：关 Tab + 清各 store 里该会话的分桶（持久化/指标/ask/重连观察态）。 */
async function deleteSessionAndClose(sessionId: string): Promise<void> {
  try {
    await deleteSession(sessionId);
  } catch (err) {
    console.error("[sessions] 删除会话失败", err);
    return;
  }
  useSessionsStore.getState().closeTab(sessionId);
  useSessionPersistStore.getState().forgetScope(sessionId);
  useRunMetricsStore.getState().forgetSession(sessionId);
  useAskStore.getState().forgetSession(sessionId);
  useCompactionStore.getState().forgetSession(sessionId);
  useRunReconnectStore.getState().forgetSession(sessionId);
  useFollowUpStore.getState().forgetSession(sessionId);
  // 用量面收尾（20 号票）：旧数据标量回落桶摘除（明细以后端删除为准）
  useUsageStatsStore.getState().forgetSession(sessionId);
  await useSessionsStore.getState().refresh();
}

/** 删除项目（仅解除注册，目录不动），随后刷新列表。 */
async function client_deleteProject(projectId: string): Promise<void> {
  try {
    await deleteProject(projectId);
    await useSessionsStore.getState().refresh();
  } catch (err) {
    console.error("[sessions] 删除项目失败", err);
  }
}

// ---- 小件 ----

/** 左栏分组标题：与行文字同字号，仅灰+medium（截图与 ZCode 组头口径一致）。 */
function SectionHeader({ title }: { title: string }) {
  return <div className="px-2.5 py-1 text-ui-base font-medium text-[var(--cw-dim)]">{title}</div>;
}

/** 左栏项目行：文件夹图标 + 项目名（前景色大字）。整行可点击展开/收起
 *  名下会话（行尾方向指示，ZCode 项目树同款）；悬停露出「新建会话」钮
 *  （高频入口的恢复——0d75ca0 曾把行内钮全收进右键菜单，但项目行没有
 *  时间戳可挤），改名/删项目仍走右键菜单，改名走内联输入框（Enter 提交，
 *  Esc 取消）。导出供测试（sidebar-overflow.test.tsx 同 MarqueeTitle 先例）。 */
export function ProjectRow({
  project,
  editing,
  collapsed,
  onToggle,
  onRenameEnd,
  onAddSession,
  onContextMenu,
}: {
  project: ProjectMeta;
  editing: boolean;
  /** 名下会话列表是否已收起（行尾方向指示与点击切换的数据面）。 */
  collapsed: boolean;
  onToggle: () => void;
  onRenameEnd: () => void;
  onAddSession: () => void;
  onContextMenu: (x: number, y: number) => void;
}) {
  const [name, setName] = useState(project.name);
  const commit = () => {
    onRenameEnd();
    const trimmed = name.trim();
    if (!trimmed || trimmed === project.name) {
      setName(project.name);
      return;
    }
    void (async () => {
      try {
        await saveProject({
          project: { id: project.id, name: trimmed, homeDir: project.homeDir },
        });
        await useSessionsStore.getState().refresh();
      } catch (err) {
        console.error("[sessions] 项目改名失败", err);
      }
    })();
  };
  if (editing) {
    return (
      <div className="px-1 py-0.5">
        <Input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") {
              setName(project.name);
              onRenameEnd();
            }
          }}
        />
      </div>
    );
  }
  return (
    // biome-ignore lint/a11y/useSemanticElements: 行内嵌「新建会话」钮，button 不可嵌套 button
    <div
      role="button"
      tabIndex={0}
      aria-expanded={!collapsed}
      onClick={onToggle}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onToggle();
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onContextMenu(e.clientX, e.clientY);
      }}
      className="group flex h-8 cursor-default select-none items-center gap-2 rounded-lg pr-1 pl-2.5 text-ui-base text-[var(--foreground)] hover:bg-[var(--selected)]"
    >
      {/* 折叠状态指示（行尾不放箭头）：闭合=收起、打开=展开；与项目名同色
          （ZCode ProjectFolderIcon 也是 text-foreground-subtle，行内单一灰阶） */}
      {collapsed ? (
        <Folder
          size={16}
          aria-hidden
          data-testid="project-folder-icon"
          className="shrink-0 text-[var(--foreground-subtle)]"
        />
      ) : (
        <FolderOpen
          size={16}
          aria-hidden
          data-testid="project-folder-icon"
          className="shrink-0 text-[var(--foreground-subtle)]"
        />
      )}
      {/* 项目名比会话标题淡一档（ZCode text-foreground-subtle 同款：60% 透明混合） */}
      <span className="min-w-0 flex-1 text-[var(--foreground-subtle)]">
        <MarqueeTitle text={project.name} />
      </span>
      {/* 悬停露出「新建会话」（ZCode 项目行同款）：样式对齐组头「新建项目」+ 钮 */}
      <button
        type="button"
        data-testid={`project-add-session-${project.id}`}
        aria-label={`在项目 ${project.name} 下新建会话`}
        title="新建会话"
        onClick={(e) => {
          // 不冒泡给行：+ 钮只新建，不切换折叠
          e.stopPropagation();
          onAddSession();
        }}
        className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-[var(--muted-foreground)] opacity-0 transition-opacity hover:bg-[var(--accent)] hover:text-[var(--foreground)] group-hover:opacity-100"
      >
        <Plus size={14} aria-hidden />
      </button>
    </div>
  );
}

/** 分组体（A7 显示更多）：超过 5 条默认只显示前 5 条，点「显示更多 (N)」展开。 */
const SECTION_PREVIEW_COUNT = 5;
function SectionBody({ children }: { children: React.ReactNode }) {
  const [expanded, setExpanded] = useState(false);
  const items = Array.isArray(children) ? children : [children];
  const hidden = items.length - SECTION_PREVIEW_COUNT;
  const visible = expanded || hidden <= 0 ? items : items.slice(0, SECTION_PREVIEW_COUNT);
  return (
    <div className="space-y-0.5">
      {visible}
      {hidden > 0 && !expanded && (
        <button
          type="button"
          className="mt-0.5 w-full rounded px-2 py-0.5 text-left text-ui-base text-[var(--cw-dim)] hover:bg-[var(--accent)]/60"
          onClick={() => setExpanded(true)}
        >
          显示更多（{hidden}）
        </button>
      )}
    </div>
  );
}

/** 滚动渐隐（ZCode ScrollFadeViewport 同款）：按滚动位置给容器切上下
 *  24px 渐隐 mask——none/top/bottom/both 四态。 */
type ScrollFade = "none" | "top" | "bottom" | "both";
const FADE_MASK: Record<ScrollFade, string> = {
  none: "",
  top: "[mask-image:linear-gradient(to_bottom,transparent_0,black_24px,black_100%)]",
  bottom: "[mask-image:linear-gradient(to_bottom,black_0,black_calc(100%-24px),transparent_100%)]",
  both: "[mask-image:linear-gradient(to_bottom,transparent_0,black_24px,black_calc(100%-24px),transparent_100%)]",
};

export function useScrollFade() {
  const ref = useRef<HTMLDivElement>(null);
  const [fade, setFade] = useState<ScrollFade>("none");
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const top = el.scrollTop > 0;
      const bottom = el.scrollTop + el.clientHeight < el.scrollHeight - 1;
      setFade(top && bottom ? "both" : top ? "top" : bottom ? "bottom" : "none");
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(el);
    const observeChildren = () => {
      ro.disconnect();
      ro.observe(el);
      for (const child of el.children) ro.observe(child);
      update();
    };
    const mutations = new MutationObserver(observeChildren);
    mutations.observe(el, { childList: true, subtree: true, characterData: true });
    observeChildren();
    return () => {
      mutations.disconnect();
      el.removeEventListener("scroll", update);
      ro.disconnect();
    };
  }, []);
  return { ref, fade };
}

/** 走马灯标题（ZCode TaskTitleOverflowText 同款）：溢出才滚动——
 *  hover 1s 后以 40px/s 平移，副本间隔 24px；未溢出为静态文本。 */
export function MarqueeTitle({ text }: { text: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const [overflow, setOverflow] = useState(false);
  const [shift, setShift] = useState(0);
  // text 变化需重测溢出（ResizeObserver 只感知容器尺寸，不感知 scrollWidth）
  // biome-ignore lint/correctness/useExhaustiveDependencies: 见上
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const check = () => {
      const width = textRef.current?.getBoundingClientRect().width ?? 0;
      setOverflow(width > el.clientWidth + 1);
      setShift(width + 24);
    };
    check();
    const ro = new ResizeObserver(check);
    ro.observe(el);
    if (textRef.current) ro.observe(textRef.current);
    return () => ro.disconnect();
  }, [text]);
  return (
    <div ref={ref} className="cw-marquee" data-overflow={overflow}>
      <span
        className="cw-marquee-track"
        style={
          overflow
            ? ({
                "--marquee-shift": `${-shift}px`,
                "--marquee-duration": `${shift / 40}s`,
              } as React.CSSProperties)
            : undefined
        }
      >
        <span ref={textRef} className="shrink-0">
          {text}
        </span>
        {overflow && (
          <span aria-hidden className="shrink-0">
            {text}
          </span>
        )}
      </span>
    </div>
  );
}

/** 相对时间（A6）：<1 分钟「刚刚」；当天「N 小时」；7 天内「N 天」；更早 MM-DD。 */
function relativeTime(updatedAt: number): string {
  const diffMs = Date.now() - updatedAt;
  const minutes = Math.floor(diffMs / 60000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days} 天`;
  const d = new Date(updatedAt);
  return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** 左栏单行会话：点击打开 Tab；右键菜单承载重命名/删除（内联输入改名，
 *  editing 由侧栏受控）；运行中显示转圈。 */
function SessionRow({
  meta,
  active,
  editing,
  onRenameEnd,
  onOpen,
  onContextMenu,
}: {
  meta: SessionMeta;
  active: boolean;
  editing: boolean;
  onRenameEnd: () => void;
  onOpen: () => void;
  onContextMenu: (x: number, y: number) => void;
}) {
  const running = useSessionsStore((s) => s.running[meta.id]);
  const [title, setTitle] = useState(meta.title);
  const commit = () => {
    onRenameEnd();
    const trimmed = title.trim();
    if (!trimmed || trimmed === meta.title) {
      setTitle(meta.title);
      return;
    }
    void (async () => {
      try {
        await renameSession(meta.id, trimmed);
        await useSessionsStore.getState().refresh();
      } catch (err) {
        console.error("[sessions] 重命名失败", err);
      }
    })();
  };
  if (editing) {
    return (
      <div className="px-1 py-0.5">
        <Input
          autoFocus
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") {
              setTitle(meta.title);
              onRenameEnd();
            }
          }}
        />
      </div>
    );
  }
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: 右键菜单宿主行，交互语义由内部 button 承担
    <div
      data-testid={`session-row-${meta.id}`}
      onContextMenu={(e) => {
        e.preventDefault();
        onContextMenu(e.clientX, e.clientY);
      }}
      className={`group flex h-8 cursor-default select-none items-center gap-2 rounded-lg pr-1 pl-2.5 text-ui-base ${
        active
          ? "bg-[var(--selected)] text-[var(--foreground)]"
          : "text-[var(--foreground)] hover:bg-[var(--selected)]"
      }`}
    >
      {/* 运行中：转圈指示（ZCode 状态槽同款，size-4） */}
      {running && (
        <LoaderCircle
          size={16}
          aria-hidden
          className="shrink-0 animate-spin text-[var(--muted-foreground)]"
        />
      )}
      {/* 标题溢出走马灯（hover 1s 后滚动）+ 右缘渐隐，比硬截断柔和 */}
      <button type="button" className="min-w-0 flex-1 text-left" onClick={onOpen}>
        <MarqueeTitle text={meta.title} />
      </button>
      {/* 相对时间（A6）：悬停让位于操作钮（ZCode 元信息 hover 隐藏同款） */}
      <span className="mr-0.5 shrink-0 text-ui-sm tabular-nums text-[var(--cw-dim)]">
        {relativeTime(meta.updatedAt)}
      </span>
    </div>
  );
}

/** 右栏折叠钮（卡头右端）：展开态高亮、收起态灰调，与右栏开合同步。 */
function SidebarToggle({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  const label = open ? "折叠右侧栏" : "展开右侧栏";
  return (
    <button
      type="button"
      data-testid="inspector-toggle-top"
      aria-label={label}
      aria-pressed={open}
      title={label}
      onClick={onToggle}
      className={`ml-auto rounded p-1.5 ${
        open
          ? "bg-[var(--accent)] text-[var(--foreground)]"
          : "text-[var(--muted-foreground)] hover:bg-[var(--accent)]"
      }`}
    >
      <PanelRight size={14} />
    </button>
  );
}

/** 侧栏拖宽手柄：pointer capture 横向拖动（右栏方向相反），双击恢复初始宽
 *  （min=初始宽），方向键 ±16px（separator 键盘约定），上限半屏；拖动起止
 *  经 onResizing 通知宿主关/开侧栏宽度过渡。 */
function ResizeHandle({
  label,
  value,
  min,
  inverse,
  className,
  onChange,
  onResizing,
}: {
  label: string;
  value: number;
  min: number;
  /** 右栏手柄：向左拖 = 加宽。 */
  inverse?: boolean;
  /** 手柄外侧留缝等布局微调（如 ml-2/mr-2）。 */
  className?: string;
  onChange: (w: number) => void;
  onResizing?: (resizing: boolean) => void;
}) {
  const drag = useRef<{ x: number; w: number } | null>(null);
  const clamp = (w: number) => clampWidth(w, min, window.innerWidth / 2);
  const dir = inverse ? -1 : 1;
  return (
    // biome-ignore lint/a11y/useSemanticElements: 交互式拖宽手柄，<hr> 无交互语义
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={value}
      tabIndex={0}
      className={`w-1 shrink-0 cursor-col-resize touch-none bg-transparent hover:bg-[var(--border)] ${className ?? ""}`}
      onPointerDown={(e) => {
        drag.current = { x: e.clientX, w: value };
        e.currentTarget.setPointerCapture(e.pointerId);
        onResizing?.(true);
      }}
      onPointerMove={(e) => {
        if (drag.current === null) return;
        onChange(clamp(drag.current.w + (e.clientX - drag.current.x) * dir));
      }}
      onPointerUp={(e) => {
        drag.current = null;
        e.currentTarget.releasePointerCapture(e.pointerId);
        onResizing?.(false);
      }}
      onPointerCancel={() => {
        drag.current = null;
        onResizing?.(false);
      }}
      onDoubleClick={() => onChange(min)}
      onKeyDown={(e) => {
        if (e.key === "ArrowLeft") onChange(clamp(value - 16 * dir));
        if (e.key === "ArrowRight") onChange(clamp(value + 16 * dir));
      }}
    />
  );
}
