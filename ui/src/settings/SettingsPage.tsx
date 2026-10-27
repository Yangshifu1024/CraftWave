/**
 * 设置页（布局照抄 CodeWave SettingsPage：全屏覆盖式 dialog）。
 *
 * 容器承担四件事：
 *   1. 左导航列（返回工作区 + 搜索 + 三组 10 页导航，页行带脏圆点）；
 *   2. 右内容列（操作条：页标题 + 取消/保存；页体：50% 宽居中）；
 *   3. 逐页脏标记（draft 与已保存基线的字段差集，字段归属由 PAGE_FIELDS 驱动）；
 *   4. 离开拦截（切页 / 返回 / 页内 Esc 共用同一份三选弹框）。
 * 页面 JSX 手写；注册表（settings/registry.ts）只提供页序、页名与分组。
 * M1 数据面：providers / activeModelId / ui / proxy / approval / log 真实接线；
 * MCP / 技能 / 写入后检查 / 工作区与智能体四页保留导航占位，随后续里程碑接线。
 */

import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { cn } from "@/components/ui/cn";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { AppConfig } from "@/ipc/types";
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
import { ProvidersPage } from "@/settings/ProvidersPage";
import {
  DEFAULT_PAGE,
  normalizePageKey,
  PAGE_GROUPS,
  PAGE_LABEL,
  PAGE_ORDER,
  type PageKey,
} from "@/settings/registry";
import { useSettingsStore } from "@/stores/settings";
import { applyTheme } from "@/theme";

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
      return config.proxy ?? null;
    case "security":
      return config.approval;
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

export function SettingsPage({ onClose }: { onClose: () => void }) {
  const config = useSettingsStore((s) => s.config);
  const baseline = useSettingsStore((s) => s.baseline);
  const loading = useSettingsStore((s) => s.loading);
  const saving = useSettingsStore((s) => s.saving);
  const error = useSettingsStore((s) => s.error);
  const load = useSettingsStore((s) => s.load);
  const saveAll = useSettingsStore((s) => s.saveAll);
  const resetDraft = useSettingsStore((s) => s.resetDraft);

  const [tab, setTab] = useState<PageKey>(DEFAULT_PAGE);
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

  // 搜索命中：按页名过滤（trim 后非空即搜索态，结果替掉导航列表）
  const searching = query.trim() !== "";
  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q === "") return [] as PageKey[];
    return PAGE_ORDER.filter((key) => PAGE_LABEL[key].toLowerCase().includes(q));
  }, [query]);

  /** 切页 / 返回工作区共用：有脏改动先走三选拦截 */
  function requestTab(next: PageKey) {
    if (next === tab) return;
    if (!anyDirty) {
      setTab(next);
      return;
    }
    setLeaveIntent({ kind: "tab", tab: next });
  }

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
      const ok = await saveAll();
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

  // 页内 Esc：返回工作区（脏改动存在时同样先走三选）。浮层（select 下拉/弹框）展开时让位。
  const closeRef = useRef(requestClose);
  closeRef.current = requestClose;
  useEffect(() => {
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing) return;
      const overlayOpen =
        leaveIntent !== null ||
        document.activeElement?.tagName === "SELECT" ||
        !!document.querySelector(".settings-dialog-mask");
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

  const pages: { key: PageKey; body: ReactNode }[] = [
    { key: "appearance", body: <AppearancePage /> },
    { key: "providers", body: config ? <ProvidersPage /> : null },
    { key: "network", body: config ? <NetworkPage /> : null },
    { key: "security", body: config ? <SecurityPage /> : null },
    { key: "mcp", body: <PlaceholderPage page="mcp" /> },
    { key: "skills", body: <PlaceholderPage page="skills" /> },
    { key: "tools", body: <PlaceholderPage page="tools" /> },
    { key: "agent", body: <PlaceholderPage page="agent" /> },
    { key: "logs", body: config ? <LogsPage /> : null },
    { key: "about", body: <AboutPage /> },
  ];
  const activePage = pages.find((p) => p.key === tab) ?? pages[0];

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
      aria-label="设置"
    >
      <nav className="settings-nav" aria-label="设置分区导航">
        <div className="settings-nav-head">
          <Button
            ref={backRef}
            variant="ghost"
            className="settings-nav-back w-full justify-start"
            onClick={requestClose}
          >
            <ArrowLeftIcon className="h-4 w-4" />
            返回工作区
          </Button>
          <div className="settings-search relative">
            <SearchIcon className="pointer-events-none absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2 text-[var(--muted-foreground)]" />
            <Input
              className="pl-9"
              aria-label="搜索设置"
              placeholder="搜索设置"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
        </div>

        {searching ? (
          <div className="settings-nav-list" role="listbox" aria-label="搜索结果">
            {results.length === 0 ? (
              <div className="px-2 py-3 text-xs text-[var(--cw-dim)]">
                <div className="mb-1 text-[13px] text-[var(--cw-text-2)]">没有匹配的设置页</div>
                换个关键词试试
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
                    <span className="truncate">{PAGE_LABEL[key]}</span>
                    <span className="ml-auto text-[11px] text-[var(--cw-dim)]">
                      {PAGE_LABEL[key]}
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
                  {group.title}
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
                        {PAGE_LABEL[key]}
                        {dirtyMap[key] && (
                          <span className="settings-dirty-dot" title="有未保存的改动" />
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
        {error && <Alert variant="error" title={`错误：${error}`} className="m-4" />}
        {loading && !config && <Alert variant="info" title="加载配置中…" className="m-4" />}
        <div className="settings-actions">
          <span className="settings-actions-title">{PAGE_LABEL[tab]}</span>
          {anyDirty && <span className="settings-dirty-dot" title="有未保存的改动" />}
          <div className="settings-actions-buttons">
            <Button
              variant="outline"
              disabled={!config}
              onClick={() => {
                void resetDraft().then(() => onClose());
              }}
            >
              取消
            </Button>
            <Button disabled={!config || saving} onClick={() => void saveAll()}>
              {saving ? "保存中…" : "保存"}
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
        title="有未保存的设置改动"
        onClose={() => void answerLeave("stay")}
        actions={[
          { label: "留在原地", variant: "ghost", onClick: () => void answerLeave("stay") },
          { label: "放弃改动", variant: "danger", onClick: () => void answerLeave("discard") },
          { label: "保存并离开", onClick: () => void answerLeave("save") },
        ]}
      >
        离开前请选择如何处理这些改动；取消按钮会直接放弃全部未保存改动。
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

/** 界面页：主题 + 界面语言 + AI 回复语言（ui 段整卡）+ 系统通知偏好 */
function AppearancePage() {
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  // 主题草稿即时预览（FR-13-03）：草稿变化即应用；丢弃草稿/保存都会令
  // config 回到基线，本副作用随之回滚——无需额外的取消预览逻辑
  useEffect(() => {
    if (config?.ui?.theme) applyTheme(config.ui.theme);
  }, [config?.ui?.theme]);
  if (!config) return null;
  const patchUi = (patch: Partial<AppConfig["ui"]>) =>
    patchConfig({ ui: { ...config.ui, ...patch } });
  const notifications = config.notifications ?? { enabled: true, interventionOnly: false };
  const patchNotifications = (patch: Partial<AppConfig["notifications"]>) =>
    patchConfig({ notifications: { ...notifications, ...patch } });

  return (
    <div>
      <SettingsSection title="主题">
        <div className="py-4">
          <div className="settings-choice-list">
            <ChoiceCard
              active={(config.ui.theme ?? "system") === "light"}
              title="浅色"
              description="常亮界面，适合白天与强光环境。"
              onClick={() => patchUi({ theme: "light" })}
            />
            <ChoiceCard
              active={(config.ui.theme ?? "system") === "dark"}
              title="深色"
              description="暗色界面，适合夜间与低光环境。"
              onClick={() => patchUi({ theme: "dark" })}
            />
            <ChoiceCard
              active={(config.ui.theme ?? "system") === "system"}
              title="跟随系统"
              description="随操作系统的外观设置自动切换。"
              onClick={() => patchUi({ theme: "system" })}
            />
          </div>
        </div>
      </SettingsSection>
      <SettingsSection title="系统通知">
        <SettingsRow
          title="启用系统通知"
          description="窗口后台/失焦时，运行完成与人工介入经系统通知提醒。"
        >
          <Switch
            checked={notifications.enabled}
            onCheckedChange={(v) => patchNotifications({ enabled: v })}
          />
        </SettingsRow>
        <SettingsRow
          title="仅人工介入时通知"
          description="开启后只在询问/审批等待时通知，运行完成不提醒。"
        >
          <Switch
            checked={notifications.interventionOnly}
            onCheckedChange={(v) => patchNotifications({ interventionOnly: v })}
          />
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title="语言">
        <SettingsRow title="界面语言" description="设置界面与提示文案的显示语言。">
          <Select
            width="mid"
            value={config.ui.language}
            onChange={(e) => patchUi({ language: e.target.value as AppConfig["ui"]["language"] })}
          >
            <option value="zh-CN">简体中文</option>
            <option value="en-US">English</option>
          </Select>
        </SettingsRow>
        <SettingsRow title="AI 回复语言" description="留空跟随用户输入语言。">
          <Input
            className="w-[240px] max-w-full"
            maxLength={40}
            placeholder="如：简体中文"
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

/** 网络页：代理模式三选 + 自定义地址（M1 保存到 config.proxy；代理生效链路随后续票） */
function NetworkPage() {
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  if (!config) return null;
  const mode = config.proxy?.mode ?? "system";
  const manualUrl = config.proxy?.url ?? "";

  return (
    <div>
      <SettingsSection title="代理">
        <div className="py-4">
          <div className="settings-choice-list">
            <ChoiceCard
              active={mode === "none"}
              title="不使用代理"
              description="所有请求直连目标服务器。"
              onClick={() => patchConfig({ proxy: { mode: "none" } })}
            />
            <ChoiceCard
              active={mode === "system"}
              title="系统代理"
              description="读取操作系统代理设置（默认）。"
              onClick={() => patchConfig({ proxy: { mode: "system" } })}
            />
            <ChoiceCard
              active={mode === "manual"}
              title="自定义"
              description="手动指定 HTTP(S)/SOCKS5 代理地址。"
              onClick={() => patchConfig({ proxy: { mode: "manual", url: manualUrl } })}
            />
          </div>
        </div>
        {mode === "manual" && (
          <div className="border-t border-[var(--border)] py-4">
            <SettingsRow title="代理地址" description="支持 http://、https://、socks5:// 前缀。">
              <Input
                className="w-[360px] max-w-full font-mono text-xs"
                placeholder="http://127.0.0.1:7890 或 socks5://127.0.0.1:1080"
                value={manualUrl}
                onChange={(e) => patchConfig({ proxy: { mode: "manual", url: e.target.value } })}
              />
            </SettingsRow>
          </div>
        )}
      </SettingsSection>
    </div>
  );
}

/** 安全与审批页：审批开关 + 自动放行 + 命令白名单（只读展示） */
function SecurityPage() {
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  if (!config) return null;
  const approval = config.approval;
  const patchApproval = (patch: Partial<AppConfig["approval"]>) =>
    patchConfig({ approval: { ...approval, ...patch } });
  const allowlist = approval.commandAllowlist ?? [];

  return (
    <div>
      <SettingsSection title="审批策略">
        <SettingsRow title="启用操作审批" description="写入文件、执行命令等敏感操作先询问再执行。">
          <Switch
            aria-label="启用操作审批"
            checked={approval.enabled}
            onCheckedChange={(v) => patchApproval({ enabled: v })}
          />
        </SettingsRow>
        <SettingsRow title="自动放行等待" description="审批等待 5 分钟无应答时自动选择推荐项。">
          <Switch
            aria-label="自动放行等待"
            checked={approval.autoConfirm}
            onCheckedChange={(v) => patchApproval({ autoConfirm: v })}
          />
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title="命令白名单">
        {allowlist.length === 0 ? (
          <p className="py-10 text-center text-sm text-[var(--muted-foreground)]">
            还没有「始终允许」的命令；审批时勾选「始终允许本项目」会出现在这里。
          </p>
        ) : (
          <div className="py-3">
            {allowlist.map((entry, index) => {
              const sep = entry.indexOf("");
              const command = sep >= 0 ? entry.slice(sep + 1) : entry;
              return (
                <div
                  key={entry}
                  className="flex items-start justify-between gap-4 border-t border-[var(--border)] py-3 first:border-t-0 first:pt-0"
                >
                  <code className="min-w-0 flex-1 font-mono text-xs break-all text-[var(--foreground)]">
                    {command.trim() || "（无效条目）"}
                  </code>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      patchApproval({ commandAllowlist: allowlist.filter((_, i) => i !== index) })
                    }
                  >
                    删除
                  </Button>
                </div>
              );
            })}
          </div>
        )}
      </SettingsSection>
    </div>
  );
}

/** 日志页：日志级别下拉 */
function LogsPage() {
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);
  if (!config) return null;
  return (
    <div>
      <SettingsSection title="日志记录">
        <SettingsRow title="日志级别" description="进程日志的输出级别；排障时可临时调到 debug。">
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
      </SettingsSection>
    </div>
  );
}

/** 关于页：应用身份只读信息 */
function AboutPage() {
  return (
    <div>
      <section className="settings-section">
        <div className="settings-section-card flex items-center gap-4 p-5">
          <img src="/app-icon.png" alt="CraftWave 图标" className="h-16 w-16 rounded-[14px]" />
          <div className="flex min-w-0 flex-col gap-1">
            <span className="text-base font-semibold">CraftWave</span>
            <span className="text-sm text-[var(--muted-foreground)]">
              本地优先的 AI 结对工作台（M1）
            </span>
            <span className="font-mono text-xs text-[var(--muted-foreground)]">
              配置文件 ~/.craftwave/config.json
            </span>
          </div>
        </div>
      </section>
    </div>
  );
}

/** 占位页：导航分组照抄 CodeWave，四页内容随后续里程碑接线 */
function PlaceholderPage({ page }: { page: PageKey }) {
  const hints: Record<string, string> = {
    mcp: "MCP 服务器管理（连接状态与 mcp.json 编辑）随后续里程碑提供。",
    skills: "技能发现与启停管理随后续里程碑提供。",
    tools: "写入后检查与结果校验的配置随后续里程碑提供。",
    agent: "Shell 选择、自定义提示词与压缩阈值随后续里程碑提供。",
  };
  return (
    <div>
      <section className="settings-section">
        <div className="settings-section-card px-5 py-10 text-center text-sm text-[var(--muted-foreground)]">
          {hints[page] ?? "该分区随后续里程碑提供。"}
        </div>
      </section>
    </div>
  );
}

export { normalizePageKey };
