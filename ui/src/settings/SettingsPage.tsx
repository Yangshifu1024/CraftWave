/**
 * 设置页（布局照抄 CodeWave——本项目的交互对标参考项目：全屏覆盖式 dialog）。
 *
 * 容器承担四件事：
 *   1. 左导航列（返回工作区 + 搜索 + 三组 10 页导航，页行带脏圆点。
 *      脏（dirty）= 该页草稿与已保存基线不一致；脏圆点即"有未保存改动"提示）；
 *   2. 右内容列（操作条：页标题 + 取消/保存；页体：50% 宽居中）；
 *   3. 逐页脏标记（草稿与已保存基线的字段差集，字段归属由 pageSlice 切片）；
 *   4. 离开拦截（切页 / 返回 / 页内 Esc 共用同一份"保存 / 放弃 / 留下"三选弹框）。
 * 页面 JSX 手写；注册表（settings/registry.ts）只提供页序、页名与分组。
 * M1 数据面：providers / activeModelId / ui / proxy / approval / log 真实接线；
 * 03 号票补工作区与智能体页（customPrompt + 项目数据目录入口）与关于/日志页
 * 的目录入口（open_dir）；37 号票补看门狗停滞超时；08 号票补 MCP 管理页
 * （McpPage：双作用域配置编辑 / 启停 / 测试连接 / mcp:status 状态徽标）；
 * 09 号票补技能管理页（SkillsPage：skill_list 列表 / 启停 / 重扫 / 预览）。
 * 写入后检查页保留导航占位，随后续票接线。
 */

import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { cn } from "@/components/ui/cn";
import { Dialog } from "@/components/ui/dialog";
import { Input, Textarea } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { applyHotkey, eventToAccelerator, formatAccelerator, useHotkeyStore } from "@/hotkey";
import { setLang, t, useT } from "@/i18n/core";
import {
  listLogFiles,
  notificationPermissionStatus,
  openDir,
  openLogsDir,
  openNotificationSettings,
  readSessionLog,
  searchTest,
} from "@/ipc/client";
import type { AppConfig, NotificationPermissionStatus, SearchTestResult } from "@/ipc/types";
import {
  ArrowLeftIcon,
  BlocksIcon,
  BotIcon,
  CheckSquareIcon,
  FileTextIcon,
  GlobeIcon,
  InfoIcon,
  PaletteIcon,
  PlugIcon,
  SearchIcon,
  ShieldIcon,
  ZapIcon,
} from "@/settings/icons";
import { McpPage } from "@/settings/McpPage";
import { ProvidersPage } from "@/settings/ProvidersPage";
import {
  DEFAULT_PAGE,
  normalizePageKey,
  PAGE_GROUPS,
  PAGE_LABEL,
  PAGE_ORDER,
  type PageKey,
} from "@/settings/registry";
import { SkillsPage } from "@/settings/SkillsPage";
import { UpdateSection } from "@/settings/UpdateSection";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { pushToast } from "@/stores/toast";
import { applyTheme } from "@/theme";
import { applyUiFontSize } from "@/ui-font-size";

/** 页图标（线性、跟随文本色，与 CodeWave 口径一致） */
const PAGE_ICON: Record<PageKey, (props: { className?: string }) => ReactNode> = {
  appearance: PaletteIcon,
  providers: PlugIcon,
  network: GlobeIcon,
  security: ShieldIcon,
  mcp: BlocksIcon,
  skills: ZapIcon,
  tools: CheckSquareIcon,
  agent: BotIcon,
  logs: FileTextIcon,
  about: InfoIcon,
};

/** 各页的字段归属（脏判定切片；null = 该页无配置字段，恒不脏） */
function pageSlice(config: AppConfig, page: PageKey): unknown {
  switch (page) {
    case "appearance":
      return config.ui;
    case "providers":
      return { providers: config.providers, activeModelId: config.activeModelId ?? null };
    case "network":
      // 网络页承载 proxy + search（13 号票：SearXNG 配置组 + Tavily Key）
      return { proxy: config.proxy ?? null, search: config.search };
    case "security":
      return config.approval;
    case "agent":
      // 工作区与智能体页承载 agent + watchdog（37 号票）两组字段
      return { agent: config.agent ?? null, watchdog: config.watchdog };
    case "logs":
      return config.log;
    default:
      return null;
  }
}

/** 两段配置切片是否等价（先按空值三态归一再比 JSON，键序无关） */
function sameSlice(a: unknown, b: unknown): boolean {
  return JSON.stringify(normalizeForCompare(a)) === JSON.stringify(normalizeForCompare(b));
}

/** 比较前的归一化，目的是消除"假脏"：
 *  - null / undefined / 空白字符串统一折叠为 null（"没填"与"填了又删掉"等价）；
 *  - 数组剔除空项、对象按键名排序后递归（键顺序不同不影响比较）；
 *  - 全空的对象/数组整体折叠为 null。 */
function normalizeForCompare(value: unknown): unknown {
  if (value === null || value === undefined) return null;
  if (typeof value === "string") {
    const trimmed = value.trim();
    return trimmed === "" ? null : trimmed;
  }
  if (Array.isArray(value)) {
    return value.map(normalizeForCompare).filter((v) => v !== null);
  }
  if (typeof value === "object") {
    const src = value as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(src).sort()) out[key] = normalizeForCompare(src[key]);
    return Object.values(out).every((v) => v === null) ? null : out;
  }
  return value;
}

/** 离开拦截意图：切页（带目标页）或返回工作区 */
type LeaveIntent = { kind: "tab"; tab: PageKey } | { kind: "leave" };

/**
 * 设置页容器组件：全屏浮层（onClose 由父层用于退出浮层）。
 * 内部管理当前页、搜索词、离开拦截三选态；数据面接 settings store 草稿。
 */
export function SettingsPage({
  onClose,
  initialPage = DEFAULT_PAGE,
}: {
  onClose: () => void;
  initialPage?: PageKey;
}) {
  const t = useT();
  const config = useSettingsStore((s) => s.config);
  const baseline = useSettingsStore((s) => s.baseline);
  const loading = useSettingsStore((s) => s.loading);
  const saving = useSettingsStore((s) => s.saving);
  const error = useSettingsStore((s) => s.error);
  const load = useSettingsStore((s) => s.load);
  const saveAll = useSettingsStore((s) => s.saveAll);
  const resetDraft = useSettingsStore((s) => s.resetDraft);
  // 预览归工作区设置容器管理：切页不失效，关闭时按保存基线复位。
  useEffect(() => {
    if (!config) return;
    applyTheme(config.ui.theme);
    applyUiFontSize(config.ui.fontSize);
  }, [config]);
  useEffect(
    () => () => {
      const ui = useSettingsStore.getState().baseline?.ui;
      applyTheme(ui?.theme ?? "system");
      applyUiFontSize(ui?.fontSize);
    },
    [],
  );

  const [tab, setTab] = useState<PageKey>(initialPage);
  const [query, setQuery] = useState("");
  const [leaveIntent, setLeaveIntent] = useState<LeaveIntent | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: 打开时按 config 有无拉一次，不随 load 引用重挂
  useEffect(() => {
    if (!config) {
      void load();
    }
  }, [config]);

  // 逐页脏标记（draft 与基线的字段差集）
  const dirtyMap = useMemo(() => {
    const out = {} as Record<PageKey, boolean>;
    for (const page of PAGE_ORDER) {
      out[page] =
        !!config && !!baseline && !sameSlice(pageSlice(config, page), pageSlice(baseline, page));
    }
    return out;
  }, [config, baseline]);
  const anyDirty = PAGE_ORDER.some((key) => dirtyMap[key]);

  // 搜索命中：按页名过滤（trim 后非空即搜索态，结果替掉导航列表；页名经
  // t() 查当前语言后再匹配——搜索跟随界面语言）
  const searching = query.trim() !== "";
  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q === "") return [] as PageKey[];
    return PAGE_ORDER.filter((key) => t(PAGE_LABEL[key]).toLowerCase().includes(q));
  }, [query, t]);

  /** 切页 / 返回工作区共用：有脏改动先走三选拦截 */
  function requestTab(next: PageKey) {
    if (next === tab) return;
    if (!anyDirty) {
      setTab(next);
      return;
    }
    setLeaveIntent({ kind: "tab", tab: next });
  }

  /** 返回工作区：无脏改动直接关，有则先走三选拦截。 */
  function requestClose() {
    if (!anyDirty) {
      onClose();
      return;
    }
    setLeaveIntent({ kind: "leave" });
  }

  /** 三选处置：保存并离开 → saveAll 成功后执行动作；放弃 → 回基线后执行；留在原地 → 只清意图 */
  async function answerLeave(action: "save" | "discard" | "stay") {
    const intent = leaveIntent;
    if (action === "stay") {
      setLeaveIntent(null);
      return;
    }
    if (action === "save") {
      const ok = await handleSave();
      if (!ok) {
        setLeaveIntent(null); // 校验/落盘失败：留在设置页，错误提示由顶部 Alert 给出
        return;
      }
    } else {
      await resetDraft();
    }
    setLeaveIntent(null);
    if (intent?.kind === "tab") {
      setTab(intent.tab);
      return;
    }
    if (intent) onClose();
  }

  /**
   * 保存（29 号票 Toast 收敛 + 热键应用）：成功/失败经顶部 Toast 反馈；
   * 成功后按新配置应用全局热键（applyHotkey 幂等——enabled=false 注销、
   * 换键先注销再注册、冲突错误进 hotkey store 红字 + Toast）。
   */
  async function handleSave(): Promise<boolean> {
    const ok = await saveAll();
    if (ok) {
      pushToast("success", t("toasts.settingsSaved"));
      const saved = useSettingsStore.getState().config;
      if (saved) void applyHotkey(saved.ui.hotkey);
    } else {
      pushToast(
        "error",
        t("toasts.settingsSaveFailed"),
        useSettingsStore.getState().error ?? undefined,
      );
    }
    return ok;
  }

  // 页内 Esc：返回工作区（脏改动存在时同样先走三选）。浮层（select 下拉/弹框）
  // 展开时让位；快捷键录制态让位（录制组件自己消费 Esc = 取消录制）。
  const closeRef = useRef(requestClose);
  closeRef.current = requestClose;
  useEffect(() => {
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing) return;
      const overlayOpen =
        leaveIntent !== null ||
        document.activeElement?.tagName === "SELECT" ||
        !!document.querySelector(".settings-dialog-mask") ||
        !!document.querySelector("[data-hotkey-recording='true']");
      if (overlayOpen) return; // 弹框自身处理 Esc；原生下拉先归浏览器
      if (query.trim() !== "") {
        e.preventDefault();
        setQuery("");
        return;
      }
      e.preventDefault();
      closeRef.current();
    };
    window.addEventListener("keydown", onKeydown, true);
    return () => window.removeEventListener("keydown", onKeydown, true);
  }, [leaveIntent, query]);

  // 可达性：打开时把焦点交给「返回工作区」
  const backRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    backRef.current?.focus();
  }, []);

  // 全部设置页的装配表：key 对应注册表页键，body 为页面 JSX（数据未加载的页暂空）
  const pages: { key: PageKey; body: ReactNode }[] = [
    { key: "appearance", body: <AppearancePage /> },
    { key: "providers", body: config ? <ProvidersPage /> : null },
    { key: "network", body: config ? <NetworkPage /> : null },
    { key: "security", body: config ? <SecurityPage /> : null },
    { key: "mcp", body: <McpPage /> },
    { key: "skills", body: <SkillsPage /> },
    { key: "tools", body: <PlaceholderPage page="tools" /> },
    { key: "agent", body: config ? <AgentPage /> : null },
    { key: "logs", body: config ? <LogsPage /> : null },
    { key: "about", body: <AboutPage /> },
  ];
  const activePage = pages.find((p) => p.key === tab) ?? pages[0];

  /** 导航列表方向键移动焦点（上下/左右都算，首尾不循环）。 */
  function onNavArrow(e: ReactKeyboardEvent<HTMLDivElement>) {
    const delta =
      e.key === "ArrowDown" || e.key === "ArrowRight"
        ? 1
        : e.key === "ArrowUp" || e.key === "ArrowLeft"
          ? -1
          : 0;
    if (delta === 0) return;
    const items = Array.from(
      e.currentTarget.querySelectorAll<HTMLButtonElement>(".settings-nav-item"),
    );
    const idx = items.indexOf(document.activeElement as HTMLButtonElement);
    if (idx < 0) return;
    const next = items[idx + delta];
    if (!next) return; // 首尾不循环
    e.preventDefault();
    next.focus();
  }

  return (
    <div
      className="settings-shell"
      data-testid="settings-page"
      role="dialog"
      aria-modal="true"
      aria-label={t("settings.title")}
    >
      <nav className="settings-nav" aria-label={t("settings.navAria")}>
        <div className="settings-nav-head">
          <Button
            ref={backRef}
            variant="ghost"
            className="settings-nav-back w-full justify-start"
            onClick={requestClose}
          >
            <ArrowLeftIcon className="h-4 w-4" />
            {t("settings.navBack")}
          </Button>
          <div className="settings-search relative">
            <SearchIcon className="pointer-events-none absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2 text-[var(--muted-foreground)]" />
            <Input
              className="pl-9"
              aria-label={t("settings.searchPlaceholder")}
              placeholder={t("settings.searchPlaceholder")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
        </div>

        {searching ? (
          <div
            className="settings-nav-list"
            role="listbox"
            aria-label={t("settings.searchResultsAria")}
          >
            {results.length === 0 ? (
              <div className="px-2 py-3 text-ui-sm text-[var(--cw-dim)]">
                <div className="mb-1 text-ui-sm text-[var(--cw-text-2)]">
                  {t("settings.searchNoMatch")}
                </div>
                {t("settings.searchTryOther")}
              </div>
            ) : (
              results.map((key) => (
                <button
                  key={key}
                  type="button"
                  role="option"
                  aria-selected={key === tab}
                  className="settings-nav-item"
                  onClick={() => requestTab(key)}
                >
                  <span className="settings-nav-label">
                    <span className="truncate">{t(PAGE_LABEL[key])}</span>
                    <span className="ml-auto text-ui-sm text-[var(--cw-dim)]">
                      {t(PAGE_LABEL[key])}
                    </span>
                  </span>
                </button>
              ))
            )}
          </div>
        ) : (
          <div
            className="settings-nav-list"
            role="tablist"
            aria-orientation="vertical"
            onKeyDown={onNavArrow}
          >
            {PAGE_GROUPS.map((group) => (
              <Fragment key={group.title}>
                <div className="settings-nav-group" role="presentation">
                  {t(group.title)}
                </div>
                {group.pages.map((key) => {
                  const Icon = PAGE_ICON[key];
                  return (
                    <button
                      key={key}
                      type="button"
                      role="tab"
                      id={`settings-tab-${key}`}
                      aria-selected={key === tab}
                      aria-controls="settings-panel"
                      data-page={key}
                      className={cn("settings-nav-item", key === tab && "settings-nav-item-active")}
                      onClick={() => requestTab(key)}
                    >
                      <span className="settings-nav-label">
                        <Icon className="settings-nav-icon" />
                        {t(PAGE_LABEL[key])}
                        {dirtyMap[key] && (
                          <span className="settings-dirty-dot" title={t("settings.dirtyDot")} />
                        )}
                      </span>
                    </button>
                  );
                })}
              </Fragment>
            ))}
          </div>
        )}
      </nav>

      <div className="settings-content">
        {error && (
          <Alert
            variant="error"
            title={t("settings.errorPrefix", { message: error })}
            className="m-4"
          />
        )}
        {loading && !config && (
          <Alert variant="info" title={t("settings.loading")} className="m-4" />
        )}
        <div className="settings-actions">
          <span className="settings-actions-title">{t(PAGE_LABEL[tab])}</span>
          {anyDirty && <span className="settings-dirty-dot" title={t("settings.dirtyDot")} />}
          <div className="settings-actions-buttons">
            <Button
              variant="outline"
              disabled={!config}
              onClick={() => {
                void resetDraft().then(() => onClose());
              }}
            >
              {t("settings.cancel")}
            </Button>
            <Button disabled={!config || saving} onClick={() => void handleSave()}>
              {saving ? t("settings.saving") : t("settings.save")}
            </Button>
          </div>
        </div>
        <div className="settings-pane">
          <div
            className="settings-pane-body"
            id="settings-panel"
            role="tabpanel"
            aria-labelledby={`settings-tab-${tab}`}
          >
            <div className="settings-pane-inner">{activePage.body}</div>
          </div>
        </div>
      </div>

      <Dialog
        open={leaveIntent !== null}
        title={t("settings.leaveTitle")}
        onClose={() => void answerLeave("stay")}
        actions={[
          {
            label: t("settings.leaveStay"),
            variant: "ghost",
            onClick: () => void answerLeave("stay"),
          },
          {
            label: t("settings.leaveDiscard"),
            variant: "danger",
            onClick: () => void answerLeave("discard"),
          },
          { label: t("settings.leaveSave"), onClick: () => void answerLeave("save") },
        ]}
      >
        {t("settings.leaveBody")}
      </Dialog>
    </div>
  );
}

/** 表单行（照抄 CodeWave SettingsFormItem：左说明右控件，37.5% 控件列） */
function SettingsRow({
  title,
  description,
  children,
  className,
}: {
  title: string;
  description?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("settings-row", className)}>
      <div className="settings-row-copy">
        <span className="settings-row-title">{title}</span>
        {description && <span className="settings-row-description">{description}</span>}
      </div>
      <div className="settings-row-control">{children}</div>
    </div>
  );
}

/**
 * 「打开目录」按钮（03 号票，FR-14-07）：统一走 open_dir 命令（后端白名单
 * 校验 + 系统打开），失败在按钮旁行内提示（错误码 + 消息），不打断页面。
 */
function OpenDirButton({
  testId,
  label,
  path,
  disabled = false,
  disabledReason,
}: {
  testId: string;
  label: string;
  /** 要打开的目录绝对路径（disabled 时可为空串占位）。 */
  path: string;
  disabled?: boolean;
  /** 禁用原因（无项目会话等），显示在按钮下方替代错误位。 */
  disabledReason?: string;
}) {
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="flex min-w-0 flex-col items-end gap-1">
      <Button
        variant="outline"
        size="sm"
        data-testid={testId}
        disabled={disabled}
        onClick={() => {
          setError(null);
          openDir(path).catch((err: unknown) => {
            const code =
              typeof err === "object" && err !== null && "code" in err
                ? String((err as { code: unknown }).code)
                : "E_INTERNAL";
            const message =
              typeof err === "object" && err !== null && "message" in err
                ? String((err as { message: unknown }).message)
                : String(err);
            setError(`${code}: ${message}`);
          });
        }}
      >
        {label}
      </Button>
      {error && (
        <span className="max-w-full text-right text-ui-sm break-all text-[var(--danger)]">
          {error}
        </span>
      )}
      {!error && disabled && disabledReason && (
        <span className="max-w-full text-right text-ui-sm text-[var(--muted-foreground)]">
          {disabledReason}
        </span>
      )}
    </div>
  );
}

/** 设置分区容器：小节标题 + 卡片底板（页内每块设置的通用外框）。 */
function SettingsSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settings-section">
      <h3 className="settings-section-title">{title}</h3>
      <div className="settings-section-card">{children}</div>
    </section>
  );
}

/** 三选一卡片（主题 / 代理模式共用视觉） */
function ChoiceCard({
  active,
  title,
  description,
  onClick,
}: {
  active: boolean;
  title: string;
  description: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={cn("settings-choice-card", active && "active")}
      aria-pressed={active}
      onClick={onClick}
    >
      <span className="settings-choice-card-title">
        {title}
        <span className="settings-choice-dot" aria-hidden />
      </span>
      <p className="settings-choice-card-desc">{description}</p>
    </button>
  );
}

/**
 * 通知授权态查询（19 号票）：挂载时查一次；窗口重新聚焦（典型场景 =
 * 用户从系统设置授权后切回）时重查，授权变更即时反映。
 * 查询失败（如非 Tauri 环境）→ null = 不显示引导，宁可少打扰。
 */
function useNotificationPermission(): NotificationPermissionStatus | null {
  const [status, setStatus] = useState<NotificationPermissionStatus | null>(null);
  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      void (async () => {
        try {
          const next = await notificationPermissionStatus();
          if (!cancelled) setStatus(next);
        } catch (err) {
          console.warn("[settings] 通知授权态查询失败（不显示引导）", err);
          if (!cancelled) setStatus(null);
        }
      })();
    };
    refresh();
    window.addEventListener("focus", refresh);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", refresh);
    };
  }, []);
  return status;
}

/** 通知授权引导条（19 号票，macOS 为主）：未授权/被拒时提示 + 打开系统
 *  设置深链；授权后（或 unknown 查不出）不渲染，不打扰。
 *  status 为生成物契约里的自由字符串（Rust 发射面取值 granted/denied/
 *  prompt/unknown 四字面量，见 system.rs 文档），组件内部按字面量比较。 */
function NotificationPermissionNotice({
  status,
  hasSettingsEntry,
}: {
  status: string;
  hasSettingsEntry: boolean;
}) {
  const t = useT();
  return (
    <Alert
      variant="warning"
      className="m-3"
      title={
        <span className="flex flex-wrap items-center gap-2">
          {status === "denied" ? t("settings.notifyDenied") : t("settings.notifyNotDetermined")}
          {hasSettingsEntry && (
            <button
              type="button"
              data-testid="open-notification-settings"
              className="shrink-0 rounded border border-[var(--warning)]/50 px-2 py-0.5 text-ui-sm hover:bg-[var(--warning)]/10"
              onClick={() => {
                void openNotificationSettings().catch((err) =>
                  console.error("[settings] 打开系统设置失败", err),
                );
              }}
            >
              {t("settings.openSystemSettings")}
            </button>
          )}
        </span>
      }
    />
  );
}

/** 界面页：主题 + 界面语言 + AI 回复语言（ui 段整卡）+ 系统通知偏好
 *  （含 19 号票的 macOS 授权引导：未授权/被拒时提示并给系统设置入口）
 *  + 窗口行为（29 号票：关闭按钮行为 + 全局快捷键）。 */
function AppearancePage() {
  const t = useT();
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  const permission = useNotificationPermission();
  if (!config) return null;
  const patchUi = (patch: Partial<AppConfig["ui"]>) =>
    patchConfig({ ui: { ...config.ui, ...patch } });
  const notifications = config.notifications ?? { enabled: true, interventionOnly: false };
  const patchNotifications = (patch: Partial<AppConfig["notifications"]>) =>
    patchConfig({ notifications: { ...notifications, ...patch } });
  // 界面语言即时生效（31 号票）：选择即切 i18n 内存态（全界面无刷新换语言）
  // 并整份保存落盘（语言是全局即时项，与其他草稿字段的「显式保存」不同——
  // 先例：白名单追加/供应商表单的即时持久化通道）。
  const changeLanguage = (language: AppConfig["ui"]["language"]) => {
    setLang(language);
    patchUi({ language });
    void useSettingsStore.getState().saveAll();
  };

  return (
    <div>
      <SettingsSection title={t("settings.sectionTheme")}>
        <div className="py-4">
          <div className="settings-choice-list">
            <ChoiceCard
              active={(config.ui.theme ?? "system") === "light"}
              title={t("settings.themeLight")}
              description={t("settings.themeLightDesc")}
              onClick={() => patchUi({ theme: "light" })}
            />
            <ChoiceCard
              active={(config.ui.theme ?? "system") === "dark"}
              title={t("settings.themeDark")}
              description={t("settings.themeDarkDesc")}
              onClick={() => patchUi({ theme: "dark" })}
            />
            <ChoiceCard
              active={(config.ui.theme ?? "system") === "system"}
              title={t("settings.themeSystem")}
              description={t("settings.themeSystemDesc")}
              onClick={() => patchUi({ theme: "system" })}
            />
          </div>
        </div>
      </SettingsSection>
      <SettingsSection title={t("settings.sectionFontSize")}>
        <SettingsRow title={t("settings.fontSizeTitle")} description={t("settings.fontSizeDesc")}>
          <label className="flex w-full items-center gap-3 text-ui-base">
            <input
              aria-label={t("settings.fontSizeTitle")}
              type="range"
              min={12}
              max={18}
              step={1}
              value={config.ui.fontSize ?? 14}
              onChange={(e) => patchUi({ fontSize: Number(e.target.value) })}
              className="min-w-0 flex-1 accent-[var(--cw-focus)]"
            />
            <span className="shrink-0 tabular-nums">{config.ui.fontSize ?? 14}px</span>
          </label>
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title={t("settings.sectionNotifications")}>
        {permission !== null &&
          permission.status !== "granted" &&
          permission.status !== "unknown" && (
            <NotificationPermissionNotice
              status={permission.status}
              hasSettingsEntry={permission.hasSettingsEntry}
            />
          )}
        <SettingsRow
          title={t("settings.notifyEnable")}
          description={t("settings.notifyEnableDesc")}
        >
          <Switch
            checked={notifications.enabled}
            onCheckedChange={(v) => patchNotifications({ enabled: v })}
          />
        </SettingsRow>
        <SettingsRow
          title={t("settings.notifyInterventionOnly")}
          description={t("settings.notifyInterventionOnlyDesc")}
        >
          <Switch
            checked={notifications.interventionOnly}
            onCheckedChange={(v) => patchNotifications({ interventionOnly: v })}
          />
        </SettingsRow>
      </SettingsSection>
      <AppearanceWindowBehavior config={config} patchUi={patchUi} />
      <SettingsSection title={t("settings.sectionLanguage")}>
        <SettingsRow title={t("settings.uiLanguage")} description={t("settings.uiLanguageDesc")}>
          <Select
            width="mid"
            value={config.ui.language}
            onChange={(e) => changeLanguage(e.target.value as AppConfig["ui"]["language"])}
          >
            {/* 语言名用各自语言的本地名（i18n 惯例：语言选择器不翻译自身） */}
            <option value="zh-CN">简体中文</option>
            <option value="en-US">English</option>
          </Select>
        </SettingsRow>
        <SettingsRow title={t("settings.aiLanguage")} description={t("settings.aiLanguageDesc")}>
          <Input
            className="w-[240px] max-w-full"
            maxLength={40}
            placeholder={t("settings.aiLanguagePlaceholder")}
            value={config.ui.aiLanguage ?? ""}
            onChange={(e) =>
              patchUi({ aiLanguage: e.target.value.trim() === "" ? undefined : e.target.value })
            }
          />
        </SettingsRow>
      </SettingsSection>
    </div>
  );
}

/**
 * 窗口行为区（29 号票）：关闭按钮行为 + 全局快捷键。
 * - closeToTray（§9.6）：开（缺省）= 点关闭按钮收进托盘继续运行；关 =
 *   点关闭按钮走退出拦截协议（活跃任务先弹确认）。保存后即时生效
 *   （Rust 侧每次关闭时读配置）；
 * - hotkey（§10.7）：启用开关 + 快捷键录制（按下组合键捕获显示、Esc 取
 *   消）；注册失败（被其他应用占用）红字提示——注册发生在保存后
 *   （applyHotkey），红字数据面在 hotkey store。
 */
function AppearanceWindowBehavior({
  config,
  patchUi,
}: {
  config: AppConfig;
  patchUi: (patch: Partial<AppConfig["ui"]>) => void;
}) {
  const hotkeyError = useHotkeyStore((s) => s.error);
  const hotkey = config.ui.hotkey ?? { enabled: true, accelerator: "alt+space" };
  const t = useT();
  return (
    <SettingsSection title={t("settings.sectionWindowBehavior")}>
      <SettingsRow title={t("settings.closeToTray")} description={t("settings.closeToTrayDesc")}>
        <Switch
          aria-label={t("settings.closeToTray")}
          data-testid="close-to-tray-switch"
          checked={config.ui.closeToTray ?? true}
          onCheckedChange={(v) => patchUi({ closeToTray: v })}
        />
      </SettingsRow>
      <SettingsRow title={t("settings.hotkeyEnable")} description={t("settings.hotkeyEnableDesc")}>
        <Switch
          aria-label={t("settings.hotkeyEnable")}
          data-testid="hotkey-enabled-switch"
          checked={hotkey.enabled}
          onCheckedChange={(v) => patchUi({ hotkey: { ...hotkey, enabled: v } })}
        />
      </SettingsRow>
      {hotkey.enabled && (
        <SettingsRow
          title={t("settings.hotkeyRecorderTitle")}
          description={t("settings.hotkeyRecorderDesc")}
        >
          <div className="flex min-w-0 flex-col items-end gap-1">
            <HotkeyRecorder
              value={hotkey.accelerator}
              onChange={(accelerator) => patchUi({ hotkey: { ...hotkey, accelerator } })}
            />
            {hotkeyError && (
              <span
                data-testid="hotkey-error"
                className="max-w-full text-right text-ui-sm text-[var(--danger)]"
              >
                {t("settings.hotkeyOccupied", {
                  accelerator: formatAccelerator(hotkey.accelerator),
                  message: hotkeyError,
                })}
              </span>
            )}
          </div>
        </SettingsRow>
      )}
    </SettingsSection>
  );
}

/**
 * 快捷键录制钮（29 号票）：常态显示当前组合键；点击进入录制态（按钮打
 * data-hotkey-recording 标记，设置页 Esc handler 据此让位），按下有效
 * 组合键即归一化写回草稿并退出录制，Esc 取消恢复原值。
 */
function HotkeyRecorder({
  value,
  onChange,
}: {
  value: string;
  onChange: (accelerator: string) => void;
}) {
  const t = useT();
  const recordingLabel = t("settings.hotkeyRecording");
  const recordingLabelAria = t("settings.hotkeyRecordAria");
  const [recording, setRecording] = useState(false);
  useEffect(() => {
    if (!recording) return;
    const onKeydown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setRecording(false);
        return;
      }
      const normalized = eventToAccelerator(event);
      if (normalized !== null) {
        onChange(normalized);
        setRecording(false);
      }
      // 纯修饰键按下：继续等待主键（录制态不退出）
    };
    window.addEventListener("keydown", onKeydown, true);
    return () => window.removeEventListener("keydown", onKeydown, true);
  }, [recording, onChange]);
  return (
    <Button
      variant="outline"
      size="sm"
      data-testid="hotkey-recorder"
      data-hotkey-recording={recording ? "true" : "false"}
      aria-label={recordingLabelAria}
      className={cn("min-w-36 font-mono", recording && "border-[var(--cw-focus)]")}
      onClick={() => setRecording(true)}
    >
      {recording ? recordingLabel : formatAccelerator(value)}
    </Button>
  );
}

/**
 * 代理地址即时校验（19 号票 FR-02-10）：scheme 白名单与后端
 * providers::http::validate_proxy_url 同口径（http/https/socks5/socks5h，
 * socks5h = 域名由代理远端解析）；返回错误文案或 undefined。
 * 草稿层允许非法值写入（输入不被打断），错误就地展示、保存时后端归一化
 * 兜底拒绝。
 */
function proxyUrlIssue(url: string): string | undefined {
  const trimmed = url.trim();
  const schemeHint = "http:// / https:// / socks5:// / socks5h://";
  const sep = trimmed.indexOf("://");
  if (sep < 0) return t("settings.proxyMissingScheme", { hint: schemeHint });
  const scheme = trimmed.slice(0, sep).toLowerCase();
  if (!["http", "https", "socks5", "socks5h"].includes(scheme)) {
    return t("settings.proxyUnsupportedScheme", { scheme, hint: schemeHint });
  }
  if (trimmed.slice(sep + 3).trim() === "") {
    return t("settings.proxyMissingHost");
  }
  return undefined;
}

/**
 * 网络页：代理模式三选 + 自定义地址（M1 保存到 config.proxy）+ 网络搜索
 * （13 号票，C-12 本地优先三通道的配置面）：
 * - 代理三模式热生效语义（19 号票 FR-02-10）：none = 显式直连（不读环境
 *   变量代理）；system = 环境变量 + 系统代理设置（内建探测）；manual =
 *   指定地址（scheme 即时校验、错误就地展示）。保存后下一次对话生效
 *   （进行中的对话不被切换——客户端按对话构建）；
 * - SearXNG 独立配置组：启用开关 / 服务地址（默认 http://127.0.0.1:8888）/
 *   连通测试按钮（草稿试连，不经保存——对齐 MCP 管理页的测试连接模式；
 *   本地回环请求不经应用代理，后端已内置该例外）；
 * - Tavily 高级区：API Key 输入（BYOK，明文随 config.json 0600 落盘）+
 *   注册指引文案（tavily.com 邮箱注册、免绑卡、免费 1000 credits/月）+
 *   连通测试（测试会消耗 1 credit，指引文案注明）。
 */
function NetworkPage() {
  const t = useT();
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  if (!config) return null;
  // 缺省高亮对齐后端语义：proxy 字段未配置 = no_proxy 直连（19 号票口径，
  // 防环境变量泄漏）；M1 时代 UI 缺省显示"系统代理"与后端行为不一致，本票对齐。
  const mode = config.proxy?.mode ?? "none";
  const manualUrl = config.proxy?.url ?? "";
  const manualIssue = mode === "manual" ? proxyUrlIssue(manualUrl) : undefined;
  // search 组草稿（生成物 SearchConfig：searxng 必填、tavilyApiKey 可空；
  // 后端 serde default 保证旧配置载入后组结构完整）
  const search = config.search;
  const patchSearch = (patch: Partial<NonNullable<AppConfig["search"]>>) =>
    patchConfig({
      search: { ...search, ...patch },
    });
  const patchSearxng = (patch: Partial<AppConfig["search"]["searxng"]>) =>
    patchSearch({ searxng: { ...search.searxng, ...patch } });

  return (
    <div>
      <SettingsSection title={t("settings.sectionProxy")}>
        <div className="py-4">
          <div className="settings-choice-list">
            <ChoiceCard
              active={mode === "none"}
              title={t("settings.proxyNone")}
              description={t("settings.proxyNoneDesc")}
              onClick={() => patchConfig({ proxy: { mode: "none" } })}
            />
            <ChoiceCard
              active={mode === "system"}
              title={t("settings.proxySystem")}
              description={t("settings.proxySystemDesc")}
              onClick={() => patchConfig({ proxy: { mode: "system" } })}
            />
            <ChoiceCard
              active={mode === "manual"}
              title={t("settings.proxyManual")}
              description={t("settings.proxyManualDesc")}
              onClick={() => patchConfig({ proxy: { mode: "manual", url: manualUrl } })}
            />
          </div>
          <p className="mt-2 text-ui-sm text-[var(--muted-foreground)]">
            {t("settings.proxyApplyHint")}
          </p>
        </div>
        {mode === "manual" && (
          <div className="border-t border-[var(--border)] py-4">
            <SettingsRow title={t("settings.proxyUrl")} description={t("settings.proxyUrlDesc")}>
              <div className="flex w-full flex-col items-end gap-1">
                <Input
                  className={cn(
                    "w-[360px] max-w-full font-mono text-ui-sm",
                    manualIssue && "settings-field-error-input",
                  )}
                  placeholder={t("workspace.proxyUrlPlaceholder")}
                  value={manualUrl}
                  onChange={(e) => patchConfig({ proxy: { mode: "manual", url: e.target.value } })}
                />
                {manualIssue ? <span className="settings-field-error">{manualIssue}</span> : null}
              </div>
            </SettingsRow>
          </div>
        )}
      </SettingsSection>
      <NetworkSearchPage search={search} patchSearch={patchSearch} patchSearxng={patchSearxng} />
    </div>
  );
}

/**
 * 网络搜索区块（13 号票，网络页的下半部分独立成组件）：连通测试状态
 * （SearXNG / Tavily 各一份）在组件内 useState 管理——测试是探测动作
 * 不落盘，不入 settings 草稿（对齐 MCP 管理页测试连接的草稿态定案）。
 */
function NetworkSearchPage({
  search,
  patchSearch,
  patchSearxng,
}: {
  search: NonNullable<AppConfig["search"]>;
  patchSearch: (patch: Partial<NonNullable<AppConfig["search"]>>) => void;
  patchSearxng: (patch: Partial<AppConfig["search"]["searxng"]>) => void;
}) {
  const t = useT();
  // 连通测试态：{ testing: 进行中, result: 结果 | null }；两通道独立
  const [searxngTest, setSearxngTest] = useState<{
    testing: boolean;
    result: SearchTestResult | null;
  }>({ testing: false, result: null });
  const [tavilyTest, setTavilyTest] = useState<{
    testing: boolean;
    result: SearchTestResult | null;
  }>({ testing: false, result: null });

  /** SearXNG 连通测试：当前草稿地址 → search_test（不经保存直接探测）。 */
  async function runSearxngTest() {
    setSearxngTest({ testing: true, result: null });
    try {
      const result = await searchTest({
        target: "searxng",
        searxngBaseUrl: search.searxng.baseUrl,
      });
      setSearxngTest({ testing: false, result });
    } catch (err) {
      setSearxngTest({
        testing: false,
        result: { ok: false, error: err instanceof Error ? err.message : String(err) },
      });
    }
  }

  /** Tavily 连通测试：当前草稿 Key → search_test（消耗 1 credit）。 */
  async function runTavilyTest() {
    setTavilyTest({ testing: true, result: null });
    try {
      const result = await searchTest({
        target: "tavily",
        tavilyApiKey: search.tavilyApiKey ?? "",
      });
      setTavilyTest({ testing: false, result });
    } catch (err) {
      setTavilyTest({
        testing: false,
        result: { ok: false, error: err instanceof Error ? err.message : String(err) },
      });
    }
  }

  return (
    <div>
      <SettingsSection title={t("settings.sectionSearxng")}>
        <SettingsRow
          title={t("settings.searxngEnable")}
          description={t("settings.searxngEnableDesc")}
        >
          <Switch
            aria-label={t("settings.searxngEnable")}
            data-testid="searxng-enabled-switch"
            checked={search.searxng.enabled}
            onCheckedChange={(v) => patchSearxng({ enabled: v })}
          />
        </SettingsRow>
        <SettingsRow
          title={t("settings.searxngBaseUrl")}
          description={t("settings.searxngBaseUrlDesc")}
        >
          <Input
            aria-label={t("settings.searxngBaseUrlAria")}
            data-testid="searxng-base-url-input"
            className="w-[360px] max-w-full font-mono text-ui-sm"
            placeholder="http://127.0.0.1:8888"
            value={search.searxng.baseUrl}
            onChange={(e) => patchSearxng({ baseUrl: e.target.value })}
          />
        </SettingsRow>
        <SettingsRow
          title={t("settings.connectivityTest")}
          description={t("settings.connectivityTestDesc")}
        >
          <div className="flex min-w-0 flex-col items-end gap-1">
            <Button
              variant="outline"
              size="sm"
              data-testid="searxng-test-button"
              disabled={searxngTest.testing || search.searxng.baseUrl.trim() === ""}
              onClick={() => void runSearxngTest()}
            >
              {searxngTest.testing ? t("settings.testing") : t("settings.testConnection")}
            </Button>
            {searxngTest.result && (
              <span
                data-testid="searxng-test-result"
                className={`max-w-full text-right text-ui-sm break-all ${
                  searxngTest.result.ok ? "text-[var(--success)]" : "text-[var(--danger)]"
                }`}
              >
                {searxngTest.result.ok
                  ? t("settings.testOk", { latency: searxngTest.result.latencyMs ?? "?" })
                  : t("settings.testFailed", {
                      error: searxngTest.result.error ?? t("settings.unknownError"),
                    })}
              </span>
            )}
          </div>
        </SettingsRow>
        {search.searxng.enabled && (
          <p
            className="border-t border-[var(--border)] px-4 py-3 text-ui-sm text-[var(--muted-foreground)]"
            data-testid="searxng-format-hint"
          >
            {t("settings.searxngFormatHintBefore")}
            <code className="mx-1 rounded bg-[var(--muted)] px-1 font-mono">json</code>
            {t("settings.searxngFormatHintAfter")}
          </p>
        )}
      </SettingsSection>
      <SettingsSection title={t("settings.sectionTavily")}>
        <SettingsRow title={t("settings.tavilyKey")} description={t("settings.tavilyKeyDesc")}>
          <Input
            aria-label={t("settings.tavilyKeyAria")}
            data-testid="tavily-api-key-input"
            type="password"
            className="w-[360px] max-w-full font-mono text-ui-sm"
            placeholder="tvly-…"
            value={search.tavilyApiKey ?? ""}
            onChange={(e) =>
              patchSearch({ tavilyApiKey: e.target.value === "" ? undefined : e.target.value })
            }
          />
        </SettingsRow>
        <SettingsRow
          title={t("settings.connectivityTest")}
          description={t("settings.tavilyTestDesc")}
        >
          <div className="flex min-w-0 flex-col items-end gap-1">
            <Button
              variant="outline"
              size="sm"
              data-testid="tavily-test-button"
              disabled={tavilyTest.testing || (search.tavilyApiKey ?? "").trim() === ""}
              onClick={() => void runTavilyTest()}
            >
              {tavilyTest.testing ? t("settings.testing") : t("settings.testConnection")}
            </Button>
            {tavilyTest.result && (
              <span
                data-testid="tavily-test-result"
                className={`max-w-full text-right text-ui-sm break-all ${
                  tavilyTest.result.ok ? "text-[var(--success)]" : "text-[var(--danger)]"
                }`}
              >
                {tavilyTest.result.ok
                  ? t("settings.testOk", { latency: tavilyTest.result.latencyMs ?? "?" })
                  : t("settings.testFailed", {
                      error: tavilyTest.result.error ?? t("settings.unknownError"),
                    })}
              </span>
            )}
          </div>
        </SettingsRow>
      </SettingsSection>
    </div>
  );
}

/** 安全与审批页：审批四开关（FR-14-04，23 号票字段级）+ 命令白名单管理。
 *  四开关：① 审批总开关 ② 自动放行等待 ③ 会话内记忆（「始终允许（本会话）」
 *  选项展示）④ 命令白名单启用。白名单条目 = 命令前缀（词序列前缀匹配），
 *  手填增删。 */
function SecurityPage() {
  const t = useT();
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  if (!config) return null;
  const approval = config.approval;
  const patchApproval = (patch: Partial<AppConfig["approval"]>) =>
    patchConfig({ approval: { ...approval, ...patch } });
  const allowlist = approval.commandAllowlist ?? [];

  return (
    <div>
      <SettingsSection title={t("settings.sectionApproval")}>
        <SettingsRow
          title={t("settings.approvalEnable")}
          description={t("settings.approvalEnableDesc")}
        >
          <Switch
            aria-label={t("settings.approvalEnable")}
            checked={approval.enabled}
            onCheckedChange={(v) => patchApproval({ enabled: v })}
          />
        </SettingsRow>
        <SettingsRow title={t("settings.autoConfirm")} description={t("settings.autoConfirmDesc")}>
          <Switch
            aria-label={t("settings.autoConfirm")}
            checked={approval.autoConfirm}
            onCheckedChange={(v) => patchApproval({ autoConfirm: v })}
          />
        </SettingsRow>
        <SettingsRow
          title={t("settings.sessionMemory")}
          description={t("settings.sessionMemoryDesc")}
        >
          <Switch
            aria-label={t("settings.sessionMemory")}
            checked={approval.sessionMemoryEnabled}
            onCheckedChange={(v) => patchApproval({ sessionMemoryEnabled: v })}
          />
        </SettingsRow>
        <SettingsRow
          title={t("settings.allowlistToggle")}
          description={t("settings.allowlistToggleDesc")}
        >
          <Switch
            aria-label={t("settings.allowlistToggle")}
            checked={approval.allowlistEnabled}
            onCheckedChange={(v) => patchApproval({ allowlistEnabled: v })}
          />
        </SettingsRow>
        <SettingsRow title={t("settings.requirePlan")} description={t("settings.requirePlanDesc")}>
          <Switch
            aria-label={t("settings.requirePlan")}
            data-testid="require-plan-switch"
            checked={approval.requirePlan}
            onCheckedChange={(v) => patchApproval({ requirePlan: v })}
          />
        </SettingsRow>
        <SettingsRow
          title={t("settings.gitAllowStageCommit")}
          description={t("settings.gitAllowStageCommitDesc")}
        >
          <Switch
            aria-label={t("settings.gitAllowStageCommit")}
            data-testid="git-allow-stage-commit-switch"
            checked={approval.gitAllowStageCommit}
            onCheckedChange={(v) => patchApproval({ gitAllowStageCommit: v })}
          />
        </SettingsRow>
      </SettingsSection>
      <AllowlistSection
        allowlist={allowlist}
        onChange={(next) => patchApproval({ commandAllowlist: next })}
      />
    </div>
  );
}

/** 白名单条目的词边界校验（23 号票：与后端 allowlist_hit 的切词口径一致）：
 *  返回 null = 合法；返回字符串 = 人话拒绝原因（经 t() 查词，31 号票 i18n）。 */
function validateAllowlistEntry(entry: string): string | null {
  const trimmed = entry.trim();
  if (trimmed === "") return t("settings.allowlistEmpty");
  if (/[|;&><()`$]/.test(trimmed)) {
    return t("settings.allowlistShellChars");
  }
  return null;
}

/** 命令白名单管理区：手填命令前缀增删（词边界校验提示）+ 条目列表删除。 */
function AllowlistSection({
  allowlist,
  onChange,
}: {
  allowlist: string[];
  onChange: (next: string[]) => void;
}) {
  const [draft, setDraft] = useState("");
  const [hint, setHint] = useState<string | null>(null);
  const t = useT();
  const add = () => {
    const error = validateAllowlistEntry(draft);
    if (error !== null) {
      setHint(error);
      return;
    }
    const entry = draft.trim();
    if (allowlist.includes(entry)) {
      setHint(t("settings.allowlistDuplicate"));
      return;
    }
    onChange([...allowlist, entry]);
    setDraft("");
    setHint(null);
  };
  return (
    <SettingsSection title={t("settings.sectionAllowlist")}>
      <div className="flex items-start gap-2 pb-2">
        <input
          type="text"
          data-testid="allowlist-input"
          value={draft}
          onChange={(e) => {
            setDraft(e.target.value);
            setHint(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
          placeholder={t("settings.allowlistPlaceholder")}
          aria-label={t("settings.allowlistAria")}
          className="min-w-0 flex-1 rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 font-mono text-ui-sm text-[var(--foreground)]"
        />
        <Button variant="outline" size="sm" data-testid="allowlist-add" onClick={add}>
          {t("settings.add")}
        </Button>
      </div>
      {hint !== null && (
        <p data-testid="allowlist-hint" className="pb-2 text-ui-sm text-[var(--warning)]">
          {hint}
        </p>
      )}
      {allowlist.length === 0 ? (
        <p className="py-10 text-center text-ui-base text-[var(--muted-foreground)]">
          {t("settings.allowlistEmptyHint")}
        </p>
      ) : (
        <div className="py-3">
          {/* key 用条目本身：添加入口（含勾选追加）已拒绝重复条目，内容即唯一键 */}
          {allowlist.map((entry, index) => (
            <div
              key={entry}
              className="flex items-start justify-between gap-4 border-t border-[var(--border)] py-3 first:border-t-0 first:pt-0"
            >
              <code className="min-w-0 flex-1 font-mono text-ui-sm break-all text-[var(--foreground)]">
                {entry}
              </code>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => onChange(allowlist.filter((_, i) => i !== index))}
              >
                {t("workspace.delete")}
              </Button>
            </div>
          ))}
        </div>
      )}
    </SettingsSection>
  );
}

/**
 * 「打开日志目录」按钮（32 号票改走 open_logs_dir，§3.3 #56）：固定打开
 * `~/.craftwave/logs/`（后端创建 + 白名单口径与 open_dir 同源），失败在
 * 按钮旁行内提示，不打断页面。
 */
function OpenLogsDirButton() {
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="flex min-w-0 flex-col items-end gap-1">
      <Button
        variant="outline"
        size="sm"
        data-testid="open-logs-dir"
        onClick={() => {
          setError(null);
          openLogsDir().catch((err: unknown) => {
            const code =
              typeof err === "object" && err !== null && "code" in err
                ? String((err as { code: unknown }).code)
                : "E_INTERNAL";
            const message =
              typeof err === "object" && err !== null && "message" in err
                ? String((err as { message: unknown }).message)
                : String(err);
            setError(`${code}: ${message}`);
          });
        }}
      >
        {t("settings.openLogsDir")}
      </Button>
      {error && (
        <span className="max-w-full text-right text-ui-sm break-all text-[var(--danger)]">
          {error}
        </span>
      )}
    </div>
  );
}

/**
 * 会话日志条目行（32 号票，kind/level 着色 + 长行折行）：行 JSON 解析成功
 * 时按 level 着色（error 红 / warn 黄 / debug 灰）并前缀 kind 徽标；坏行
 * 原样灰显（日志是诊断面，坏行不给读制造障碍）。
 */
function LogLine({ raw }: { raw: string }) {
  // 行解析面（行 JSON 的已知字段；坏行原样展示）
  type ParsedLogLine = {
    ts?: string;
    level?: string;
    kind?: string;
    msg?: string;
  };
  let parsed: ParsedLogLine | null = null;
  try {
    parsed = JSON.parse(raw) as ParsedLogLine;
  } catch {
    parsed = null;
  }
  const level = parsed?.level ?? "";
  const levelClass =
    level === "error"
      ? "text-[var(--danger)]"
      : level === "warn"
        ? "text-[var(--warning)]"
        : level === "debug"
          ? "text-[var(--muted-foreground)]"
          : "text-[var(--foreground)]";
  if (!parsed) {
    return (
      <div className="text-ui-sm break-all whitespace-pre-wrap text-[var(--muted-foreground)]">
        {raw}
      </div>
    );
  }
  return (
    <div className={`text-ui-sm break-all whitespace-pre-wrap ${levelClass}`}>
      <span className="mr-2 font-mono text-[var(--muted-foreground)]">
        {parsed.ts}
        {parsed.kind ? ` [${parsed.kind}]` : ""}
      </span>
      {parsed.msg}
    </div>
  );
}

/**
 * 会话日志查看器（32 号票，FR-15-01/§10.7 诊断组）：会话选择器（日志文件
 * 清单过滤 sessions/ 前缀，活跃 Tab 做标题映射）→ read_session_log 拉尾部
 * 500 行渲染只读日志面 +「刷新」钮。拉取式（无实时流，票面边界）。
 */
function LogViewerSection() {
  const t = useT();
  const tabs = useSessionsStore((s) => s.tabs);
  const [files, setFiles] = useState<string[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [lines, setLines] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /** 会话日志文件 → (sessionId, 展示名)：活跃 Tab 的标题优先。 */
  const sessionOptions = useMemo(() => {
    const ids = new Set<string>();
    for (const name of files) {
      if (!name.startsWith("sessions/")) continue;
      const base = name.slice("sessions/".length).replace(/\.jsonl(\.1)?$/, "");
      if (base) ids.add(base);
    }
    return Array.from(ids).map((id) => ({
      id,
      label: tabs.find((tab) => tab.id === id)?.title ?? id,
    }));
  }, [files, tabs]);

  const refreshList = useCallback(() => {
    listLogFiles()
      .then((names) => setFiles(names))
      .catch((err: unknown) => {
        console.error("[settings] 日志清单拉取失败", err);
        setFiles([]);
      });
  }, []);
  // 挂载拉一次清单（拉取式：不在页签时不轮询）
  useEffect(() => {
    refreshList();
  }, [refreshList]);

  const fetchTail = useCallback((sessionId: string) => {
    setLoading(true);
    setError(null);
    readSessionLog({ sessionId, tail: 500 })
      .then((result) => setLines(result.lines))
      .catch((err: unknown) => {
        const message =
          typeof err === "object" && err !== null && "message" in err
            ? String((err as { message: unknown }).message)
            : String(err);
        setError(message);
        setLines([]);
      })
      .finally(() => setLoading(false));
  }, []);

  function onSelect(sessionId: string) {
    setSelected(sessionId);
    fetchTail(sessionId);
  }

  return (
    <SettingsSection title={t("settings.sectionLogViewer")}>
      <p className="border-b border-[var(--border)] px-4 py-3 text-ui-sm text-[var(--muted-foreground)]">
        {t("settings.logViewerDesc")}
      </p>
      <div className="flex items-center gap-3 px-4 py-3">
        <Select
          width="mid"
          aria-label={t("settings.logSessionPickerAria")}
          data-testid="log-session-select"
          value={selected ?? ""}
          onChange={(e) => {
            if (e.target.value !== "") onSelect(e.target.value);
          }}
        >
          <option value="">{t("settings.logSessionPicker")}</option>
          {sessionOptions.map((option) => (
            <option key={option.id} value={option.id}>
              {option.label}
            </option>
          ))}
        </Select>
        <Button
          variant="outline"
          size="sm"
          data-testid="log-refresh"
          aria-label={t("settings.logRefreshAria")}
          disabled={selected === null || loading}
          onClick={() => {
            refreshList();
            if (selected !== null) fetchTail(selected);
          }}
        >
          {loading ? "…" : t("settings.logRefresh")}
        </Button>
      </div>
      {error && (
        <p data-testid="log-viewer-error" className="px-4 pb-3 text-ui-sm text-[var(--danger)]">
          {error}
        </p>
      )}
      {selected === null && lines.length === 0 ? (
        <p className="px-4 py-10 text-center text-ui-base text-[var(--muted-foreground)]">
          {t("settings.logEmpty")}
        </p>
      ) : (
        <pre
          data-testid="log-viewer-lines"
          role="log"
          aria-label={t("settings.logLinesAria")}
          className="m-4 max-h-96 overflow-auto rounded border border-[var(--border)] bg-[var(--muted)] p-3 font-mono text-ui-sm"
        >
          {lines.map((raw, index) => (
            <LogLine
              // biome-ignore lint/suspicious/noArrayIndexKey: 日志面是只读 append-only 列表，行无稳定 id（内容可能重复），序号即身份（DiffView 同款先例）
              key={index}
              raw={raw}
            />
          ))}
        </pre>
      )}
    </SettingsSection>
  );
}

/** 日志页：级别下拉（verbose 说明）+ 打开日志目录（open_logs_dir）+
 *  会话日志查看器（32 号票，FR-15-01/FR-14-04/FR-14-07）。 */
function LogsPage() {
  const t = useT();
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  if (!config) return null;
  return (
    <div>
      <SettingsSection title={t("settings.sectionLogging")}>
        <SettingsRow title={t("settings.logLevel")} description={t("settings.logLevelDesc")}>
          <Select
            width="narrow"
            value={config.log.level}
            onChange={(e) =>
              patchConfig({ log: { level: e.target.value as AppConfig["log"]["level"] } })
            }
          >
            <option value="debug">debug</option>
            <option value="info">info</option>
            <option value="warn">warn</option>
            <option value="error">error</option>
          </Select>
        </SettingsRow>
        <SettingsRow title={t("settings.logDir")} description={t("settings.logDirDesc")}>
          <OpenLogsDirButton />
        </SettingsRow>
      </SettingsSection>
      <LogViewerSection />
    </div>
  );
}

/**
 * 工作区与智能体页（03 号票起承载本票两项；Shell 选择、压缩阈值等余项随后
 * 续票——10/04 号票——接入）：
 * - 自定义提示词（FR-09-06，agent.customPrompt）：多行文本，注入为每次对话
 *   的系统级前缀；空/未配置零注入；走草稿 + saveAll 提交，下次 run 生效；
 * - 模型流停滞超时（FR-03-07，watchdog.stallTimeoutSecs，37 号票）：数字
 *   输入（5–3600 秒，缺省 300）；按 run 装配驱动时读取生效（保存后下次
 *   对话应用）；
 * - 打开项目数据目录（FR-14-07）：当前活跃 Tab 所属项目的 `<homeDir>/.craftwave`，
 *   无项目会话时按钮禁用并说明原因。
 */
function AgentPage() {
  const t = useT();
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  const tabs = useSessionsStore((s) => s.tabs);
  const activeId = useSessionsStore((s) => s.activeId);
  const projects = useSessionsStore((s) => s.projects);
  if (!config) return null;
  const customPrompt = config.agent?.customPrompt ?? "";
  const patchAgent = (patch: Partial<NonNullable<AppConfig["agent"]>>) =>
    patchConfig({
      // 生成物 AgentConfig 的 maxConcurrentSubagents 是必填（serde default
      // 回填 4）：旧草稿缺 agent 组时以缺省值补底再覆写
      agent: {
        maxConcurrentSubagents: config.agent?.maxConcurrentSubagents ?? 4,
        ...config.agent,
        ...patch,
      },
    });
  const stallTimeoutSecs = config.watchdog.stallTimeoutSecs;
  const patchWatchdog = (patch: Partial<AppConfig["watchdog"]>) =>
    patchConfig({
      watchdog: { ...config.watchdog, ...patch },
    });
  // 当前项目：活跃 Tab 所属（无活跃 Tab 或临时会话 → 无项目数据目录可开）
  const activeTab = tabs.find((t) => t.id === activeId);
  const project = activeTab?.projectId
    ? projects.find((p) => p.id === activeTab.projectId)
    : undefined;
  const projectDataDir = project ? `${project.homeDir.replace(/[\\/]+$/, "")}/.craftwave` : null;

  return (
    <div>
      <SettingsSection title={t("settings.sectionAgent")}>
        <SettingsRow
          title={t("settings.customPrompt")}
          description={t("settings.customPromptDesc")}
        >
          <Textarea
            aria-label={t("settings.customPromptAria")}
            data-testid="custom-prompt-input"
            className="min-h-24 w-[360px] max-w-full"
            rows={4}
            placeholder={t("settings.customPromptPlaceholder")}
            value={customPrompt}
            onChange={(e) =>
              patchAgent({ customPrompt: e.target.value === "" ? undefined : e.target.value })
            }
          />
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title={t("settings.sectionSubagents")}>
        <SettingsRow
          title={t("settings.subagentConcurrency")}
          description={t("settings.subagentConcurrencyDesc")}
        >
          <label className="flex w-full items-center gap-3 text-ui-base">
            <input
              aria-label={t("settings.subagentConcurrencyAria")}
              data-testid="subagent-concurrency-input"
              type="number"
              min={1}
              max={16}
              step={1}
              value={config.agent?.maxConcurrentSubagents ?? 4}
              onChange={(e) => {
                const parsed = Number(e.target.value);
                if (Number.isFinite(parsed) && parsed > 0) {
                  patchAgent({ maxConcurrentSubagents: parsed });
                }
              }}
              className="w-32 rounded-[var(--cw-radius)] border border-[var(--cw-border)] bg-transparent px-3 py-2 text-ui-base tabular-nums outline-none focus:border-[var(--cw-focus)]"
            />
            <span className="shrink-0 text-ui-sm text-[var(--muted-foreground)]">
              {t("settings.range116")}
            </span>
          </label>
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title={t("settings.sectionWatchdog")}>
        <SettingsRow
          title={t("settings.stallTimeout")}
          description={t("settings.stallTimeoutDesc")}
        >
          <label className="flex w-full items-center gap-3 text-ui-base">
            <input
              aria-label={t("settings.stallTimeoutAria")}
              data-testid="watchdog-stall-timeout-input"
              type="number"
              min={5}
              max={3600}
              step={1}
              value={stallTimeoutSecs}
              onChange={(e) => {
                const parsed = Number(e.target.value);
                if (Number.isFinite(parsed) && parsed > 0) {
                  patchWatchdog({ stallTimeoutSecs: parsed });
                }
              }}
              className="w-32 rounded-[var(--cw-radius)] border border-[var(--cw-border)] bg-transparent px-3 py-2 text-ui-base tabular-nums outline-none focus:border-[var(--cw-focus)]"
            />
            <span className="shrink-0 text-ui-sm text-[var(--muted-foreground)]">
              {t("settings.range53600")}
            </span>
          </label>
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title={t("settings.sectionAudit")}>
        <SettingsRow title={t("settings.auditModel")} description={t("settings.auditModelDesc")}>
          <Select
            width="wide"
            aria-label={t("settings.auditModelAria")}
            data-testid="command-audit-model-select"
            value={config.agent?.commandAudit?.modelId ?? ""}
            onChange={(e) => {
              const value = e.target.value;
              patchAgent({
                commandAudit: value === "" ? undefined : { modelId: value },
              });
            }}
          >
            <option value="">{t("settings.auditFollowSession")}</option>
            {config.providers.flatMap((provider) =>
              provider.models.map((model) => (
                <option key={model.id} value={model.id}>
                  {provider.name} / {model.model}
                </option>
              )),
            )}
          </Select>
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title={t("settings.sectionProjectData")}>
        <SettingsRow
          title={t("settings.projectDataDir")}
          description={t("settings.projectDataDirDesc")}
        >
          <OpenDirButton
            testId="open-project-data-dir"
            label={t("settings.openProjectDataDir")}
            path={projectDataDir ?? ""}
            disabled={projectDataDir === null}
            disabledReason={t("settings.projectDataDirDisabled")}
          />
        </SettingsRow>
      </SettingsSection>
    </div>
  );
}

/** 关于页：应用身份只读信息 + 检查更新区（33 号票，FR-16-03）+ 打开数据
 * 目录与备份目录入口（03 号票 FR-14-07 / 33 号票 Q43 备份目录入口） */
function AboutPage() {
  const t = useT();
  return (
    <div>
      <section className="settings-section">
        <div className="settings-section-card flex items-center gap-4 p-5">
          <img
            src="/app-icon.png"
            alt={t("settings.appIconAlt")}
            className="h-16 w-16 rounded-[14px]"
          />
          <div className="flex min-w-0 flex-col gap-1">
            <span className="text-ui-lg font-semibold">CraftWave</span>
            <span className="text-ui-base text-[var(--muted-foreground)]">
              {t("settings.appTagline")}
            </span>
            <span className="font-mono text-ui-sm text-[var(--muted-foreground)]">
              {t("settings.configPath")}
            </span>
          </div>
        </div>
      </section>
      <section className="settings-section">
        <h3 className="settings-section-title">{t("settings.sectionUpdate")}</h3>
        <UpdateSection />
      </section>
      <section className="settings-section">
        <h3 className="settings-section-title">{t("settings.sectionData")}</h3>
        <div className="settings-section-card">
          <SettingsRow title={t("settings.dataDir")} description={t("settings.dataDirDesc")}>
            <OpenDirButton
              testId="open-data-dir"
              label={t("settings.openDataDir")}
              path="~/.craftwave"
            />
          </SettingsRow>
          <SettingsRow title={t("settings.backupDir")} description={t("settings.backupDirDesc")}>
            <OpenDirButton
              testId="open-backups-dir"
              label={t("settings.openBackupDir")}
              path="~/.craftwave/backups"
            />
          </SettingsRow>
        </div>
      </section>
    </div>
  );
}

/** 占位页：导航分组照抄 CodeWave，余下页内容随后续里程碑接线 */
function PlaceholderPage({ page }: { page: PageKey }) {
  const t = useT();
  const hints: Record<string, string> = {
    mcp: t("settings.placeholderMcp"),
    tools: t("settings.placeholderTools"),
  };
  return (
    <div>
      <section className="settings-section">
        <div className="settings-section-card px-5 py-10 text-center text-ui-base text-[var(--muted-foreground)]">
          {hints[page] ?? t("settings.placeholderDefault")}
        </div>
      </section>
    </div>
  );
}

export { normalizePageKey };
