/**
 * 会话工作区（16 号票，FR-01-01/02 多会话多 Tab + §10.7 左栏）：
 *
 * - 左栏：临时会话区 + 项目分组会话树、新建（临时会话/项目/项目下会话）、
 *   标题本地过滤、重命名/删除、项目删除（名下会话转临时）；
 * - 顶栏：拖拽区 + Tab 条（运行中指示、关闭）；
 * - Tab 面：每会话一个 keep-mounted SessionTab（独立 runtime/草稿/运行态/
 *   滚动锚点；非活跃仅隐藏，run 帧照收）。
 */

import {
  AssistantRuntimeProvider,
  AuiConfig,
  Tools,
  useAuiState,
  useLocalRuntime,
} from "@assistant-ui/react";
import { useEffect, useMemo, useState } from "react";
import { AskPanel } from "@/chat/AskPanel";
import { useBackendEvents } from "@/chat/event-bridge";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { sessionAdapter } from "@/chat/runtime";
import { SessionScopeContext } from "@/chat/session-scope";
import { ChatThread } from "@/chat/Thread";
import { cwToolkit } from "@/chat/toolkit";
import { loadedTreeToExported } from "@/chat/tree-persistence";
import { ConfirmDialog } from "@/components/ui/confirm";
import { Input } from "@/components/ui/input";
import {
  createSession,
  deleteProject,
  deleteSession,
  renameSession,
  saveProject,
  selectDir,
} from "@/ipc/client";
import type { ProjectMeta, SessionMeta } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { loadHistory, useSessionPersistStore } from "@/stores/session";
import { useSessionsStore } from "@/stores/sessions";

// 正式工具卡装配（S1 实测钉死：Tools 是 resource 工厂，经 AuiConfig 注入；
// config 为纯数据可提模块级，各 SessionTab 共用同一份）。
const cwAuiConfig = AuiConfig({ tools: Tools({ toolkit: cwToolkit }) });

export function SessionWorkspace() {
  useBackendEvents();
  const ready = useSessionsStore((s) => s.ready);
  const tabs = useSessionsStore((s) => s.tabs);
  const activeId = useSessionsStore((s) => s.activeId);
  const setActive = useSessionsStore((s) => s.setActive);
  const closeTab = useSessionsStore((s) => s.closeTab);

  useEffect(() => {
    void useSessionsStore
      .getState()
      .bootstrap()
      .catch((err) => console.error("[sessions] 启动恢复失败", err));
  }, []);

  if (!ready) {
    return (
      <div className="flex h-dvh items-center justify-center bg-neutral-950 text-sm text-neutral-500">
        正在恢复会话…
      </div>
    );
  }
  return (
    <div className="flex h-dvh bg-neutral-950 text-neutral-100">
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col">
        <header
          data-tauri-drag-region
          className="flex h-10 shrink-0 cursor-default select-none items-center bg-neutral-900 pl-2 text-xs text-neutral-400"
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
            <SessionTab key={tab.id} sessionId={tab.id} active={tab.id === activeId} />
          ))}
        </div>
      </div>
    </div>
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
        active ? "bg-neutral-950 text-neutral-100" : "text-neutral-500 hover:text-neutral-300"
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
        className="ml-0.5 hidden rounded px-0.5 text-neutral-500 hover:bg-neutral-800 group-hover:block"
      >
        ×
      </button>
    </div>
  );
}

/** 单会话 Tab 面（keep-mounted：非活跃 display:none，运行态与帧流不断）。 */
function SessionTab({ sessionId, active }: { sessionId: string; active: boolean }) {
  const adapter = useMemo(() => sessionAdapter(sessionId), [sessionId]);
  const runtime = useLocalRuntime(adapter);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const tree = await loadHistory(sessionId);
        if (cancelled || tree === null) return;
        runtime.thread.import(loadedTreeToExported(tree));
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
          <ChatThread />
          <AskPanel />
          <RunningReporter sessionId={sessionId} />
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

/** 左栏（§10.7）：搜索 + 临时会话区 + 项目分组会话树。 */
function Sidebar() {
  const [filter, setFilter] = useState("");
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
    <aside
      data-testid="session-sidebar"
      className="flex w-60 shrink-0 flex-col border-r border-neutral-800 bg-neutral-900"
    >
      <div className="flex items-center gap-1 p-2">
        <button
          type="button"
          data-testid="new-temp-session"
          onClick={() => void createAndOpen(undefined)}
          className="flex-1 rounded bg-sky-700 px-2 py-1 text-xs text-white hover:bg-sky-600"
        >
          + 新会话
        </button>
        <button
          type="button"
          data-testid="new-project"
          onClick={() => void createProject()}
          className="rounded border border-neutral-700 px-2 py-1 text-xs text-neutral-300 hover:border-neutral-500"
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

async function createAndOpen(projectId: string | undefined): Promise<void> {
  try {
    const meta = await createSession(projectId);
    useSessionsStore.getState().openTab(meta);
    await useSessionsStore.getState().refresh();
  } catch (err) {
    console.error("[sessions] 新建会话失败", err);
  }
}

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

async function client_deleteProject(projectId: string): Promise<void> {
  try {
    await deleteProject(projectId);
    await useSessionsStore.getState().refresh();
  } catch (err) {
    console.error("[sessions] 删除项目失败", err);
  }
}

// ---- 小件 ----

function Section({
  title,
  count,
  children,
  onAddSession,
  onDeleteProject,
}: {
  title: string;
  count: number;
  children: React.ReactNode;
  onAddSession?: () => void;
  onDeleteProject?: () => void;
}) {
  return (
    <div className="mb-2">
      <div className="group flex items-center gap-1 px-1 py-0.5 text-[11px] text-neutral-500">
        <span className="truncate">{title}</span>
        <span className="text-neutral-700">{count}</span>
        {onAddSession && (
          <button
            type="button"
            aria-label="在项目下新建会话"
            onClick={onAddSession}
            className="ml-auto hidden rounded px-1 hover:bg-neutral-800 group-hover:block"
          >
            +
          </button>
        )}
        {onDeleteProject && (
          <button
            type="button"
            aria-label="删除项目"
            onClick={onDeleteProject}
            className={`hidden rounded px-1 hover:bg-neutral-800 group-hover:block ${onAddSession ? "" : "ml-auto"}`}
          >
            🗑
          </button>
        )}
      </div>
      {children}
    </div>
  );
}

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
        active ? "bg-neutral-800 text-neutral-100" : "text-neutral-400 hover:bg-neutral-800/60"
      }`}
    >
      <button type="button" className="min-w-0 flex-1 truncate text-left" onClick={onOpen}>
        {meta.title}
      </button>
      <button
        type="button"
        aria-label="重命名会话"
        onClick={() => {
          setTitle(meta.title);
          setEditing(true);
        }}
        className="hidden rounded px-0.5 hover:bg-neutral-700 group-hover:block"
      >
        ✎
      </button>
      <button
        type="button"
        aria-label="删除会话"
        onClick={onDelete}
        className="hidden rounded px-0.5 hover:bg-neutral-700 group-hover:block"
      >
        ×
      </button>
    </div>
  );
}
