/**
 * 会话工作区（16 号票 FR-01-01/02 + 17 号票三栏 FR-13-01）：
 *
 * - 左栏：临时会话区 + 项目分组会话树、新建（临时会话/项目/项目下会话）、
 *   标题本地过滤、重命名/删除、项目删除（名下会话转临时）、项目改名
 *   （16 号票 shortcut，17 号票收敛）；
 * - 顶栏：拖拽区 + Tab 条（运行中指示、关闭）；
 * - Tab 面：每会话一个 keep-mounted SessionTab（独立 runtime/草稿/运行态/
 *   滚动锚点；非活跃仅隐藏，run 帧照收）+ 右栏 Inspector（可折叠，开合入
 *   ui-state，FR-13-01）；
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
import { useEffect, useMemo, useState } from "react";
import { AskPanel } from "@/chat/AskPanel";
import { useBackendEvents } from "@/chat/event-bridge";
import { Inspector } from "@/chat/Inspector";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { sessionAdapter } from "@/chat/runtime";
import { RunSummariesContext, SessionScopeContext } from "@/chat/session-scope";
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

/** 会话工作区根组件：左栏 + 顶栏 Tab 条 + 每会话 Tab 面 + 右栏用量面板。
 *  挂载时恢复会话列表，未就绪前显示加载占位。 */
export function SessionWorkspace() {
  useBackendEvents();
  const { exitDialog } = useHostEvents();
  const ready = useSessionsStore((s) => s.ready);
  const tabs = useSessionsStore((s) => s.tabs);
  const activeId = useSessionsStore((s) => s.activeId);
  const inspectorOpen = useSessionsStore((s) => s.inspectorOpen);
  const setActive = useSessionsStore((s) => s.setActive);
  const closeTab = useSessionsStore((s) => s.closeTab);
  // 设置页浮层（17 号后从 App 顶层挪入工作区；入口在左栏底部）
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    void useSessionsStore
      .getState()
      .bootstrap()
      .catch((err) => console.error("[sessions] 启动恢复失败", err));
  }, []);

  if (!ready) {
    return (
      <div className="flex h-dvh items-center justify-center bg-[var(--background)] text-sm text-[var(--muted-foreground)]">
        正在恢复会话…
      </div>
    );
  }
  return (
    <div className="flex h-dvh bg-[var(--background)] text-[var(--foreground)]">
      <Sidebar onOpenSettings={() => setSettingsOpen(true)} />
      <div className="flex min-w-0 flex-1 flex-col">
        <header
          data-tauri-drag-region
          className="flex h-10 shrink-0 cursor-default select-none items-center bg-[var(--card)] pl-2 text-xs text-[var(--muted-foreground)]"
          style={{
            paddingLeft: "max(var(--tauri-plugin-decoration-left-clearance, 76px), 8px)",
            paddingRight: "var(--tauri-plugin-decoration-right-clearance, 8px)",
          }}
        >
          {tabs.map((tab) => (
            <TabChip
              key={tab.id}
              meta={tab}
              active={tab.id === activeId}
              onActivate={() => setActive(tab.id)}
              onClose={() => closeTab(tab.id)}
            />
          ))}
          <span className="ml-auto pr-1">CraftWave</span>
        </header>
        <div className="relative min-h-0 flex-1">
          {tabs.map((tab) => (
            <SessionTab
              key={tab.id}
              sessionId={tab.id}
              active={tab.id === activeId}
              inspectorOpen={inspectorOpen}
            />
          ))}
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

/** Tab 页签：标题 + 运行指示 + 关闭。 */
function TabChip({
  meta,
  active,
  onActivate,
  onClose,
}: {
  meta: SessionMeta;
  active: boolean;
  onActivate: () => void;
  onClose: () => void;
}) {
  const running = useSessionsStore((s) => s.running[meta.id]);
  return (
    <div
      data-testid={`tab-${meta.id}`}
      role="tab"
      aria-selected={active}
      tabIndex={0}
      className={`group mr-1 flex max-w-40 shrink-0 cursor-default items-center gap-1 rounded-t px-2 py-1 ${
        active
          ? "bg-[var(--background)] text-[var(--foreground)]"
          : "text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
      }`}
      onClick={onActivate}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") onActivate();
      }}
    >
      {running && (
        <span
          data-testid={`tab-running-${meta.id}`}
          className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-400"
        />
      )}
      <span className="truncate text-xs">{meta.title}</span>
      <button
        type="button"
        aria-label="关闭会话"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        className="ml-0.5 hidden rounded px-0.5 text-[var(--muted-foreground)] hover:bg-[var(--accent)] group-hover:block"
      >
        ×
      </button>
    </div>
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

/** 左栏（§10.7）：搜索 + 临时会话区 + 项目分组会话树；底部「设置/用量」
 *  双入口各占一半宽度。顶部首行下移 40px 与中间 Tab 条同高——避开 macOS
 *  交通灯（红黄绿窗口按钮，占据窗口左上角约 76×32 像素）。 */
function Sidebar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const [filter, setFilter] = useState("");
  const [confirm, setConfirm] = useState<
    { kind: "session"; session: SessionMeta } | { kind: "project"; project: ProjectMeta } | null
  >(null);
  const projects = useSessionsStore((s) => s.projects);
  const sessions = useSessionsStore((s) => s.sessions);
  const activeId = useSessionsStore((s) => s.activeId);
  const inspectorOpen = useSessionsStore((s) => s.inspectorOpen);
  const setInspectorOpen = useSessionsStore((s) => s.setInspectorOpen);
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
    <aside
      data-testid="session-sidebar"
      className="flex w-60 shrink-0 flex-col border-r border-[var(--border)] bg-[var(--card)]"
    >
      {/* 顶部下移 40px（pt-10）与中间 Tab 条同基线：避开 macOS 交通灯区域 */}
      <div className="flex items-center gap-1 p-2 pt-10">
        <button
          type="button"
          data-testid="new-temp-session"
          onClick={() => void createAndOpen(undefined)}
          className="flex-1 rounded bg-[var(--primary)] px-2 py-1 text-xs text-[var(--primary-foreground)] hover:opacity-90"
        >
          + 新会话
        </button>
        <button
          type="button"
          data-testid="new-project"
          onClick={() => void createProject()}
          className="rounded border border-[var(--border)] px-2 py-1 text-xs text-[var(--muted-foreground)] hover:border-[var(--ring)]"
        >
          项目
        </button>
      </div>
      <div className="px-2 pb-2">
        <Input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="搜索会话标题…"
          className="w-full"
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        <Section title="临时会话" count={temporaries.length}>
          {temporaries.map((meta) => (
            <SessionRow
              key={meta.id}
              meta={meta}
              active={meta.id === activeId}
              onOpen={() => openTab(meta)}
              onDelete={() => setConfirm({ kind: "session", session: meta })}
            />
          ))}
        </Section>
        {projects.map((project) => {
          const list = byProject.get(project.id) ?? [];
          return (
            <Section
              key={project.id}
              title={project.name}
              count={list.length}
              project={project}
              onAddSession={() => void createAndOpen(project.id)}
              onDeleteProject={() => setConfirm({ kind: "project", project })}
            >
              {list.map((meta) => (
                <SessionRow
                  key={meta.id}
                  meta={meta}
                  active={meta.id === activeId}
                  onOpen={() => openTab(meta)}
                  onDelete={() => setConfirm({ kind: "session", session: meta })}
                />
              ))}
            </Section>
          );
        })}
      </div>
      {/* 底部双入口：设置页 + 右栏用量页开合，横向平铺各占一半宽度 */}
      <div className="flex gap-1 border-t border-[var(--border)] p-2">
        <button
          type="button"
          data-testid="open-settings"
          onClick={onOpenSettings}
          className="flex-1 rounded border border-[var(--border)] px-2 py-1 text-xs text-[var(--muted-foreground)] hover:border-[var(--ring)]"
        >
          设置
        </button>
        <button
          type="button"
          data-testid="inspector-toggle"
          aria-pressed={inspectorOpen}
          onClick={() => setInspectorOpen(!inspectorOpen)}
          className="flex-1 rounded border border-[var(--border)] px-2 py-1 text-xs text-[var(--muted-foreground)] hover:border-[var(--ring)]"
        >
          用量
        </button>
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

// ---- 左栏数据操作（ IPC + store 同步；异常只 console——列表仍在） ----

/** 新建会话并打开 Tab（可选挂在某项目下），随后刷新左栏列表。 */
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

// ---- 小件 ----

/** 左栏分组容器：标题 + 计数，项目分组时悬停露出改名/加会话/删项目按钮；
 *  改名走内联输入框（Enter 提交，Esc 取消）。 */
function Section({
  title,
  count,
  project,
  children,
  onAddSession,
  onDeleteProject,
}: {
  title: string;
  count: number;
  /** 项目分组时传入（标题行内改名，16 号票 shortcut，17 号票收敛）。 */
  project?: ProjectMeta;
  children: React.ReactNode;
  onAddSession?: () => void;
  onDeleteProject?: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(title);
  const commit = () => {
    setEditing(false);
    const trimmed = name.trim();
    if (project === undefined || !trimmed || trimmed === project.name) {
      setName(title);
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
  return (
    <div className="mb-2">
      <div className="group flex items-center gap-1 px-1 py-0.5 text-[11px] text-[var(--muted-foreground)]">
        {editing ? (
          <Input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              if (e.key === "Escape") {
                setName(title);
                setEditing(false);
              }
            }}
          />
        ) : (
          <>
            <span className="truncate">{title}</span>
            <span className="text-[var(--cw-dim)]">{count}</span>
            {project !== undefined && (
              <button
                type="button"
                aria-label="项目改名"
                onClick={() => {
                  setName(title);
                  setEditing(true);
                }}
                className="hidden rounded px-1 hover:bg-[var(--accent)] group-hover:block"
              >
                ✎
              </button>
            )}
          </>
        )}
        {onAddSession && (
          <button
            type="button"
            aria-label="在项目下新建会话"
            onClick={onAddSession}
            className={`${project === undefined ? "ml-auto" : ""} hidden rounded px-1 hover:bg-[var(--accent)] group-hover:block`}
          >
            +
          </button>
        )}
        {onDeleteProject && (
          <button
            type="button"
            aria-label="删除项目"
            onClick={onDeleteProject}
            className={`hidden rounded px-1 hover:bg-[var(--accent)] group-hover:block ${onAddSession ? "" : "ml-auto"}`}
          >
            🗑
          </button>
        )}
      </div>
      <SectionBody>{children}</SectionBody>
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
    <>
      {visible}
      {hidden > 0 && !expanded && (
        <button
          type="button"
          className="mt-0.5 w-full rounded px-1 py-0.5 text-left text-[11px] text-[var(--muted-foreground)] hover:bg-[var(--accent)]/60"
          onClick={() => setExpanded(true)}
        >
          显示更多（{hidden}）
        </button>
      )}
    </>
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

/** 左栏单行会话：点击打开 Tab；悬停露出重命名/删除按钮（重命名内联输入）。 */
function SessionRow({
  meta,
  active,
  onOpen,
  onDelete,
}: {
  meta: SessionMeta;
  active: boolean;
  onOpen: () => void;
  onDelete: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(meta.title);
  const commit = () => {
    setEditing(false);
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
              setEditing(false);
            }
          }}
        />
      </div>
    );
  }
  return (
    <div
      data-testid={`session-row-${meta.id}`}
      className={`group flex cursor-default items-center gap-1 rounded px-1 py-0.5 text-xs ${
        active
          ? "bg-[var(--accent)] text-[var(--foreground)]"
          : "text-[var(--muted-foreground)] hover:bg-[var(--accent)]/60"
      }`}
    >
      <button type="button" className="min-w-0 flex-1 truncate text-left" onClick={onOpen}>
        {meta.title}
      </button>
      {/* 相对时间（A6）：非活跃时淡显，活跃时与操作按钮共存 */}
      <span className="shrink-0 text-[10px] text-[var(--cw-dim)]">
        {relativeTime(meta.updatedAt)}
      </span>
      <button
        type="button"
        aria-label="重命名会话"
        onClick={() => {
          setTitle(meta.title);
          setEditing(true);
        }}
        className="hidden rounded px-0.5 hover:bg-[var(--accent)] group-hover:block"
      >
        ✎
      </button>
      <button
        type="button"
        aria-label="删除会话"
        onClick={onDelete}
        className="hidden rounded px-0.5 hover:bg-[var(--accent)] group-hover:block"
      >
        ×
      </button>
    </div>
  );
}
