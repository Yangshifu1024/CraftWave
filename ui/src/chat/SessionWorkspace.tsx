/**
 * 会话工作区（16 号票 FR-01-01/02 + 17 号票三栏 FR-13-01；界面像素级
 * 复刻 ZCode 桌面端主界面——类名与结构对照其 packages/ui 源码）：
 *
 * - 外壳：磨砂灰底（bg-background-alt）上浮一块圆角内容面板（mac 12px
 *   圆角 + 1px 边框，缝隙露出灰底——ZCode 桌面端的显著观感）；
 * - 左栏：264px 基准（自研指针拖拽调宽，CSS 变量驱动，折叠 200ms 过渡），
 *   顶部 h-12 让位给 macOS 原生交通灯（titleBarStyle Overlay）+ 左上悬浮
 *   窗控组（交通灯右侧 96px 安全区起），依次是新建任务/自动化/插件市场
 *   入口、任务列表节头（标题过滤）、项目分组会话树、任务分组（临时会话）、
 *   底部账号区（设置/用量双入口）；
 * - 中栏：WorkspaceHeader（h-12 拖拽区 + 会话标题 + 右栏开关）+ 每会话
 *   keep-mounted SessionTab（独立 runtime/草稿/运行态；非活跃仅隐藏）；
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
  Blocks,
  CalendarClock,
  ChevronDown,
  ChevronRight,
  Ellipsis,
  Folder,
  FolderOpen,
  ListChevronsDownUp,
  ListChevronsUpDown,
  ListFilter,
  Loader,
  MessageCirclePlus,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRight,
  PanelRightClose,
  PenLine,
  Plus,
  Search,
  Settings,
  Trash2,
  User,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AskPanel } from "@/chat/AskPanel";
import { useBackendEvents } from "@/chat/event-bridge";
import { Inspector } from "@/chat/Inspector";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { sessionAdapter } from "@/chat/runtime";
import { ScrollFadeViewport } from "@/chat/ScrollFadeViewport";
import { RunSummariesContext, SessionScopeContext } from "@/chat/session-scope";
import { TaskTitleOverflowText } from "@/chat/TaskTitleOverflowText";
import { ChatThread } from "@/chat/Thread";
import { cwToolkit } from "@/chat/toolkit";
import type { RunSummary } from "@/chat/tree-persistence";
import { deriveRunSummaries, loadedTreeToExported } from "@/chat/tree-persistence";
import { ConfirmDialog } from "@/components/ui/confirm";
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
import { SettingsPage } from "@/settings/SettingsPage";
import { useAskStore } from "@/stores/ask";
import { loadHistory, useSessionPersistStore } from "@/stores/session";
import { useSessionsStore } from "@/stores/sessions";

// 正式工具卡装配（S1 实测钉死：Tools 是 resource 工厂，经 AuiConfig 注入；
// config 为纯数据可提模块级，各 SessionTab 共用同一份）。
const cwAuiConfig = AuiConfig({ tools: Tools({ toolkit: cwToolkit }) });

/** 侧栏宽度常量（对照 ZCode WorkspaceShellLayout.tsx:125-133）。 */
const SIDEBAR_DEFAULT_WIDTH_PX = 264;
const SIDEBAR_MIN_WIDTH_PX = 264;
const SIDEBAR_MAX_WIDTH_RATIO = 0.5;
const SIDEBAR_WIDTH_STORAGE_KEY = "craftwave:workspace-shell:sidebar-width-px";
/** macOS 交通灯右侧安全区（ZCode MACOS_TRAFFIC_LIGHT_BASE_LEFT_PADDING_PX）。 */
const MAC_TRAFFIC_LIGHT_LEFT_PADDING_PX = 96;
/** 侧栏收起后 header 左侧让位（ZCode resolveTopOverlayLeadingPaddingClass 的 mac pl-58）。 */
const HEADER_LEADING_PADDING_COLLAPSED_PX = 232;

function clampSidebarWidth(width: number): number {
  const max = Math.max(
    SIDEBAR_MIN_WIDTH_PX,
    Math.floor(window.innerWidth * SIDEBAR_MAX_WIDTH_RATIO),
  );
  return Math.min(Math.max(Math.round(width), SIDEBAR_MIN_WIDTH_PX), max);
}

function loadSidebarWidth(): number {
  try {
    const saved = Number(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY));
    if (Number.isFinite(saved) && saved > 0) return clampSidebarWidth(saved);
  } catch {
    // localStorage 不可用（隐私模式等）：用默认宽度
  }
  return SIDEBAR_DEFAULT_WIDTH_PX;
}

/** 会话工作区根组件：左栏 + 会话面板 + 右栏用量面板。
 *  挂载时恢复会话列表，未就绪前显示加载占位。 */
export function SessionWorkspace() {
  useBackendEvents();
  const { exitDialog } = useHostEvents();
  const ready = useSessionsStore((s) => s.ready);
  const tabs = useSessionsStore((s) => s.tabs);
  const activeId = useSessionsStore((s) => s.activeId);
  const inspectorOpen = useSessionsStore((s) => s.inspectorOpen);
  const setInspectorOpen = useSessionsStore((s) => s.setInspectorOpen);
  // 设置页浮层（17 号后从 App 顶层挪入工作区；入口在左栏底部账号区）
  const [settingsOpen, setSettingsOpen] = useState(false);
  // 侧栏几何：宽度（本地持久化）与折叠态（200ms 过渡由面板类名承担）
  const [sidebarWidth, setSidebarWidth] = useState(loadSidebarWidth);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [resizing, setResizing] = useState(false);
  const shellRef = useRef<HTMLDivElement>(null);
  const dragState = useRef<{ startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    void useSessionsStore
      .getState()
      .bootstrap()
      .catch((err) => console.error("[sessions] 启动恢复失败", err));
  }, []);

  // ⌘N 新建任务（左栏按钮的同键位路径）
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "n") {
        event.preventDefault();
        void createAndOpen(undefined);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // 侧栏拖拽调宽：拖动中直接写 CSS 变量（不经 React，避免每帧重渲染），
  // 松手后才 setState + 持久化——对照 ZCode applyWorkspaceSidebarWidthDuringDrag。
  const onResizeStart = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      event.preventDefault();
      dragState.current = { startX: event.clientX, startWidth: sidebarWidth };
      setResizing(true);
      const onMove = (moveEvent: PointerEvent) => {
        const drag = dragState.current;
        if (drag === null) return;
        const next = clampSidebarWidth(drag.startWidth + (moveEvent.clientX - drag.startX));
        shellRef.current?.style.setProperty("--sidebar-panel-width", `${next}px`);
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        dragState.current = null;
        setResizing(false);
        setSidebarWidth((current) => {
          const inline = shellRef.current?.style.getPropertyValue("--sidebar-panel-width");
          const parsed = inline ? Number.parseFloat(inline) : current;
          const next = Number.isFinite(parsed) ? clampSidebarWidth(parsed) : current;
          try {
            localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(next));
          } catch {
            // localStorage 不可用：本次会话内仍生效
          }
          return next;
        });
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [sidebarWidth],
  );

  const toggleSidebar = useCallback(() => setSidebarCollapsed((v) => !v), []);

  if (!ready) {
    return (
      <div className="flex h-dvh items-center justify-center bg-background text-sm text-foreground-subtle">
        正在恢复会话…
      </div>
    );
  }
  const activeMeta = tabs.find((tab) => tab.id === activeId);
  const panelWidth = sidebarCollapsed ? 0 : sidebarWidth;
  return (
    // 外壳：灰底透出内容面板的圆角缝隙（ZCode DesktopWindowFrame + bg-background-alt）
    <div className="flex h-dvh flex-col overflow-hidden border-border text-foreground bg-background-alt">
      <div className="relative min-h-0 w-full flex-1">
        <div
          ref={shellRef}
          data-workspace-shell="true"
          data-workspace-sidebar-resizing={resizing || undefined}
          style={{ "--sidebar-panel-width": `${panelWidth}px` } as React.CSSProperties}
          className="relative flex h-full min-h-0 w-full overflow-hidden"
        >
          {/* 侧栏面板：折叠 = 宽度收到 0 + 淡出，200ms ease-out（ZCode:2267） */}
          <div
            data-workspace-sidebar-panel="true"
            className={`w-[var(--sidebar-panel-width)] max-w-[50%] flex-none overflow-hidden transition-[width,opacity] duration-200 ease-out data-[workspace-sidebar-resizing=true]:transition-opacity ${
              sidebarCollapsed ? "pointer-events-none opacity-0" : "opacity-100"
            }`}
          >
            <Sidebar
              onOpenSettings={() => setSettingsOpen(true)}
              onCreateProject={() => void createProject()}
              onCreateSession={(projectId) => void createAndOpen(projectId)}
            />
          </div>
          {/* 拖拽把手：4px 命中区，悬停露出 2px 天蓝竖条（ZCode:2313）。
              可拖拽调宽 + 方向键步进 16px（ZCode 键盘步进同款）+ 双击复位；
              separator 语义无法用 <hr> 表达（需可交互），抑制该建议。 */}
          {/* biome-ignore lint/a11y/useSemanticElements: 可交互分隔符需保留 div（hr 不可聚焦） */}
          <div
            data-testid="sidebar-resize-handle"
            role="separator"
            aria-orientation="vertical"
            aria-label="拖拽调整侧栏宽度（双击复位）"
            aria-valuenow={sidebarWidth}
            aria-valuemin={SIDEBAR_MIN_WIDTH_PX}
            tabIndex={0}
            onKeyDown={(event) => {
              if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
                event.preventDefault();
                const delta = event.key === "ArrowLeft" ? -16 : 16;
                const next = clampSidebarWidth(sidebarWidth + delta);
                setSidebarWidth(next);
                try {
                  localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(next));
                } catch {
                  // localStorage 不可用：本次会话内仍生效
                }
              }
            }}
            onPointerDown={onResizeStart}
            onDoubleClick={() => {
              setSidebarWidth(SIDEBAR_DEFAULT_WIDTH_PX);
              try {
                localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(SIDEBAR_DEFAULT_WIDTH_PX));
              } catch {
                // localStorage 不可用：本次会话内仍生效
              }
            }}
            className="group/handle relative z-10 flex h-full w-1 shrink-0 touch-none cursor-ew-resize items-center justify-center bg-transparent outline-none"
          >
            <span className="pointer-events-none absolute inset-y-2 w-0.5 rounded-full bg-resize-handle opacity-0 transition-opacity group-hover/handle:opacity-100 group-focus-visible/handle:opacity-100" />
          </div>
          {/* 内容列：桌面端右/下留 4px 缝隙露出灰底（ZCode:2349） */}
          <div className="flex min-w-[320px] flex-1 flex-col platform-mac-desktop:pr-1 platform-mac-desktop:pb-1">
            {/* 顶部窗控让位条（ZCode:2360） */}
            <div className="h-1 w-full" data-tauri-drag-region />
            {/* 会话面板：浮在灰底上的圆角内容块（ZCode:2407 + mac 圆角 12px） */}
            <div className="relative flex h-full min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-border bg-background">
              <WorkspaceHeader
                title={activeMeta?.title ?? "CraftWave"}
                inspectorOpen={inspectorOpen}
                sidebarCollapsed={sidebarCollapsed}
                onToggleInspector={() => setInspectorOpen(!inspectorOpen)}
                onRename={async (next) => {
                  if (activeMeta === undefined || !next.trim() || next === activeMeta.title) return;
                  try {
                    await renameSession(activeMeta.id, next.trim());
                    await useSessionsStore.getState().refresh();
                  } catch (err) {
                    console.error("[sessions] 重命名失败", err);
                  }
                }}
                onDelete={
                  activeMeta === undefined
                    ? undefined
                    : () => void deleteSessionAndClose(activeMeta.id)
                }
              />
              <div className="relative min-h-0 flex-1">
                {tabs.map((tab) => (
                  <SessionTab
                    key={tab.id}
                    sessionId={tab.id}
                    active={tab.id === activeId}
                    inspectorOpen={inspectorOpen}
                  />
                ))}
                {tabs.length === 0 && (
                  <div className="flex h-full items-center justify-center text-sm text-foreground-subtle">
                    从左侧选择或新建一个任务
                  </div>
                )}
              </div>
            </div>
          </div>
          {/* 左上悬浮窗控组：交通灯右侧 96px 安全区起（ZCode DesktopTopOverlay） */}
          <DesktopTopOverlay collapsed={sidebarCollapsed} onToggleSidebar={toggleSidebar} />
        </div>
      </div>
      {exitDialog}
      {settingsOpen && <SettingsPage onClose={() => setSettingsOpen(false)} />}
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

/** 单会话 Tab 面（keep-mounted：非活跃 display:none，运行态与帧流不断）。
 *  右栏 Inspector 只挂在活跃 Tab 内（FR-13-01：按活跃会话取数 + 随 Tab 切换）。 */
function SessionTab({
  sessionId,
  active,
  inspectorOpen,
}: {
  sessionId: string;
  active: boolean;
  inspectorOpen: boolean;
}) {
  const adapter = useMemo(() => sessionAdapter(sessionId), [sessionId]);
  // unstable_enableMessageQueue（实验性消息队列，FR-01-07 排队面）：运行中
  // 发送的消息先入 composer 队列展示，当前 run 结束后自动依次发送——
  // 避开同会话并发 run（后端 E_SESSION_RUNNING）。Steer 立即注入随 §5.5 票。
  const runtime = useLocalRuntime(adapter, { unstable_enableMessageQueue: true });
  // run 收口摘要（17 号票折叠分组数据面）：历史收养后按 runMarker 推导
  const [summaries, setSummaries] = useState<Map<string, RunSummary>>(() => new Map());
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const tree = await loadHistory(sessionId);
        if (cancelled || tree === null) return;
        runtime.thread.import(loadedTreeToExported(tree));
        setSummaries(deriveRunSummaries(tree.messages));
      } catch (err) {
        console.warn("[session] 历史收养失败（以空会话继续）", err);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [runtime, sessionId]);
  return (
    <div className={active ? "absolute inset-0 flex" : "hidden"}>
      <AssistantRuntimeProvider runtime={runtime} config={cwAuiConfig}>
        <SessionScopeContext.Provider value={sessionId}>
          <RunSummariesContext.Provider value={summaries}>
            <ChatThread />
            <AskPanel />
            {active && inspectorOpen && <Inspector />}
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

// ---- 中栏顶部标题栏（对照 ZCode WorkspaceHeader / WorkspaceHeaderSections） ----

/** 中栏标题栏：h-12 + 底部 1px 半透明分隔线，整条是窗口拖拽区；
 *  左侧工作区图标 + 会话标题（semibold）+ ⋯ 菜单，右侧右栏开关。 */
function WorkspaceHeader({
  title,
  inspectorOpen,
  sidebarCollapsed,
  onToggleInspector,
  onRename,
  onDelete,
}: {
  title: string;
  inspectorOpen: boolean;
  sidebarCollapsed: boolean;
  onToggleInspector: () => void;
  onRename: (next: string) => Promise<void>;
  onDelete?: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);
  const [menuOpen, setMenuOpen] = useState(false);
  const commitRename = () => {
    setEditing(false);
    void onRename(draft);
  };
  return (
    <header className="@container/workspace-header relative flex h-12 w-full shrink-0 border-b border-border/50">
      <div
        data-tauri-drag-region
        className="flex h-12 min-w-0 flex-1 items-center justify-between gap-2 overflow-hidden p-2 transition-[padding] duration-300"
        style={sidebarCollapsed ? { paddingLeft: HEADER_LEADING_PADDING_COLLAPSED_PX } : undefined}
      >
        {/* 左：工作区图标 + 标题 + ⋯（ZCode WorkspaceHeaderTitleSection） */}
        <div className="flex min-w-0 items-center gap-2 overflow-hidden">
          <button
            type="button"
            title="工作区"
            className="inline-flex size-7 shrink-0 items-center justify-center rounded-lg text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
          >
            <Folder className="size-4" />
          </button>
          {editing ? (
            <Input
              autoFocus
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onBlur={commitRename}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitRename();
                if (e.key === "Escape") setEditing(false);
              }}
              className="h-7 max-w-80"
            />
          ) : (
            <>
              <h1
                data-testid="workspace-title"
                className="flex min-w-12 max-w-100 shrink items-center gap-2 truncate text-ui-base font-semibold text-foreground"
              >
                {title}
              </h1>
              <div className="relative shrink-0">
                <button
                  type="button"
                  aria-label="更多"
                  onClick={() => setMenuOpen((v) => !v)}
                  className="inline-flex size-7 shrink-0 items-center justify-center rounded-lg text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
                >
                  <Ellipsis className="size-4" />
                </button>
                {menuOpen && (
                  <ul
                    className="absolute left-0 top-full z-30 mt-1 w-48 rounded-lg border border-popover-border bg-popover p-1 shadow-md"
                    onMouseLeave={() => setMenuOpen(false)}
                  >
                    <li>
                      <button
                        type="button"
                        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-ui-base text-foreground hover:bg-menu-hover"
                        onClick={() => {
                          setMenuOpen(false);
                          setDraft(title);
                          setEditing(true);
                        }}
                      >
                        <PenLine className="size-4 text-foreground-subtle" />
                        重命名任务
                      </button>
                    </li>
                    {onDelete && (
                      <li>
                        <button
                          type="button"
                          className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-ui-base text-destructive hover:bg-destructive-surface"
                          onClick={() => {
                            setMenuOpen(false);
                            onDelete();
                          }}
                        >
                          <Trash2 className="size-4" />
                          删除会话
                        </button>
                      </li>
                    )}
                  </ul>
                )}
              </div>
            </>
          )}
        </div>
        {/* 右：右栏开关（ZCode WorkspaceSidePaneToggleButton） */}
        <div className="flex shrink-0 items-center gap-0.5">
          <button
            type="button"
            aria-label={inspectorOpen ? "收起右栏" : "展开右栏"}
            onClick={onToggleInspector}
            className="inline-flex size-7 shrink-0 items-center justify-center rounded-lg text-foreground transition-colors hover:bg-hover"
          >
            {inspectorOpen ? <PanelRightClose className="size-4" /> : <PanelRightOpenIcon />}
          </button>
        </div>
      </div>
    </header>
  );
}

function PanelRightOpenIcon() {
  return <PanelRight className="size-4" />;
}

/** 左上悬浮窗控组（对照 ZCode DesktopTopOverlay）：h-14 悬浮层，内容从
 *  交通灯右侧 96px 安全区起；侧栏可见时浮在侧栏让位区上，收起时浮在
 *  标题栏上（标题栏左侧自行让位 232px）。 */
function DesktopTopOverlay({
  collapsed,
  onToggleSidebar,
}: {
  collapsed: boolean;
  onToggleSidebar: () => void;
}) {
  return (
    <div className="pointer-events-none absolute left-0 top-0 z-20 flex h-14 w-fit">
      <div
        className="flex h-full items-center pt-1"
        style={{ paddingLeft: MAC_TRAFFIC_LIGHT_LEFT_PADDING_PX }}
      >
        <div className="pointer-events-auto flex shrink-0 items-center gap-1">
          <button
            type="button"
            aria-label={collapsed ? "展开侧栏" : "收起侧栏"}
            onClick={onToggleSidebar}
            className="inline-flex size-7 items-center justify-center rounded-lg text-foreground transition-colors hover:bg-hover"
          >
            {collapsed ? (
              <PanelLeftOpen className="size-4" />
            ) : (
              <PanelLeftClose className="size-4" />
            )}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---- 左栏（对照 ZCode ProjectSidebar，project-sidebar/index.tsx） ----

/** 左栏：交通灯让位区 + 导航入口 + 任务列表（项目/任务两节）+ 底部账号区。
 *  面板自身无背景，透出外壳灰底（ZCode 侧栏面板透明 + vibrancy 的不透明兜底）。 */
function Sidebar({
  onOpenSettings,
  onCreateProject,
  onCreateSession,
}: {
  onOpenSettings: () => void;
  onCreateProject: () => void;
  onCreateSession: (projectId: string | undefined) => void;
}) {
  const [filter, setFilter] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  // 项目行展开态（受控，提升到侧栏）：支撑节头的「全部展开/折叠」联动
  const [expandedProjects, setExpandedProjects] = useState<Record<string, boolean>>({});
  const [confirm, setConfirm] = useState<
    { kind: "session"; session: SessionMeta } | { kind: "project"; project: ProjectMeta } | null
  >(null);
  const projects = useSessionsStore((s) => s.projects);
  const sessions = useSessionsStore((s) => s.sessions);
  const activeId = useSessionsStore((s) => s.activeId);
  const openTab = useSessionsStore((s) => s.openTab);

  const needle = filter.trim().toLowerCase();
  const visible = needle
    ? sessions.filter((m) => m.title.toLowerCase().includes(needle))
    : sessions;
  const temporaries = visible.filter((m) => m.projectId === undefined);
  const byProject = new Map<string, SessionMeta[]>();
  for (const meta of visible) {
    if (meta.projectId === undefined) continue;
    byProject.set(meta.projectId, [...(byProject.get(meta.projectId) ?? []), meta]);
  }
  return (
    <aside data-testid="session-sidebar" className="flex h-full min-h-0 flex-col overflow-hidden">
      {/* 顶部 48px：macOS 原生交通灯区 + 悬浮窗控落点，整条为窗口拖拽区 */}
      <div className="h-12 shrink-0" data-tauri-drag-region />
      {/* 顶部导航块（ZCode ProjectSidebarNavigation：px-2 py-3 flex-col gap-1） */}
      <div className="flex flex-col gap-1 px-2 py-3">
        <button
          type="button"
          data-testid="new-temp-session"
          onClick={() => onCreateSession(undefined)}
          className="group inline-flex h-8 w-full shrink-0 cursor-pointer items-center justify-stretch gap-2 overflow-hidden rounded-lg pl-2.5 pr-2.5 hover:bg-surface-hover hover:text-foreground active:translate-y-0"
        >
          <span className="flex min-w-0 flex-1 items-center gap-2 text-ui-base">
            <MessageCirclePlus className="h-4 w-4 shrink-0" />
            <span className="truncate">新建任务</span>
            <span className="ml-auto shrink-0 text-ui-xs font-normal text-foreground-subtlest">
              ⌘N
            </span>
          </span>
        </button>
        <button
          type="button"
          disabled
          title="即将开放"
          className="inline-flex h-8 w-full shrink-0 items-center justify-start gap-2 rounded-lg px-2.5 text-ui-base text-foreground opacity-50 hover:bg-hover"
        >
          <CalendarClock className="size-4 shrink-0" />
          <span className="truncate">自动化</span>
        </button>
        <button
          type="button"
          disabled
          title="即将开放"
          className="inline-flex h-8 w-full shrink-0 items-center justify-start gap-2 rounded-lg px-2.5 text-ui-base text-foreground opacity-50 hover:bg-hover"
        >
          <Blocks className="size-4 shrink-0" />
          <span className="truncate">插件市场</span>
        </button>
      </div>
      {/* 任务列表区：导航区与列表区之间 1px 半透明分割线（ZCode index.tsx:232） */}
      <div className="flex min-h-0 flex-1 flex-col border-t border-border/50 pt-3 [anchor-name:--sidebar-task-list-divider]">
        {/* 列表节头：标题 + 搜索 + 筛选（ZCode ProjectSidebarListHeader） */}
        <div className="flex min-w-0 shrink-0 items-center gap-1 px-2 pb-2">
          {searchOpen ? (
            <div className="min-w-0 flex-1 px-1">
              <Input
                autoFocus
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    setFilter("");
                    setSearchOpen(false);
                  }
                }}
                placeholder="搜索任务标题…"
                className="h-7"
              />
            </div>
          ) : (
            <span className="min-w-0 flex-1 truncate px-2.5 text-ui-base font-medium text-foreground-subtle">
              任务列表
            </span>
          )}
          <button
            type="button"
            aria-label="搜索任务"
            onClick={() => {
              if (searchOpen) {
                setFilter("");
                setSearchOpen(false);
              } else {
                setSearchOpen(true);
              }
            }}
            className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
          >
            {searchOpen ? <X className="size-3.5" /> : <Search className="size-3.5" />}
          </button>
          <button
            type="button"
            aria-label="切换视图"
            disabled
            title="即将开放"
            className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle opacity-50"
          >
            <ListFilter className="size-3.5" />
          </button>
        </div>
        {/* 滚动列表：项目节 + 任务节，节间距 gap-3；滚动容器用 ZCode
            ScrollFadeViewport 同款渐隐（index.tsx:246 也是 overflow-x-hidden） */}
        <ScrollFadeViewport className="overflow-x-hidden">
          <div className="flex flex-col gap-3 pb-3">
            <PurposeSection
              title="项目"
              actions={
                <>
                  {projects.length > 0 && (
                    <button
                      type="button"
                      aria-label={
                        projects.some((p) => expandedProjects[p.id] ?? true)
                          ? "收起全部项目"
                          : "展开全部项目"
                      }
                      onClick={() => {
                        const hasExpanded = projects.some((p) => expandedProjects[p.id] ?? true);
                        const next: Record<string, boolean> = {};
                        for (const p of projects) next[p.id] = !hasExpanded;
                        setExpandedProjects(next);
                      }}
                      className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
                    >
                      {projects.some((p) => expandedProjects[p.id] ?? true) ? (
                        <ListChevronsDownUp className="size-3.5" />
                      ) : (
                        <ListChevronsUpDown className="size-3.5" />
                      )}
                    </button>
                  )}
                  <button
                    type="button"
                    data-testid="new-project"
                    aria-label="新项目"
                    onClick={onCreateProject}
                    className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
                  >
                    <Plus className="size-3.5" />
                  </button>
                </>
              }
            >
              {projects.map((project) => {
                const list = byProject.get(project.id) ?? [];
                return (
                  <ProjectNode
                    key={project.id}
                    project={project}
                    sessions={list}
                    activeId={activeId}
                    expanded={expandedProjects[project.id] ?? true}
                    onToggleExpanded={() =>
                      setExpandedProjects((prev) => ({
                        ...prev,
                        [project.id]: !(prev[project.id] ?? true),
                      }))
                    }
                    onOpen={openTab}
                    onAddSession={() => onCreateSession(project.id)}
                    onRename={async (name) => {
                      if (!name.trim() || name === project.name) return;
                      try {
                        await saveProject({
                          project: { id: project.id, name: name.trim(), homeDir: project.homeDir },
                        });
                        await useSessionsStore.getState().refresh();
                      } catch (err) {
                        console.error("[sessions] 项目改名失败", err);
                      }
                    }}
                    onDelete={() => setConfirm({ kind: "project", project })}
                  />
                );
              })}
            </PurposeSection>
            <PurposeSection
              title="任务"
              actions={
                <button
                  type="button"
                  aria-label="新建任务"
                  onClick={() => onCreateSession(undefined)}
                  className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
                >
                  <MessageCirclePlus className="size-3.5" />
                </button>
              }
            >
              <div className="pl-2">
                <SessionRows
                  sessions={temporaries}
                  activeId={activeId}
                  onOpen={openTab}
                  onRename={async (meta, title) => {
                    if (!title.trim() || title === meta.title) return;
                    try {
                      await renameSession(meta.id, title.trim());
                      await useSessionsStore.getState().refresh();
                    } catch (err) {
                      console.error("[sessions] 重命名失败", err);
                    }
                  }}
                  onDelete={(meta) => setConfirm({ kind: "session", session: meta })}
                />
              </div>
            </PurposeSection>
          </div>
        </ScrollFadeViewport>
      </div>
      {/* 底部账号区（ZCode WorkspaceSidebarFooter：px-4 pt-2 pb-4） */}
      <div className="flex shrink-0 flex-col gap-2.5 pb-4 pl-4 pr-3 pt-2">
        <div className="flex min-w-0 gap-2">
          <button
            type="button"
            className="inline-flex h-8 min-w-0 flex-1 items-center justify-start gap-2 overflow-hidden rounded-tl-2xl rounded-bl-2xl pl-0 text-ui-base transition-colors hover:bg-hover"
          >
            <span className="relative flex size-8 shrink-0 items-center justify-center rounded-full bg-background text-foreground after:absolute after:inset-0 after:rounded-full after:border after:border-border after:mix-blend-darken">
              <User className="size-4" />
            </span>
            <span className="min-w-0 truncate text-ui-base font-semibold text-foreground">
              CraftWave
            </span>
          </button>
          <div className="flex shrink-0 items-center gap-1.5">
            <SettingsFooterButton onOpenSettings={onOpenSettings} />
          </div>
        </div>
      </div>
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

/** footer 右侧设置齿轮（ZCode ghost icon-lg + Settings size-4）。 */
function SettingsFooterButton({ onOpenSettings }: { onOpenSettings: () => void }) {
  return (
    <button
      type="button"
      data-testid="open-settings"
      aria-label="设置"
      onClick={onOpenSettings}
      className="inline-flex size-8 shrink-0 items-center justify-center rounded-lg text-foreground transition-colors hover:bg-hover"
    >
      <Settings className="size-4" />
    </button>
  );
}

// ---- 任务列表两节（对照 ZCode WorkspacePurposeSection） ----

/** 节容器（对照 ZCode WorkspacePurposeSection）：28px 头行 = 折叠按钮
 *  （标题 truncate span 在前、三角紧跟其后，hover/focus 显现）+ 右侧动作区
 *  （hover/focus/菜单开启时显现）；折叠态直接不渲染子项。 */
function PurposeSection({
  title,
  actions,
  children,
}: {
  title: string;
  actions?: React.ReactNode;
  children: React.ReactNode;
}) {
  const [expanded, setExpanded] = useState(true);
  return (
    <div className="group/purpose-section relative min-w-0">
      <div className="ml-2 mr-1 flex h-7 min-w-0 items-center">
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="flex h-7 min-w-0 flex-1 touch-none items-center gap-1 px-2.5 text-left text-ui-base font-medium text-foreground-subtlest outline-none transition-colors hover:text-foreground focus-visible:text-foreground focus-visible:ring-2 focus-visible:ring-ring/30"
        >
          <span className="min-w-0 truncate">{title}</span>
          {expanded ? (
            <ChevronDown className="size-3.5 shrink-0 opacity-0 transition-opacity group-hover/purpose-section:opacity-100 group-focus-within/purpose-section:opacity-100 [@media(hover:none)]:opacity-100" />
          ) : (
            <ChevronRight className="size-3.5 shrink-0 opacity-0 transition-opacity group-hover/purpose-section:opacity-100 group-focus-within/purpose-section:opacity-100 [@media(hover:none)]:opacity-100" />
          )}
        </button>
        {actions !== undefined && (
          <div className="flex shrink-0 items-center opacity-0 transition-opacity group-hover/purpose-section:opacity-100 group-focus-within/purpose-section:opacity-100 has-[[data-state=open]]:opacity-100 [@media(hover:none)]:opacity-100">
            {actions}
          </div>
        )}
      </div>
      {/* 折叠动画：grid-rows 高度过渡 300ms ease-in-out（对齐 ZCode
          Collapsible 的 animate-collapsible-down/up） */}
      <div
        className={`grid transition-[grid-template-rows] duration-300 ease-in-out ${
          expanded ? "grid-rows-[1fr]" : "grid-rows-[0fr]"
        }`}
      >
        <div className="min-w-0 overflow-hidden rounded-xl">{children}</div>
      </div>
    </div>
  );
}

// ---- 项目分组行（对照 ZCode ProjectNodeContent） ----

/** 项目行（对照 ZCode ProjectNodeContent）：32px，文件夹开/合图标表达展开态
 *  （ZCode 项目行无三角）+ 项目名 + hover 动作（新会话/改名/删除）；展开态由
 *  侧栏受控（支撑节头「全部展开/折叠」）；子会话容器 pl-2（8px 缩进，无竖线）。 */
function ProjectNode({
  project,
  sessions,
  activeId,
  expanded,
  onToggleExpanded,
  onOpen,
  onAddSession,
  onRename,
  onDelete,
}: {
  project: ProjectMeta;
  sessions: SessionMeta[];
  activeId: string | null;
  expanded: boolean;
  onToggleExpanded: () => void;
  onOpen: (meta: SessionMeta) => void;
  onAddSession: () => void;
  onRename: (name: string) => Promise<void>;
  onDelete: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(project.name);
  const [menuOpen, setMenuOpen] = useState(false);
  const commitRename = () => {
    setEditing(false);
    void onRename(name);
  };
  return (
    <div className="min-w-0 rounded-xl">
      <div className="group/project-row ml-2 flex h-8 min-w-0 items-center gap-1 rounded-lg pr-1 hover:bg-surface-hover">
        {editing ? (
          <div className="min-w-0 flex-1 px-1">
            <Input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              onBlur={commitRename}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitRename();
                if (e.key === "Escape") {
                  setName(project.name);
                  setEditing(false);
                }
              }}
              className="h-7"
            />
          </div>
        ) : (
          <button
            type="button"
            aria-expanded={expanded}
            onClick={() => onToggleExpanded()}
            className="flex h-8 min-w-0 flex-1 touch-pan-y items-center justify-start gap-2 rounded-lg pl-2.5 pr-1 text-left text-foreground hover:bg-transparent hover:text-foreground aria-expanded:bg-transparent aria-expanded:text-foreground"
          >
            <span className="flex size-4 shrink-0 items-center justify-center">
              {expanded ? (
                <FolderOpen className="size-4 shrink-0 text-foreground-subtle" />
              ) : (
                <Folder className="size-4 shrink-0 text-foreground-subtle" />
              )}
            </span>
            <span className="min-w-0 truncate text-ui-base text-foreground-subtle">
              {project.name}
            </span>
          </button>
        )}
        {/* hover 动作区（ZCode ProjectNodeContent.tsx:311：opacity-0 → 100） */}
        <div className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover/project-row:opacity-100 group-focus-within/project-row:opacity-100 [&:has([data-state=open])]:opacity-100 [@media(hover:none)]:opacity-100">
          <button
            type="button"
            aria-label="新建任务"
            onClick={onAddSession}
            className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
          >
            <MessageCirclePlus className="size-3.5" />
          </button>
          <div className="relative">
            <button
              type="button"
              aria-label="更多"
              onClick={() => setMenuOpen((v) => !v)}
              className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
            >
              <Ellipsis className="size-3.5" />
            </button>
            {menuOpen && (
              <ul
                className="absolute right-0 top-full z-30 mt-1 w-40 rounded-lg border border-popover-border bg-popover p-1 shadow-md"
                onMouseLeave={() => setMenuOpen(false)}
              >
                <li>
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-ui-base text-foreground hover:bg-menu-hover"
                    onClick={() => {
                      setMenuOpen(false);
                      setName(project.name);
                      setEditing(true);
                    }}
                  >
                    <PenLine className="size-4 text-foreground-subtle" />
                    编辑项目
                  </button>
                </li>
                <li>
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-ui-base text-destructive hover:bg-destructive-surface"
                    onClick={() => {
                      setMenuOpen(false);
                      onDelete();
                    }}
                  >
                    <Trash2 className="size-4" />
                    删除项目
                  </button>
                </li>
              </ul>
            )}
          </div>
        </div>
      </div>
      {/* 折叠动画：grid-rows 高度过渡 300ms（同节头，对齐 ZCode Collapsible） */}
      <div
        className={`grid transition-[grid-template-rows] duration-300 ease-in-out ${
          expanded ? "grid-rows-[1fr]" : "grid-rows-[0fr]"
        }`}
      >
        <div className="overflow-hidden">
          <div className="pl-2 pt-1">
            <SessionRows
              sessions={sessions}
              activeId={activeId}
              onOpen={onOpen}
              onRename={async (meta, title) => {
                if (!title.trim() || title === meta.title) return;
                try {
                  await renameSession(meta.id, title.trim());
                  await useSessionsStore.getState().refresh();
                } catch (err) {
                  console.error("[sessions] 重命名失败", err);
                }
              }}
              onDelete={(meta) => void deleteSessionAndClose(meta.id)}
            />
          </div>
        </div>
      </div>
    </div>
  );
}

// ---- 会话条目（对照 ZCode TaskListItem default 变体） ----

/** 分页常量：项目每页 5 条（ZCode ProjectTaskRows.tsx:98）。 */
const ROWS_PREVIEW_COUNT = 5;

/** 会话条目列表：默认显示前 5 条，超出出「显示更多」；行 = 状态槽 + 标题 +
 *  相对时间（hover 时换成操作按钮）。 */
function SessionRows({
  sessions,
  activeId,
  onOpen,
  onRename,
  onDelete,
}: {
  sessions: SessionMeta[];
  activeId: string | null;
  onOpen: (meta: SessionMeta) => void;
  onRename: (meta: SessionMeta, title: string) => Promise<void>;
  onDelete: (meta: SessionMeta) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const hidden = sessions.length - ROWS_PREVIEW_COUNT;
  const visible = expanded || hidden <= 0 ? sessions : sessions.slice(0, ROWS_PREVIEW_COUNT);
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      {visible.map((meta) => (
        <SessionRow
          key={meta.id}
          meta={meta}
          active={meta.id === activeId}
          onOpen={() => onOpen(meta)}
          onRename={onRename}
          onDelete={() => onDelete(meta)}
        />
      ))}
      {hidden > 0 && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="ml-8.5 h-auto w-fit p-0 text-left text-ui-base text-foreground-subtlest outline-none transition-colors hover:bg-transparent hover:text-foreground focus-visible:text-foreground"
        >
          {expanded ? "显示更少" : "显示更多"}
        </button>
      )}
    </div>
  );
}

/** 单行会话条目：32px；状态槽 16px（运行中 spinner / 活跃蓝点）、标题
 *  truncate、相对时间（hover 隐去换操作按钮）——ZCode TaskListItem.tsx:569。 */
function SessionRow({
  meta,
  active,
  onOpen,
  onRename,
  onDelete,
}: {
  meta: SessionMeta;
  active: boolean;
  onOpen: () => void;
  onRename: (meta: SessionMeta, title: string) => Promise<void>;
  onDelete: () => void;
}) {
  const running = useSessionsStore((s) => s.running[meta.id]);
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(meta.title);
  const commitRename = () => {
    setEditing(false);
    void onRename(meta, title);
  };
  if (editing) {
    return (
      <div className="px-1 py-0.5">
        <Input
          autoFocus
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onBlur={commitRename}
          onKeyDown={(e) => {
            if (e.key === "Enter") commitRename();
            if (e.key === "Escape") {
              setTitle(meta.title);
              setEditing(false);
            }
          }}
          className="h-7"
        />
      </div>
    );
  }
  return (
    <div
      data-testid={`session-row-${meta.id}`}
      data-task-item-key={meta.id}
      className={`group/task-item flex cursor-pointer items-center gap-2 rounded-lg py-1 pl-2.5 pr-1 transition-[background-color,border-color,box-shadow] ${
        active ? "bg-selected" : "hover:bg-surface-hover"
      }`}
    >
      {/* 状态槽（16px）：运行中 spinner，否则活跃蓝点，再否则不渲染 */}
      <span className="relative flex size-4 shrink-0 items-center justify-center">
        {running ? (
          <span
            data-testid={`session-running-${meta.id}`}
            className="flex size-4 animate-spin items-center justify-center text-foreground-subtle"
          >
            <Loader className="size-4" />
          </span>
        ) : active ? (
          <span className="size-[7px] rounded-full bg-icon-blue" />
        ) : null}
      </span>
      <button
        type="button"
        onClick={onOpen}
        className="flex h-6 min-w-0 flex-1 cursor-pointer text-left text-ui-base text-foreground"
      >
        {/* 标题溢出走马灯（ZCode TaskTitleOverflowText）：溢出时才挂渐隐
            mask，hover 行 1 秒后 40px/s 滚动，对齐后停留 2 秒。h-6 对照
            ZCode TaskListItem.tsx:729 标题行锁高——hover 换槽时操作按钮
            (24px) 高于时间文本 (~20px)，不锁高则行体 28↔32 撑缩，下方行
            被顶得上下跳。 */}
        <TaskTitleOverflowText as="span">{meta.title}</TaskTitleOverflowText>
      </button>
      {/* 时间与操作按钮同槽互换（ZCode TaskListItem 同款） */}
      <span className="mr-0.5 flex shrink-0 items-center gap-1 text-ui-sm text-foreground-subtle group-hover/task-item:hidden">
        {relativeTime(meta.updatedAt)}
      </span>
      <span className="hidden shrink-0 items-center gap-0.5 group-hover/task-item:flex">
        <button
          type="button"
          aria-label="重命名任务"
          onClick={(e) => {
            e.stopPropagation();
            setTitle(meta.title);
            setEditing(true);
          }}
          className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
        >
          <PenLine className="size-3.5" />
        </button>
        <button
          type="button"
          aria-label="删除会话"
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
          className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      </span>
    </div>
  );
}

// ---- 左栏数据操作（ IPC + store 同步；异常只 console——列表仍在） ----

/** 新建会话并打开（可选挂在某项目下），随后刷新左栏列表。 */
async function createAndOpen(projectId: string | undefined): Promise<void> {
  try {
    const meta = await createSession(projectId);
    useSessionsStore.getState().openTab(meta);
    await useSessionsStore.getState().refresh();
  } catch (err) {
    console.error("[sessions] 新建会话失败", err);
  }
}

/** 新建项目：弹目录选择器，取目录名作项目名注册。 */
async function createProject(): Promise<void> {
  try {
    const dir = await selectDir();
    if (!dir) return;
    const name = dir.split("/").filter(Boolean).at(-1) ?? dir;
    await saveProject({ project: { name, homeDir: dir } });
    await useSessionsStore.getState().refresh();
  } catch (err) {
    console.error("[sessions] 新建项目失败", err);
  }
}

/** 删除会话并收尾：关 Tab + 清各 store 里该会话的分桶（持久化/指标/ask）。 */
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

/** 相对时间（A6）：<1 分钟「刚刚」；当天「N 小时」；7 天内「N 天」；更早 MM-DD。 */
function relativeTime(updatedAt: number): string {
  const diffMs = Date.now() - updatedAt;
  const minutes = Math.floor(diffMs / 60000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes}分`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}小时`;
  const days = Math.floor(hours / 24);
  return `${days}天`;
}
